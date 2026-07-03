use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{self, Config};
use crate::core_service::CoreService;
use crate::plugin_sdk::PluginRegistry;
use crate::runtime::log_info;
use crate::runtime::log_warn;
use crate::runtime_overlay_rows::PendingUninstallConfirmation;
use crate::runtime_search_session::OverlaySearchSession;
#[cfg(target_os = "windows")]
use crate::overlay::NativeOverlayShell;

#[cfg(target_os = "windows")]
const QUEUED_DISCOVERY_REINDEX_DEBOUNCE_MS: u64 = 1200;
#[cfg(target_os = "windows")]
const CONFIG_RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[cfg(target_os = "windows")]
#[derive(Debug)]
pub(crate) struct RuntimeConfigWatcher {
    pub(crate) path: std::path::PathBuf,
    pub(crate) last_checked: Instant,
    pub(crate) last_modified: Option<SystemTime>,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
pub(crate) struct BackgroundIndexRefresh {
    pub(crate) completed: Arc<AtomicBool>,
    pub(crate) result: Arc<Mutex<Option<Result<crate::core_service::IndexRefreshReport, String>>>>,
    pub(crate) cache_applied: bool,
    pub(crate) indexes_synced: bool,
    pub(crate) initial_cache_empty: bool,
    pub(crate) pending_discovery_reindex: bool,
    pub(crate) pending_discovery_reindex_due_at: Option<Instant>,
    pub(crate) pending_discovery_reindex_requests: usize,
    pub(crate) started_at: Instant,
    pub(crate) startup_started_at: Instant,
}

#[cfg(target_os = "windows")]
pub(crate) fn start_background_index_refresh(
    config: &Config,
    initial_cache_empty: bool,
    startup_started_at: Instant,
) -> BackgroundIndexRefresh {
    let completed = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let completed_worker = completed.clone();
    let result_worker = result.clone();
    let worker_config = config.clone();
    std::thread::spawn(move || {
        // Catch panics so a buggy provider can never silently leave the main
        // thread waiting on a completion flag that will never flip.
        let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            CoreService::new(worker_config)
                .map(|service| service.with_runtime_providers())
                .and_then(|service| service.rebuild_index_incremental_with_report())
                .map_err(|error| format!("background indexing failed: {error}"))
        })) {
            Ok(result) => result,
            Err(payload) => {
                let message = panic_message_to_string(&payload);
                log_warn(&format!(
                    "[nex] background indexing thread panicked: {message}"
                ));
                Err(format!("background indexing panicked: {message}"))
            }
        };
        let mut slot = match result_worker.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some(outcome);
        completed_worker.store(true, Ordering::Release);
    });

    BackgroundIndexRefresh {
        completed,
        result,
        cache_applied: false,
        indexes_synced: false,
        initial_cache_empty,
        pending_discovery_reindex: false,
        pending_discovery_reindex_due_at: None,
        pending_discovery_reindex_requests: 0,
        started_at: Instant::now(),
        startup_started_at,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn maybe_apply_background_index_refresh(
    service: &CoreService,
    state: &mut BackgroundIndexRefresh,
    runtime_config: &Config,
) {
    if state.cache_applied {
        if !state.indexes_synced {
            match service.sync_indexes_from_cache() {
                Ok(()) => {
                    state.indexes_synced = true;
                    log_info("[nex] Tantivy/FTS5 search indexes synced from cache");
                }
                Err(e) => {
                    log_warn(&format!(
                        "[nex] background indexing Tantivy/FTS5 sync failed: {e}"
                    ));
                }
            }
        }
        maybe_start_queued_discovery_reindex(service, state, runtime_config);
        return;
    }
    if !state.completed.load(Ordering::Acquire) {
        return;
    }

    let outcome = {
        let mut slot = match state.result.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.take()
    };

    match outcome {
        Some(Ok(report)) => {
            let elapsed_ms = state.started_at.elapsed().as_millis();
            let startup_elapsed_ms = state.startup_started_at.elapsed().as_millis();
            log_info(&format!(
                "[nex] startup_phase phase=indexing_completed elapsed_ms={} worker_elapsed_ms={} indexed_items={} discovered={} upserted={} removed={}",
                startup_elapsed_ms,
                elapsed_ms,
                report.indexed_total,
                report.discovered_total,
                report.upserted_total,
                report.removed_total
            ));
            match service.reload_cache_from_store() {
                Ok(cached_items) => {
                    log_info(&format!(
                        "[nex] startup_phase phase=cache_applied elapsed_ms={} cached_items={} initial_cache_empty={}",
                        startup_elapsed_ms,
                        cached_items,
                        state.initial_cache_empty
                    ));
                    for provider in &report.providers {
                        log_info(&format!(
                            "[nex] index_provider name={} discovered={} upserted={} removed={} skipped={} elapsed_ms={}",
                            provider.provider,
                            provider.discovered,
                            provider.upserted,
                            provider.removed,
                            provider.skipped,
                            provider.elapsed_ms
                        ));
                    }
                    if let Err(e) = service.sync_indexes_from_cache() {
                        log_warn(&format!(
                        "[nex] background indexing Tantivy sync failed: {e}"
                        ));
                    } else {
                        state.indexes_synced = true;
                    log_info("[nex] Tantivy search index synced from cache");
                    }
                }
                Err(error) => {
                    log_warn(&format!(
                        "[nex] background indexing cache refresh failed: {error}"
                    ));
                }
            }
        }
        Some(Err(error)) => {
            log_warn(&format!("[nex] {error}"));
        }
        None => {
            log_warn("[nex] background indexing completed without result");
        }
    }

    state.cache_applied = true;

    if state.pending_discovery_reindex {
        log_info(
            "[nex] discovery settings queued during indexing; pending reindex remains scheduled",
        );
        maybe_start_queued_discovery_reindex(service, state, runtime_config);
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn queue_discovery_reindex_after_active_index(state: &mut BackgroundIndexRefresh) {
    state.pending_discovery_reindex = true;
    state.pending_discovery_reindex_requests =
        state.pending_discovery_reindex_requests.saturating_add(1);
    state.pending_discovery_reindex_due_at =
        Some(Instant::now() + Duration::from_millis(QUEUED_DISCOVERY_REINDEX_DEBOUNCE_MS));
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn queued_discovery_reindex_is_due(
    cache_applied: bool,
    pending: bool,
    due_at: Option<Instant>,
    now: Instant,
) -> bool {
    cache_applied && pending && due_at.is_some_and(|due| now >= due)
}

#[cfg(target_os = "windows")]
pub(crate) fn maybe_start_queued_discovery_reindex(
    service: &CoreService,
    state: &mut BackgroundIndexRefresh,
    runtime_config: &Config,
) {
    if !queued_discovery_reindex_is_due(
        state.cache_applied,
        state.pending_discovery_reindex,
        state.pending_discovery_reindex_due_at,
        Instant::now(),
    ) {
        return;
    }

    let request_count = state.pending_discovery_reindex_requests.max(1);
    let startup_started_at = state.startup_started_at;
    log_info(&format!(
        "[nex] discovery settings queued during indexing; starting debounced reindex requests={} debounce_ms={}",
        request_count,
        QUEUED_DISCOVERY_REINDEX_DEBOUNCE_MS
    ));
    log_info(&format!(
        "[nex] startup_phase phase=indexing_started elapsed_ms={} initial_cache_empty=false cached_items={}",
        startup_started_at.elapsed().as_millis(),
        service.cached_items_len()
    ));
    *state = start_background_index_refresh(runtime_config, false, startup_started_at);
}

#[cfg(target_os = "windows")]
pub(crate) fn config_file_modified_time(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn panic_message_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn maybe_apply_runtime_config_reload(
    overlay: &NativeOverlayShell,
    service: &CoreService,
    runtime_config: &mut Config,
    plugin_registry: &mut PluginRegistry,
    search_session: &mut OverlaySearchSession,
    pending_uninstall_confirmation: &mut Option<PendingUninstallConfirmation>,
    max_results: &mut usize,
    watcher: &mut RuntimeConfigWatcher,
    background_index_refresh: &mut BackgroundIndexRefresh,
) {
    if watcher.last_checked.elapsed() < CONFIG_RELOAD_POLL_INTERVAL {
        return;
    }
    watcher.last_checked = Instant::now();
    let modified = config_file_modified_time(watcher.path.as_path());
    if modified == watcher.last_modified {
        return;
    }
    watcher.last_modified = modified;

    match config::load(Some(watcher.path.as_path())) {
        Ok(next_config) => {
            let previous = runtime_config.clone();
            let hotkey_changed = next_config.hotkey != previous.hotkey;
            let index_db_path_changed = next_config.index_db_path != previous.index_db_path;
            let discovery_config_changed = next_config.discovery_roots != previous.discovery_roots
                || next_config.discovery_exclude_roots != previous.discovery_exclude_roots
                || next_config.show_files != previous.show_files
                || next_config.show_folders != previous.show_folders
                || next_config.index_max_items_total != previous.index_max_items_total
                || next_config.index_max_items_per_root != previous.index_max_items_per_root
                || next_config.index_max_items_per_query_seed
                    != previous.index_max_items_per_query_seed;
            let mut discovery_reindex_queued = false;
            *runtime_config = next_config;
            *max_results = runtime_config.max_results as usize;

            overlay.set_performance_tuning(
                runtime_config.idle_cache_trim_ms,
                runtime_config.active_memory_target_mb,
                runtime_config.ui_warm_release_ms,
            );
            overlay.set_game_mode_enabled(runtime_config.game_mode_enabled);
            *plugin_registry = PluginRegistry::load_from_config(runtime_config);
            for warning in &plugin_registry.load_warnings {
                log_warn(&format!("[nex] plugin_warning {warning}"));
            }
            search_session.clear();
            *pending_uninstall_confirmation = None;

            if hotkey_changed {
                log_warn(&format!(
                    "[nex] config hotkey changed ({} -> {}), restart required to apply",
                    previous.hotkey, runtime_config.hotkey
                ));
            }
            if index_db_path_changed {
                log_warn("[nex] config index_db_path changed; restart required to apply");
            }
            if discovery_config_changed {
                if let Err(error) = service.reconfigure_runtime_providers(runtime_config) {
                    log_warn(&format!(
                        "[nex] provider reconfigure failed after config reload: {error}"
                    ));
                } else {
                    if background_index_refresh.cache_applied {
                        *background_index_refresh = start_background_index_refresh(
                            runtime_config,
                            false,
                            background_index_refresh.startup_started_at,
                        );
                        log_info("[nex] discovery settings changed; background reindex started");
                    } else {
                        queue_discovery_reindex_after_active_index(background_index_refresh);
                        discovery_reindex_queued = true;
                        log_info(&format!(
                            "[nex] discovery settings changed while indexing is active; reindex queued debounce_ms={} requests={}",
                            QUEUED_DISCOVERY_REINDEX_DEBOUNCE_MS,
                            background_index_refresh.pending_discovery_reindex_requests
                        ));
                    }
                }
            }

            log_info(&format!(
                "[nex] config reloaded max_results={} mode={:?} show_files={} show_folders={} game_mode={} dsl={} clipboard={} uninstall_actions={} plugins_enabled={} plugins_actions={} index_caps_total={} index_caps_per_root={} index_seed_cap={}",
                runtime_config.max_results,
                runtime_config.search_mode_default,
                runtime_config.show_files,
                runtime_config.show_folders,
                runtime_config.game_mode_enabled,
                runtime_config.search_dsl_enabled,
                runtime_config.clipboard_enabled,
                runtime_config.uninstall_actions_enabled,
                runtime_config.plugins_enabled,
                plugin_registry.action_items.len(),
                runtime_config.index_max_items_total,
                runtime_config.index_max_items_per_root,
                runtime_config.index_max_items_per_query_seed,
            ));

            if discovery_config_changed {
                if discovery_reindex_queued {
                    overlay.set_status_text(
                        "Discovery settings queued; reindex starts after debounce",
                    );
                } else {
                    overlay.set_status_text("Discovery settings updated; reindexing...");
                }
            } else if index_db_path_changed {
                overlay.set_status_text("Restart required to apply index path changes");
            } else if hotkey_changed {
                overlay.set_status_text("Restart required to apply hotkey changes");
            } else {
                overlay.set_status_text("Settings applied");
            }
        }
        Err(error) => {
            log_warn(&format!(
                "[nex] config reload skipped due to invalid config: {error}"
            ));
        }
        }
    }
