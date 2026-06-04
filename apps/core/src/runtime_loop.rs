#[cfg(target_os = "windows")]
use std::cell::RefCell;
#[cfg(target_os = "windows")]
use std::rc::Rc;
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "windows")]
use std::time::Instant;

use crate::clipboard_history;
#[cfg(target_os = "windows")]
use crate::config::Config;
#[cfg(target_os = "windows")]
use crate::config::DiscoveryBackend;
#[cfg(target_os = "windows")]
use crate::core_service::CoreService;
#[cfg(target_os = "windows")]
use crate::everything_bridge::EverythingBridge;
#[cfg(target_os = "windows")]
use crate::overlay_state::{HotkeyAction, OverlayState};
#[cfg(target_os = "windows")]
use crate::plugin_sdk::PluginRegistry;
#[cfg(target_os = "windows")]
use crate::query_dsl::ParsedQuery;
#[cfg(target_os = "windows")]
use crate::runtime::{log_info, log_warn, RuntimeError};
#[cfg(target_os = "windows")]
use crate::runtime_actions::{
    execute_action_selection, launch_overlay_selection, should_suppress_failed_uninstall,
    uninstall_confirmation_results,
};
#[cfg(target_os = "windows")]
use crate::runtime_hotkey::{should_suppress_hotkey_for_game_mode, toggle_game_mode_from_tray};
#[cfg(target_os = "windows")]
use crate::runtime_index::{
    config_file_modified_time, maybe_apply_background_index_refresh,
    maybe_apply_runtime_config_reload, start_background_index_refresh, BackgroundIndexRefresh,
    RuntimeConfigWatcher,
};
#[cfg(target_os = "windows")]
use crate::runtime_overlay_rows::{
    filter_suppressed_uninstall_results, next_selection_index, overlay_rows,
    reconcile_suppressed_uninstall_titles, set_idle_overlay_state, set_status_row_overlay_state,
    track_uninstall_title_suppression, PendingUninstallConfirmation, ACTION_UNINSTALL_CANCEL_ID,
    ACTION_UNINSTALL_CONFIRM_ID, STATUS_ROW_NO_COMMAND_RESULTS, STATUS_ROW_NO_RESULTS,
    STATUS_ROW_TYPE_TO_SEARCH,
};
#[cfg(target_os = "windows")]
use crate::runtime_process::{
    acquire_single_instance_guard, hotkey_registration_recovery_message,
    hotkey_registration_status_text, launch_stable_updater,
};
#[cfg(target_os = "windows")]
use crate::runtime_search_session::{
    maybe_expand_uninstall_quick_shortcut, result_limit_for_query, OverlaySearchSession,
};
#[cfg(target_os = "windows")]
use crate::search_worker::SearchWorker;

#[cfg(target_os = "windows")]
use crate::overlay::boot::Boot;
#[cfg(target_os = "windows")]
use crate::overlay::hotkey::HotkeyListener;
#[cfg(target_os = "windows")]
use crate::overlay::indexing_progress::run_with_progress_window;
#[cfg(target_os = "windows")]
use crate::overlay::NEX_WM_SEARCH_RESULTS_READY;
#[cfg(target_os = "windows")]
use crate::overlay::{
    signal_existing_instance_show, NativeOverlayShell, OverlayEvent, OverlayRow, OverlayRowRole,
};

