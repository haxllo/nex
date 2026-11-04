use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{self, Config};
use crate::core_service::CoreService;
use crate::plugin_sdk::PluginRegistry;
use crate::runtime::log_info;
use crate::runtime::log_warn;
use crate::runtime_overlay_rows::PendingConfirmation;
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
    pub(crate) apply_started: Arc<AtomicBool>,
    pub(crate) apply_done: Arc<AtomicBool>,
    /// Set in `NexUIProcess`: the indexer publishes snapshots and a monitor
    /// thread reports when one has been applied.
    pub(crate) ui_snapshot_applied: Option<Arc<AtomicBool>>,
}

#[cfg(target_os = "windows")]
pub(crate) fn start_background_index_refresh(
    service: &Arc<RwLock<CoreService>>,
    _config: &Config,
    initial_cache_empty: bool,
    startup_started_at: Instant,
) -> BackgroundIndexRefresh {
    // The UI process never indexes: discovery and Tantivy live in the indexer.
    start_ui_snapshot_sync(service, initial_cache_empty, startup_started_at)
}

/// `NexUIProcess` never discovers or indexes locally: it mirrors the indexer's
/// published snapshot into its in-memory search set.
#[cfg(target_os = "windows")]
fn start_ui_snapshot_sync(
    service: &Arc<RwLock<CoreService>>,
    initial_cache_empty: bool,
    startup_started_at: Instant,
) -> BackgroundIndexRefresh {
    let applied = Arc::new(AtomicBool::new(false));
    let applied_worker = applied.clone();
    let service = Arc::clone(service);
    std::thread::spawn(move || {
        let mut last_generation = 0_u64;
        loop {
            match sync_indexer_snapshot(last_generation) {
                SnapshotSync::Applied(generation, items) => {
                    let count = items.len();
                    // The main loop only takes short read locks; retry a
                    // write instead of stalling it behind a writer request.
                    for _ in 0..40 {
                        match service.try_write() {
                            Ok(svc) => {
                                svc.apply_external_snapshot(items);
                                last_generation = generation;
                                applied_worker.store(true, Ordering::Release);
                                log_info(&format!(
                                    "[nex] ui_snapshot_applied items={count} generation={generation}"
                                ));
                                break;
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(50)),
                        }
                    }
                }
                SnapshotSync::Unchanged => {}
                SnapshotSync::Unreachable => {
                    crate::ipc::ensure_indexer_running(crate::ipc::INDEXER_ADDR);
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    BackgroundIndexRefresh {
        completed: Arc::new(AtomicBool::new(true)),
        result: Arc::new(Mutex::new(None)),
        cache_applied: false,
        indexes_synced: false,
        initial_cache_empty,
        pending_discovery_reindex: false,
        pending_discovery_reindex_due_at: None,
        pending_discovery_reindex_requests: 0,
        started_at: Instant::now(),
        startup_started_at,
        apply_started: Arc::new(AtomicBool::new(true)),
        apply_done: Arc::new(AtomicBool::new(true)),
        ui_snapshot_applied: Some(applied),
    }
}

#[cfg(target_os = "windows")]
enum SnapshotSync {
    /// Nothing changed (or a rebuild is in flight); keep the last generation.
    Unchanged,
    Applied(u64, Vec<crate::model::SearchItem>),
    Unreachable,
}

/// One request per poll: the indexer reports whether the generation moved and
/// ships items only when it did. Replaces the old status+snapshot pair, which
/// opened two connections and always transferred the full store.
#[cfg(target_os = "windows")]
fn sync_indexer_snapshot(known_generation: u64) -> SnapshotSync {
    use crate::ipc::{ipc_call, items_from_values, IpcRequest, INDEXER_ADDR};
    use serde_json::{json, Value};

    let response = match ipc_call(
        INDEXER_ADDR,
        &IpcRequest::new("sync", json!({ "generation": known_generation })),
    ) {
        Ok(response) if response.ok => response,
        _ => return SnapshotSync::Unreachable,
    };
    let Some(result) = response.result else {
        return SnapshotSync::Unchanged;
    };
    if result
        .get("indexing")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !result
            .get("changed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return SnapshotSync::Unchanged;
    }
    let Some(generation) = result.get("generation").and_then(Value::as_u64) else {
        return SnapshotSync::Unchanged;
    };
    let Some(items) = result.get("items").and_then(Value::as_array) else {
        return SnapshotSync::Unchanged;
    };
    SnapshotSync::Applied(generation, items_from_values(items))
}

#[cfg(target_os = "windows")]
pub(crate) fn maybe_apply_background_index_refresh(
    service: &Arc<RwLock<CoreService>>,
    state: &mut BackgroundIndexRefresh,
    runtime_config: &Config,
) {
    if let Some(applied) = state.ui_snapshot_applied.clone() {
        if applied.load(Ordering::Acquire) && !state.cache_applied {
            state.cache_applied = true;
            state.indexes_synced = true;
            log_info(&format!(
                "[nex] startup_phase phase=indexing_completed elapsed_ms={} source=nex-indexer",
                state.startup_started_at.elapsed().as_millis()
            ));
        }
        return;
    }
    if state.cache_applied {
        if !state.indexes_synced {
            let svc = service.read().unwrap_or_else(|e| e.into_inner());
            match svc.sync_indexes_from_cache() {
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

            // Heavy cache/index apply runs off the UI thread.
            if !state.apply_started.swap(true, Ordering::AcqRel) {
                let service = Arc::clone(service);
                let apply_done = state.apply_done.clone();
                std::thread::spawn(move || {
                    let svc = service.read().unwrap_or_else(|e| e.into_inner());
                    match svc.reload_cache_from_store() {
                        Ok(cached_items) => {
                            let startup_elapsed_ms = 0usize;
                            let _ = startup_elapsed_ms;
                            log_info(&format!(
                                "[nex] startup_phase phase=cache_applied cached_items={}",
                                cached_items
                            ));
                            if let Err(e) = svc.sync_indexes_from_cache() {
                                log_warn(&format!(
                                    "[nex] background indexing Tantivy sync failed: {e}"
                                ));
                            } else {
                                log_info("[nex] Tantivy search index synced from cache");
                            }
                        }
                        Err(error) => {
                            log_warn(&format!(
                                "[nex] background indexing cache refresh failed: {error}"
                            ));
                        }
                    }
                    apply_done.store(true, Ordering::Release);
                });
            }
        }
        Some(Err(error)) => {
            log_warn(&format!("[nex] {error}"));
            state.apply_done.store(true, Ordering::Release);
        }
        None => {
            log_warn("[nex] background indexing completed without result");
            state.apply_done.store(true, Ordering::Release);
        }
    }

    if state.apply_done.load(Ordering::Acquire) {
        state.cache_applied = true;
        state.indexes_synced = true;

        if state.pending_discovery_reindex {
            log_info(
                "[nex] discovery settings queued during indexing; pending reindex remains scheduled",
            );
            maybe_start_queued_discovery_reindex(service, state, runtime_config);
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn queue_discovery_reindex_after_active_index(state: &mut BackgroundIndexRefresh) {
    if state.ui_snapshot_applied.is_some() {
        // The UI process does not discover; the indexer picks up root changes.
        return;
    }
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
    service: &Arc<RwLock<CoreService>>,
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
        service.read().unwrap_or_else(|e| e.into_inner()).cached_items_len()
    ));
    *state = start_background_index_refresh(service, runtime_config, false, startup_started_at);
}

#[cfg(target_os = "windows")]
pub(crate) fn config_file_modified_time(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(target_os = "windows")]
pub(crate) fn maybe_apply_runtime_config_reload(
    overlay: &NativeOverlayShell,
    service: &CoreService,
    service_arc: &Arc<RwLock<CoreService>>,
    runtime_config: &mut Config,
    plugin_registry: &mut PluginRegistry,
    search_session: &mut OverlaySearchSession,
    pending_uninstall_confirmation: &mut Option<PendingConfirmation>,
    max_results: &mut usize,
    watcher: &mut RuntimeConfigWatcher,
    background_index_refresh: &mut BackgroundIndexRefresh,
) -> bool {
    if watcher.last_checked.elapsed() < CONFIG_RELOAD_POLL_INTERVAL {
        return false;
    }
    watcher.last_checked = Instant::now();
    let modified = config_file_modified_time(watcher.path.as_path());
    if modified == watcher.last_modified {
        return false;
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
            overlay.set_grid_view(runtime_config.grid_view);
            overlay.set_compact_view(!runtime_config.view_mode.shows_idle_feed());
            overlay.set_panel_background(runtime_config.panel_background);
            *plugin_registry = PluginRegistry::load_from_config(runtime_config);
            for warning in &plugin_registry.load_warnings {
                log_warn(&format!("[nex] plugin_warning {warning}"));
            }
            search_session.clear();
            *pending_uninstall_confirmation = None;

            if hotkey_changed {
                log_info(&format!(
                    "[nex] config hotkey changed ({} -> {}), re-registering listener",
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
                } else if background_index_refresh.ui_snapshot_applied.is_some() {
                    log_info(
                        "[nex] discovery settings changed; NexIndexerProcess reloads them",
                    );
                } else if background_index_refresh.cache_applied {
                    *background_index_refresh = start_background_index_refresh(
                        service_arc,
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

            // Re-read mtime after config::load to absorb any self-writes
            // (migration, template generation) so the next poll doesn't
            // falsely detect a change.
            watcher.last_modified = config_file_modified_time(watcher.path.as_path());

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
                overlay.set_status_text("Hotkey updated");
            } else {
                // Generic apply: don't leave a lingering "Settings applied"
                // status that resurfaces on the next show.
                overlay.set_status_text("");
            }
            return hotkey_changed;
        }
        Err(error) => {
            log_warn(&format!(
                "[nex] config reload skipped due to invalid config: {error}"
            ));
            return false;
        }
        }
    }