#[cfg(target_os = "windows")]
pub(crate) fn run_windows_runtime(
    startup_started_at: Instant,
    mut runtime_config: Config,
    service: CoreService,
) -> Result<(), RuntimeError> {
    let service = Arc::new(Mutex::new(service));

    let initial_cache_empty = {
        let guard = service.lock().unwrap();
        guard.cached_items_len() == 0
    };

    let mut background_index_refresh = if initial_cache_empty {
        let use_progress_window = match runtime_config.file_discovery_backend {
            DiscoveryBackend::Everything => false,
            DiscoveryBackend::Walkdir => true,
            DiscoveryBackend::Auto => match EverythingBridge::detect() {
                Some(bridge) if bridge.is_service_running() => false,
                _ => true,
            },
        };
        if use_progress_window {
            log_info("[nex] startup cached_items=0 (first-time indexing with progress window)");
            let service_arc = service.clone();
            let result = run_with_progress_window(move |pct| {
                let svc = service_arc.lock().unwrap();
                *svc.progress.lock().unwrap() = Some(pct);
                let report = svc.rebuild_index_incremental_with_report();
                *svc.progress.lock().unwrap() = None;
                report
            });
            match result {
                Ok(report) => {
                    log_info(&format!(
                        "[nex] startup indexed_items={} discovered={} upserted={} removed={}",
                        report.indexed_total,
                        report.discovered_total,
                        report.upserted_total,
                        report.removed_total,
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
                    BackgroundIndexRefresh {
                        completed: Arc::new(AtomicBool::new(true)),
                        result: Arc::new(Mutex::new(Some(Ok(report)))),
                        cache_applied: true,
                        indexes_synced: false,
                        initial_cache_empty: true,
                        pending_discovery_reindex: false,
                        pending_discovery_reindex_due_at: None,
                        pending_discovery_reindex_requests: 0,
                        started_at: Instant::now(),
                        startup_started_at,
                    }
                }
                Err(e) => {
                    log_warn(&format!("[nex] first-time indexing failed: {e}"));
                    start_background_index_refresh(&runtime_config, true, startup_started_at)
                }
            }
        } else {
            log_info("[nex] startup cached_items=0 (first-time indexing, Everything backend — async, no progress window)");
            log_info(&format!(
                "[nex] startup_phase phase=indexing_started elapsed_ms={} initial_cache_empty=true cached_items=0",
                startup_started_at.elapsed().as_millis()
            ));
            start_background_index_refresh(&runtime_config, true, startup_started_at)
        }
    } else {
        log_info(&format!(
            "[nex] startup cached_items={} (async indexing scheduled)",
            {
                let guard = service.lock().unwrap();
                guard.cached_items_len()
            }
        ));
        start_background_index_refresh(&runtime_config, false, startup_started_at)
    };

    let mut plugin_registry = PluginRegistry::load_from_config(&runtime_config);
    for warning in &plugin_registry.load_warnings {
        log_warn(&format!("[nex] plugin_warning {warning}"));
    }
    log_info(&format!(
        "[nex] plugins loaded provider_items={} action_items={}",
        plugin_registry.provider_items.len(),
        plugin_registry.action_items.len()
    ));

    unsafe {
        let _ = windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Err(error) = crate::startup::set_enabled(runtime_config.launch_at_startup, &exe) {
            log_warn(&format!("[nex] startup sync warning: {error}"));
        }
    }

    let _single_instance = match acquire_single_instance_guard() {
        Ok(guard) => guard,
        Err(error) => return Err(RuntimeError::Overlay(error)),
    };
    if _single_instance.is_none() {
        let _ = signal_existing_instance_show();
        log_info("[nex] runtime already active; signaled existing instance");
        return Ok(());
    }

    let mut overlay_state = OverlayState::default();
    let overlay = NativeOverlayShell::create().map_err(RuntimeError::Overlay)?;
    overlay.set_help_config_path(runtime_config.config_path.to_string_lossy().as_ref());
    overlay.set_hotkey_hint(&runtime_config.hotkey);
    overlay.set_performance_tuning(
        runtime_config.idle_cache_trim_ms,
        runtime_config.active_memory_target_mb,
    );
    overlay.set_game_mode_enabled(runtime_config.game_mode_enabled);
    log_info("[nex] native overlay shell initialized (hidden)");
    log_info(&format!(
        "[nex] startup_phase phase=overlay_ready elapsed_ms={}",
        startup_started_at.elapsed().as_millis()
    ));

    // Build the event channel that the Iced `State` and the hotkey
    // listener write to, and the runtime worker thread reads from.
    let (event_tx, event_rx) = crossbeam_channel::unbounded::<OverlayEvent>();

    let search_worker = SearchWorker::new(
        service.clone(),
        runtime_config.clone(),
        Arc::new(plugin_registry.clone()),
        overlay.hwnd(),
        NEX_WM_SEARCH_RESULTS_READY,
    );

    // Register the global hotkey on its own OS thread, sending
    // `OverlayEvent::Hotkey(id)` to the shared event channel.
    let (hotkey_listener, hotkey_issue_status) =
        match HotkeyListener::start(&runtime_config.hotkey, event_tx.clone()) {
            Ok(listener) => {
                log_info(&format!(
                    "[nex] hotkey registered native_id=1 hotkey={}",
                    runtime_config.hotkey
                ));
                overlay.set_hotkey_issue_active(false);
                (Some(listener), None)
            }
            Err(error) => {
                let recovery_message = hotkey_registration_recovery_message(
                    &runtime_config.hotkey,
                    &runtime_config.config_path,
                );
                let suggested = crate::settings::suggested_hotkey_presets(
                    &runtime_config.hotkey,
                    3,
                )
                .join("|");
                log_warn(&format!(
                    "[nex] hotkey_registration_issue hotkey={} suggestions={} error={:?}",
                    runtime_config.hotkey, suggested, error
                ));
                log_warn(&format!("[nex] {recovery_message}"));
                overlay.set_hotkey_issue_active(true);
                let status = hotkey_registration_status_text(&runtime_config.hotkey);
                overlay.set_status_text(&status);
                (None, Some(status))
            }
        };
    log_info(&format!(
        "[nex] startup_phase phase=hotkey_ready elapsed_ms={} hotkey={}",
        startup_started_at.elapsed().as_millis(),
        runtime_config.hotkey
    ));
    log_info("[nex] event loop running (native overlay)");

    let max_results = runtime_config.max_results as usize;
    let config_watcher = RuntimeConfigWatcher {
        path: runtime_config.config_path.clone(),
        last_checked: Instant::now(),
        last_modified: config_file_modified_time(runtime_config.config_path.as_path()),
    };

    // Build the Iced `Boot`. The Iced event loop runs on the main
    // thread (winit 0.30 requires `EventLoop::new` on the main
    // thread), reading from the shared model and writing events
    // to the `event_tx` channel.
    let shared_model = overlay.shared_model();
    let is_running = overlay.is_running();
    is_running.store(true, std::sync::atomic::Ordering::SeqCst);
    let boot = Boot {
        model: shared_model,
        event_tx: event_tx.clone(),
        is_running: is_running.clone(),
    };

    // Bundle the runtime's mutable state into a struct that the
    // worker thread owns. The worker thread runs the message pump
    // loop and calls `on_event` for every event from the channel.
    let worker = RuntimeWorker {
        overlay: overlay.clone(),
        service: service.clone(),
        runtime_config,
        background_index_refresh,
        plugin_registry,
        search_worker,
        overlay_state,
        max_results,
        config_watcher,
        current_results: Vec::new(),
        suppressed_uninstall_titles: Vec::new(),
        pending_uninstall_confirmation: None,
        selected_index: 0,
        last_query: String::new(),
        last_sent_generation: 0,
        search_session: OverlaySearchSession::default(),
        hotkey_issue_status,
        event_rx,
        is_running,
    };

    let worker_join = std::thread::Builder::new()
        .name("nex-runtime".to_string())
        .spawn(move || worker.run())
        .map_err(|e| RuntimeError::Overlay(format!("failed to spawn runtime thread: {e}")))?;

    // Run the Iced event loop on the main thread (blocking). This
    // is the only place `iced::application().run()` can be called:
    // winit 0.30 panics if the EventLoop is created on a non-main
    // thread.
    let iced_result = crate::overlay::boot::run(boot);

    // When the Iced event loop exits, `boot::run` already set
    // `is_running = false`, so the worker's `run_message_pump`
    // loop will exit on the next 50ms tick. Drop the hotkey
    // listener (unregisters the hotkey) and wait for the worker
    // to finish.
    drop(hotkey_listener);
    let _ = worker_join.join();

    iced_result.map_err(RuntimeError::Overlay)
}

/// All mutable state owned by the runtime worker thread. The
/// `on_event` method is the body of the legacy Win32 message-pump
/// callback, refactored from a closure into a method on a struct
/// so it can be called from the worker thread.
struct RuntimeWorker {
    overlay: NativeOverlayShell,
    service: Arc<Mutex<CoreService>>,
    runtime_config: Config,
    background_index_refresh: BackgroundIndexRefresh,
    plugin_registry: PluginRegistry,
    search_worker: SearchWorker,
    overlay_state: OverlayState,
    max_results: usize,
    config_watcher: RuntimeConfigWatcher,
    current_results: Vec<crate::model::SearchItem>,
    suppressed_uninstall_titles: Vec<String>,
    pending_uninstall_confirmation: Option<PendingUninstallConfirmation>,
    selected_index: usize,
    last_query: String,
    last_sent_generation: u64,
    search_session: OverlaySearchSession,
    hotkey_issue_status: Option<String>,
    event_rx: crossbeam_channel::Receiver<OverlayEvent>,
    is_running: Arc<AtomicBool>,
}

impl RuntimeWorker {
    fn run(self) {
        // Share `self` with the closure via `Rc<RefCell<>>`. The
        // worker thread is single-threaded, so `RefCell::borrow_mut`
        // never panics from contention (only from aliasing, which
        // the closure doesn't do).
        let shared: Rc<RefCell<Self>> = Rc::new(RefCell::new(self));
        let shared_for_closure = shared.clone();
        let (overlay, event_rx, is_running) = {
            let guard = shared.borrow();
            (
                guard.overlay.clone(),
                guard.event_rx.clone(),
                guard.is_running.clone(),
            )
        };
        let _ = overlay.run_message_pump(&event_rx, &is_running, move |event| {
            let _ = shared_for_closure.borrow_mut().on_event(event);
        });
    }

    fn on_event(&mut self, event: OverlayEvent) {
        // The message-pump callback must never block on the service
        // lock. The service is shared with the background indexer
        // thread and the per-root directory-watcher consumer threads,
        // and any blocking wait here would prevent the message pump
        // from delivering WM_HOTKEY and other input. We use `try_lock`
        // and skip the tick if a worker is currently holding the
        // service; the work is re-attempted on the next event.
        let was_indexing_complete = self
            .background_index_refresh
            .completed
            .load(std::sync::atomic::Ordering::Acquire);

        if let Ok(svc) = self.service.try_lock() {
            maybe_apply_runtime_config_reload(
                &self.overlay,
                &*svc,
                &mut self.runtime_config,
                &mut self.plugin_registry,
                &mut self.search_session,
                &mut self.pending_uninstall_confirmation,
                &mut self.max_results,
                &mut self.config_watcher,
                &mut self.background_index_refresh,
            );
            maybe_apply_background_index_refresh(
                &*svc,
                &mut self.background_index_refresh,
                &self.runtime_config,
            );

            // Start per-root file watchers the first time the index
            // cache becomes usable. The handle is idempotent: it is a
            // no-op if a watcher is already running.
            if self.background_index_refresh.cache_applied {
                if let Err(error) = svc.start_file_watchers(&self.service) {
                    log_warn(&format!("[nex] directory_watcher start failed: {error}"));
                }
            }
        } else {
            // Service lock is held by a worker. The config reloader,
            // index-refresh applicator, and watcher starter will
            // retry on the next event; the cost of skipping a tick
            // is bounded by the worker's per-item critical section.
        }

        if !was_indexing_complete
            && self
                .background_index_refresh
                .completed
                .load(std::sync::atomic::Ordering::Acquire)
            && !self.last_query.is_empty()
        {
            let pending_query = self.last_query.clone();
            self.last_query.clear();
            apply_query_change(
                pending_query,
                &self.overlay,
                &self.search_worker,
                &self.runtime_config,
                self.max_results,
                &mut self.pending_uninstall_confirmation,
                &mut self.current_results,
                &mut self.selected_index,
                &mut self.last_query,
                &mut self.last_sent_generation,
            );
        }
        match event {
            OverlayEvent::Hotkey(_) => {
                log_info("[nex] hotkey_event received");
                let overlay_visible = self.overlay.is_visible();
                self.overlay_state.set_visible(overlay_visible);
                if self.overlay.query_text().trim().starts_with('=') {
                    let query = self.overlay.query_text();
                    if let Some(expr) = query.trim().strip_prefix('=') {
                        let expr = expr.trim();
                        if let Ok(value) = crate::calculator::evaluate(expr) {
                            let display = format_result(value);
                            let status_text = if copy_to_clipboard(&display) {
                                format!("Copied: {display}")
                            } else {
                                format!("= {display}")
                            };
                            self.overlay.set_status_text(&status_text);
                        }
                    }
                    return;
                }
                if !overlay_visible
                    && should_suppress_hotkey_for_game_mode(&self.runtime_config)
                {
                    log_info(
                        "[nex] hotkey ignored because game mode is active for the foreground app",
                    );
                    return;
                }
                let action = self.overlay_state.on_hotkey(self.overlay.has_focus());
                match action {
                    HotkeyAction::ShowAndFocus | HotkeyAction::FocusExisting => {
                        reconcile_suppressed_uninstall_titles(
                            &mut self.suppressed_uninstall_titles,
                        );
                        self.overlay.show_and_focus();
                        if self.runtime_config.clipboard_enabled {
                            let _ = clipboard_history::maybe_capture_latest(&self.runtime_config);
                        }
                        if self.overlay.query_text().trim().is_empty() {
                            set_idle_overlay_state(&self.overlay);
                            if let Some(issue) = self.hotkey_issue_status.as_deref() {
                                self.overlay.set_status_text(issue);
                            }
                        }
                    }
                    HotkeyAction::Hide => {
                        self.overlay.hide();
                        reset_overlay_session(
                            &self.overlay,
                            &mut self.current_results,
                            &mut self.selected_index,
                        );
                        self.pending_uninstall_confirmation = None;
                        self.last_query.clear();
                        self.last_sent_generation = 0;
                        self.search_session.clear();
                        self.search_worker.clear_session();
                        maybe_apply_background_index_refresh(
                            &*self.service.lock().unwrap(),
                            &mut self.background_index_refresh,
                            &self.runtime_config,
                        );
                    }
                }
            }
            OverlayEvent::ExternalShow => {
                self.overlay_state.set_visible(self.overlay.is_visible());
                reconcile_suppressed_uninstall_titles(&mut self.suppressed_uninstall_titles);
                self.overlay.show_and_focus();
                if self.runtime_config.clipboard_enabled {
                    let _ = clipboard_history::maybe_capture_latest(&self.runtime_config);
                }
                if self.overlay.query_text().trim().is_empty() {
                    set_idle_overlay_state(&self.overlay);
                    if let Some(issue) = self.hotkey_issue_status.as_deref() {
                        self.overlay.set_status_text(issue);
                    }
                }
            }
            OverlayEvent::ExternalQuit => {
                self.overlay.hide_now();
                self.last_query.clear();
                self.last_sent_generation = 0;
                self.search_session.clear();
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                }
            }
            OverlayEvent::TrayToggleGameMode => {
                toggle_game_mode_from_tray(&self.overlay, &mut self.runtime_config);
            }
            OverlayEvent::TrayCheckForUpdates => {
                match launch_stable_updater() {
                    Ok(_) => self.overlay.set_status_text("Updater launched"),
                    Err(error) => {
                        log_warn(&format!("[nex] updater launch failed from tray: {error}"));
                        self.overlay.set_status_text("Could not launch updater");
                    }
                }
            }
            OverlayEvent::Escape => {
                if self.overlay_state.on_escape() {
                    self.overlay.hide_now();
                    reset_overlay_session(
                        &self.overlay,
                        &mut self.current_results,
                        &mut self.selected_index,
                    );
                    self.pending_uninstall_confirmation = None;
                    self.last_query.clear();
                    self.last_sent_generation = 0;
                    self.search_session.clear();
                }
            }
            OverlayEvent::QueryChanged(query) => {
                apply_query_change(
                    query,
                    &self.overlay,
                    &self.search_worker,
                    &self.runtime_config,
                    self.max_results,
                    &mut self.pending_uninstall_confirmation,
                    &mut self.current_results,
                    &mut self.selected_index,
                    &mut self.last_query,
                    &mut self.last_sent_generation,
                );
            }
            OverlayEvent::SearchResultsReady => {
                apply_search_results(
                    &self.search_worker,
                    &self.overlay,
                    &self.runtime_config,
                    &self.background_index_refresh,
                    &self.suppressed_uninstall_titles,
                    &mut self.current_results,
                    &mut self.selected_index,
                    self.last_sent_generation,
                );
            }
            OverlayEvent::MoveSelection(direction) => {
                if self.current_results.is_empty() {
                    return;
                }

                self.selected_index = next_selection_index(
                    self.selected_index,
                    self.current_results.len(),
                    direction,
                );
                self.overlay.set_selected_index(self.selected_index);
            }
            OverlayEvent::Submit => {
                if self.current_results.is_empty() {
                    if self.overlay.query_text().trim().is_empty() {
                        set_idle_overlay_state(&self.overlay);
                        self.overlay.show_placeholder_hint(STATUS_ROW_TYPE_TO_SEARCH);
                    } else {
                        let parsed_query = ParsedQuery::parse(
                            self.overlay.query_text().trim(),
                            self.runtime_config.search_dsl_enabled,
                        );
                        set_status_row_overlay_state(
                            &self.overlay,
                            if parsed_query.command_mode {
                                STATUS_ROW_NO_COMMAND_RESULTS
                            } else {
                                STATUS_ROW_NO_RESULTS
                            },
                        );
                    }
                    return;
                }

                if let Some(list_selection) = self.overlay.selected_index() {
                    self.selected_index = list_selection.min(self.current_results.len() - 1);
                }

                let selected = &self.current_results[self.selected_index];
                if self.pending_uninstall_confirmation.is_some() {
                    let selected_id = selected.id.clone();
                    if selected_id == ACTION_UNINSTALL_CONFIRM_ID {
                        let Some(pending) = self.pending_uninstall_confirmation.take() else {
                            return;
                        };
                        self.overlay.hide_now();
                        self.overlay_state.on_escape();
                        match execute_action_selection(
                            &*self.service.lock().unwrap(),
                            &self.runtime_config,
                            &self.plugin_registry,
                            &pending.uninstall_action,
                        ) {
                            Ok(()) => {
                                track_uninstall_title_suppression(
                                    &mut self.suppressed_uninstall_titles,
                                    pending.uninstall_action.title.as_str(),
                                );
                                self.overlay.set_status_text("");
                                reset_overlay_session(
                                    &self.overlay,
                                    &mut self.current_results,
                                    &mut self.selected_index,
                                );
                                self.last_query.clear();
                                self.last_sent_generation = 0;
                                self.search_session.clear();
                                self.search_worker.clear_session();
                            }
                            Err(error) => {
                                if should_suppress_failed_uninstall(error.as_str()) {
                                    track_uninstall_title_suppression(
                                        &mut self.suppressed_uninstall_titles,
                                        pending.uninstall_action.title.as_str(),
                                    );
                                    self.current_results = pending.previous_results;
                                    filter_suppressed_uninstall_results(
                                        &mut self.current_results,
                                        &self.suppressed_uninstall_titles,
                                    );
                                    self.selected_index = pending
                                        .previous_selected_index
                                        .min(self.current_results.len().saturating_sub(1));
                                    if self.current_results.is_empty() {
                                        set_status_row_overlay_state(
                                            &self.overlay,
                                            if pending.previous_command_mode {
                                                STATUS_ROW_NO_COMMAND_RESULTS
                                            } else {
                                                STATUS_ROW_NO_RESULTS
                                            },
                                        );
                                    } else {
                                        let rows = overlay_rows(
                                            &self.current_results,
                                            pending.previous_command_mode,
                                        );
                                        self.overlay.set_results(&rows, self.selected_index);
                                    }
                                    self.overlay.set_status_text(
                                        "Uninstall entry is stale and was hidden",
                                    );
                                } else {
                                    self.pending_uninstall_confirmation = Some(pending);
                                    self.overlay.show_and_focus();
                                    self.overlay
                                        .set_status_text(&format!("Launch error: {error}"));
                                }
                            }
                        }
                        return;
                    }

                    if selected_id == ACTION_UNINSTALL_CANCEL_ID {
                        let Some(pending) = self.pending_uninstall_confirmation.take() else {
                            return;
                        };
                        self.current_results = pending.previous_results;
                        self.selected_index = pending
                            .previous_selected_index
                            .min(self.current_results.len().saturating_sub(1));
                        if self.current_results.is_empty() {
                            set_status_row_overlay_state(
                                &self.overlay,
                                if pending.previous_command_mode {
                                    STATUS_ROW_NO_COMMAND_RESULTS
                                } else {
                                    STATUS_ROW_NO_RESULTS
                                },
                            );
                        } else {
                            let rows = overlay_rows(
                                &self.current_results,
                                pending.previous_command_mode,
                            );
                            self.overlay.set_results(&rows, self.selected_index);
                        }
                        self.overlay.set_status_text("");
                        return;
                    }

                    self.pending_uninstall_confirmation = None;
                }

                let selected_is_uninstall = selected
                    .id
                    .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX);

                if selected_is_uninstall {
                    let parsed_query = ParsedQuery::parse(
                        self.overlay.query_text().trim(),
                        self.runtime_config.search_dsl_enabled,
                    );
                    self.pending_uninstall_confirmation = Some(PendingUninstallConfirmation {
                        uninstall_action: selected.clone(),
                        previous_results: self.current_results.clone(),
                        previous_selected_index: self.selected_index,
                        previous_command_mode: parsed_query.command_mode,
                    });
                    self.current_results = uninstall_confirmation_results(selected);
                    self.selected_index = 0;
                    let rows = overlay_rows(&self.current_results, true);
                    self.overlay.set_results(&rows, self.selected_index);
                    self.overlay.set_status_text("");
                    return;
                }

                if selected.id == crate::action_registry::ACTION_TRIM_MEMORY_ID {
                    self.search_session.clear();
                    self.overlay.trim_runtime_memory();
                    self.overlay.set_status_text("Memory caches trimmed");
                    return;
                }

                match launch_overlay_selection(
                    &*self.service.lock().unwrap(),
                    &self.runtime_config,
                    &self.plugin_registry,
                    &self.current_results,
                    self.selected_index,
                    self.last_query.as_str(),
                ) {
                    Ok(()) => {
                        self.overlay.set_status_text("");
                        self.overlay.hide_now();
                        self.overlay_state.on_escape();
                        reset_overlay_session(
                            &self.overlay,
                            &mut self.current_results,
                            &mut self.selected_index,
                        );
                        self.pending_uninstall_confirmation = None;
                        self.last_query.clear();
                        self.last_sent_generation = 0;
                        self.search_session.clear();
                        self.search_worker.clear_session();
                    }
                    Err(error) => {
                        self.overlay
                            .set_status_text(&format!("Launch error: {error}"));
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn reset_overlay_session(
    overlay: &NativeOverlayShell,
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
) {
    overlay.clear_query_text();
    current_results.clear();
    *selected_index = 0;
    set_idle_overlay_state(overlay);
}

#[cfg(target_os = "windows")]
fn apply_query_change(
    query: String,
    overlay: &NativeOverlayShell,
    search_worker: &SearchWorker,
    runtime_config: &Config,
    max_results: usize,
    pending_uninstall_confirmation: &mut Option<PendingUninstallConfirmation>,
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
    last_query: &mut String,
    last_sent_generation: &mut u64,
) {
    *pending_uninstall_confirmation = None;
    let mut query = query;
    if let Some(expanded) = maybe_expand_uninstall_quick_shortcut(&query, last_query.as_str()) {
        overlay.set_query_text(&expanded);
        query = expanded;
    }

    let trimmed = query.trim();
    if trimmed.is_empty() {
        current_results.clear();
        *selected_index = 0;
        last_query.clear();
        *last_sent_generation = last_sent_generation.wrapping_add(1);
        *pending_uninstall_confirmation = None;
        set_idle_overlay_state(overlay);
        return;
    }

    // Calculator mode: evaluate expression inline, no search worker dispatch
    if let Some(expr) = trimmed.strip_prefix('=') {
        let expr = expr.trim();
        if !expr.is_empty() {
            *last_query = trimmed.to_string();
            *last_sent_generation = last_sent_generation.wrapping_add(1);
            current_results.clear();
            *selected_index = 0;
            match crate::calculator::evaluate(expr) {
                Ok(value) => {
                    let display = format_result(value);
                    let row = OverlayRow {
                        role: OverlayRowRole::Calculator,
                        result_index: Some(0),
                        kind: "calculator".into(),
                        title: format!("= {expr}"),
                        path: display,
                        icon_path: String::new(),
                    };
                    overlay.set_results(&[row], 0);
                }
                Err(error) => {
                    let row = OverlayRow {
                        role: OverlayRowRole::Status,
                        result_index: None,
                        kind: String::new(),
                        title: format!("{error}"),
                        path: String::new(),
                        icon_path: String::new(),
                    };
                    overlay.set_results(&[row], 0);
                }
            }
        }
        return;
    }

    if trimmed == last_query {
        return;
    }
    *last_query = trimmed.to_string();
    let parsed_query = ParsedQuery::parse(trimmed, runtime_config.search_dsl_enabled);
    let query_result_limit = result_limit_for_query(max_results, &parsed_query);

    let gen = search_worker.send_request(parsed_query, query_result_limit);
    *last_sent_generation = gen;
}

#[cfg(target_os = "windows")]
fn apply_search_results(
    search_worker: &SearchWorker,
    overlay: &NativeOverlayShell,
    _runtime_config: &Config,
    background_index_refresh: &BackgroundIndexRefresh,
    suppressed_uninstall_titles: &[String],
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
    last_sent_generation: u64,
) {
    let Some(result) = search_worker.try_recv() else {
        return;
    };

    if result.generation < last_sent_generation {
        return;
    }

    let command_mode = result.command_mode;

    if let Some(error) = result.error {
        current_results.clear();
        *selected_index = 0;
        overlay.set_results(&[], 0);
        overlay.set_status_text(&format!("Search error: {error}"));
        return;
    }

    let mut results = result.results;
    crate::runtime_overlay_rows::dedupe_overlay_results(&mut results);
    if !suppressed_uninstall_titles.is_empty() {
        filter_suppressed_uninstall_results(&mut results, suppressed_uninstall_titles);
    }
    *current_results = results;
    *selected_index = 0;

    if current_results.is_empty() {
        let indexing_in_progress = !background_index_refresh
            .completed
            .load(std::sync::atomic::Ordering::Acquire);
        let message = if indexing_in_progress && !command_mode {
            "Indexing, please wait..."
        } else if command_mode {
            STATUS_ROW_NO_COMMAND_RESULTS
        } else {
            STATUS_ROW_NO_RESULTS
        };
        set_status_row_overlay_state(overlay, message);
    } else {
        let rows = overlay_rows(current_results, command_mode);
        overlay.set_results(&rows, *selected_index);
    }
}

#[cfg(target_os = "windows")]
fn format_result(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value.is_sign_positive() {
            "∞".into()
        } else {
            "-∞".into()
        }
    } else if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else if value.abs() > 1e10 || value.abs() < 1e-4 {
        format!("{:.6e}", value)
    } else {
        let s = format!("{:.10}", value);
        let trimmed = s.trim_end_matches('0');
        if trimmed.ends_with('.') {
            format!("{}.0", &trimmed[..trimmed.len() - 1])
        } else {
            trimmed.to_string()
        }
    }
}

#[cfg(target_os = "windows")]
fn copy_to_clipboard(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let len_bytes = (wide.len() * 2) as u32;
    unsafe {
        let hglob = windows_sys::Win32::System::Memory::GlobalAlloc(
            windows_sys::Win32::System::Memory::GMEM_MOVEABLE,
            (len_bytes + 2) as usize,
        );
        if hglob.is_null() {
            return false;
        }
        let lock = windows_sys::Win32::System::Memory::GlobalLock(hglob);
        if lock.is_null() {
            windows_sys::Win32::Foundation::GlobalFree(hglob);
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), lock as *mut u16, wide.len());
        windows_sys::Win32::System::Memory::GlobalUnlock(hglob);
        let opened = windows_sys::Win32::System::DataExchange::OpenClipboard(std::ptr::null_mut());
        if opened == 0 {
            windows_sys::Win32::Foundation::GlobalFree(hglob);
            return false;
        }
        windows_sys::Win32::System::DataExchange::EmptyClipboard();
        let result = windows_sys::Win32::System::DataExchange::SetClipboardData(
            13, // CF_UNICODETEXT
            hglob,
        );
        windows_sys::Win32::System::DataExchange::CloseClipboard();
        if result.is_null() {
            windows_sys::Win32::Foundation::GlobalFree(hglob);
            return false;
        }
    }
    true
}
