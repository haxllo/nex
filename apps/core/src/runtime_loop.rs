#[cfg(target_os = "windows")]
use std::cell::RefCell;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
use std::rc::Rc;
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex, RwLock};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

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
use crate::model::SearchItem;
#[cfg(target_os = "windows")]
use crate::runtime_actions::{
    execute_action_selection, launch_overlay_selection, power_confirmation_results,
    should_suppress_failed_uninstall, uninstall_confirmation_results,
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
    filter_suppressed_uninstall_results, overlay_rows, overlay_rows_ext,
    reconcile_suppressed_uninstall_titles, result_row, set_idle_overlay_state,
    set_quick_launch_overlay_state, set_status_row_overlay_state,
    track_uninstall_title_suppression, uninstall_target_title_from_action_title,
    ConfirmationKind, PendingConfirmation,
    ACTION_POWER_CANCEL_ID, ACTION_POWER_CONFIRM_ID,
    ACTION_UNINSTALL_CANCEL_ID, ACTION_UNINSTALL_CONFIRM_ID,
    STATUS_ROW_NO_COMMAND_RESULTS, STATUS_ROW_NO_RESULTS, STATUS_ROW_TYPE_TO_SEARCH,
};
#[cfg(target_os = "windows")]
use crate::runtime_process::{
    acquire_single_instance_guard, hotkey_registration_recovery_message,
    hotkey_registration_status_text,
};
#[cfg(target_os = "windows")]
use crate::runtime_search_session::{
    maybe_expand_uninstall_quick_shortcut, result_limit_for_query, OverlaySearchSession,
};
#[cfg(target_os = "windows")]
use crate::search_worker::SearchWorker;

#[cfg(target_os = "windows")]
use crate::overlay::host::Host;
#[cfg(target_os = "windows")]
use crate::overlay::hotkey::HotkeyListener;
#[cfg(target_os = "windows")]
use crate::overlay::indexing_progress::run_with_progress_window;
#[cfg(target_os = "windows")]
use crate::overlay::{
    signal_existing_instance_show, NativeOverlayShell, OverlayEvent, OverlayRow, OverlayRowRole,
};
#[cfg(target_os = "windows")]
use crate::overlay::tray::TrayIcon;

/// Debounce gate for hotkey events.  Both WH_KEYBOARD_LL (hook) and
/// WM_INPUT (raw input sink) can fire for the same keystroke.  This
/// prevents double-trigger when both detection paths fire within 50ms.
static HOTKEY_DEBOUNCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Gate to prevent overlapping hotkey re-registration attempts.
/// The re-registration thread sets this while it's starting a new listener,
/// and the dead-thread recovery path skips its check while this is true.
static RE_REGISTERING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set to `true` when any quit path is taken (ExternalQuit, TrayQuit,
/// power restart).  Prevents the dead-thread recovery from restarting the
/// hotkey listener during teardown — the restart race is what triggers
/// the tao "cannot move state from Destroyed" panic.
static QUIT_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "windows")]
pub(crate) fn run_windows_runtime(
    startup_started_at: Instant,
    runtime_config: Config,
    service: CoreService,
) -> Result<(), RuntimeError> {
    let service = Arc::new(RwLock::new(service));

    let initial_cache_empty = {
        let guard = service.read().unwrap_or_else(|e| e.into_inner());
        guard.cached_items_len() == 0
    };

    let background_index_refresh = if initial_cache_empty {
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
                let svc = service_arc.write().unwrap_or_else(|e| e.into_inner());
                *svc.progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(pct);
                let report = svc.rebuild_index_incremental_with_report();
                *svc.progress.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
                    // Sync Tantivy immediately so the first
                    // keystroke never hits the CPU-bound cached_items
                    // scan.  The progress window just closed and the
                    // overlay + hotkey haven't been created yet, so
                    // the user perceives no delay.
                    if let Ok(svc) = service.write() {
                        let _ = svc.sync_indexes_from_cache();
                    }
                    BackgroundIndexRefresh {
                        completed: Arc::new(AtomicBool::new(true)),
                        result: Arc::new(Mutex::new(Some(Ok(report)))),
                        cache_applied: true,
                        indexes_synced: true,
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
                let guard = service.read().unwrap_or_else(|e| e.into_inner());
                guard.cached_items_len()
            }
        ));
        start_background_index_refresh(&runtime_config, false, startup_started_at)
    };

    let plugin_registry = PluginRegistry::load_from_config(&runtime_config);
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
        let _ = signal_existing_instance_show(&[]);
        log_info("[nex] runtime already active; signaled existing instance");
        return Ok(());
    }

    let overlay_state = OverlayState::default();
    let overlay = NativeOverlayShell::create().map_err(RuntimeError::Overlay)?;
    overlay.set_hotkey_hint(&runtime_config.hotkey);
    overlay.set_performance_tuning(
        runtime_config.idle_cache_trim_ms,
        runtime_config.active_memory_target_mb,
        runtime_config.ui_warm_release_ms,
    );
    overlay.set_game_mode_enabled(runtime_config.game_mode_enabled);
    overlay.set_grid_view(runtime_config.grid_view);
    log_info("[nex] native overlay shell initialized (hidden)");
    log_info(&format!(
        "[nex] startup_phase phase=overlay_ready elapsed_ms={}",
        startup_started_at.elapsed().as_millis()
    ));

    // Build the event channel that the WebView host's IPC handler
    // (and the hotkey listener / tray) write to, and the runtime
    // worker thread reads from.
    let (event_tx, event_rx) = crossbeam_channel::unbounded::<OverlayEvent>();

    // The power popup is initialized by the overlay host after its own
    // WebView2 build completes (host.rs) — never here. WebView2 env
    // creation is not concurrency-safe across threads sharing the same
    // user-data folder; building the popup's env in parallel with the
    // host's eager overlay webview can hang one and fail the other
    // (E_INVALIDARG), wedging the host loop before it starts dispatching
    // UiCommands. The host serializes the two builds on one thread.

    // Install a console Ctrl+C handler so `nex --foreground` can be
    // stopped from the terminal. The handler sends ExternalQuit down
    // the same channel the tray uses. Only effective when a console
    // is present (AttachConsole succeeded in main.rs).
    crate::console_signal::install(event_tx.clone());

    // Create the system tray icon with context menu. The tray uses
    // the same event channel so menu selections are delivered as
    // OverlayEvent variants to the worker thread.
    let tray_icon = TrayIcon::create(
        event_tx.clone(),
        runtime_config.config_path.to_string_lossy().as_ref(),
    )
    .map_err(|e| RuntimeError::Overlay(format!("tray icon: {e}")))?;
    log_info("[nex] system tray icon created");

    // Channels for updating tray state (game mode, hotkey issue)
    // from the worker thread. A dedicated updater thread owns the
    // tray_icon and applies state changes.
    let (tray_gm_tx, tray_gm_rx) = crossbeam_channel::unbounded::<bool>();
    let (tray_hi_tx, tray_hi_rx) = crossbeam_channel::unbounded::<bool>();
    let _tray_updater = std::thread::Builder::new()
        .name("nex-tray-updater".into())
        .spawn(move || {
            loop {
                crossbeam_channel::select! {
                    recv(tray_gm_rx) -> msg => {
                        match msg {
                            Ok(enabled) => tray_icon.set_game_mode(enabled),
                            Err(_) => break,
                        }
                    }
                    recv(tray_hi_rx) -> msg => {
                        match msg {
                            Ok(active) => tray_icon.set_hotkey_issue(active),
                            Err(_) => break,
                        }
                    }
                }
            }
            // tray_icon is dropped here, cleaning up the tray
        })
        .map_err(|e| RuntimeError::Overlay(format!("tray updater thread: {e}")))?;

    // Register the global hotkey on its own OS thread, sending
    // `OverlayEvent::Hotkey(id)` to the shared event channel.
    let hotkey_listener: Arc<Mutex<Option<HotkeyListener>>> = Arc::new(Mutex::new(None));
    let hotkey_issue_status: Option<String> =
        match HotkeyListener::start(&runtime_config.hotkey, event_tx.clone()) {
            Ok(listener) => {
                log_info(&format!(
                    "[nex] hotkey registered native_id=1 hotkey={} listener_thread_id={}",
                    runtime_config.hotkey,
                    listener
                        .thread_id()
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ));
                overlay.set_hotkey_issue_active(false);
                let _ = tray_hi_tx.send(false);
                *hotkey_listener.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener);
                None
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
                let _ = tray_hi_tx.send(true);
                let status = hotkey_registration_status_text(&runtime_config.hotkey);
                overlay.set_status_text(&status);
                Some(status)
            }
        };

    log_info(&format!(
        "[nex] startup_phase phase=hotkey_ready elapsed_ms={} hotkey={}",
        startup_started_at.elapsed().as_millis(),
        runtime_config.hotkey
    ));
    log_info("[nex] event loop running (native overlay)");

    let shared_config = Arc::new(RwLock::new(runtime_config.clone()));
    let shared_plugin_registry = Arc::new(RwLock::new(plugin_registry.clone()));

    let search_worker = SearchWorker::new(
        service.clone(),
        shared_config.clone(),
        shared_plugin_registry.clone(),
        event_tx.clone(),
    );

    let max_results = runtime_config.max_results as usize;
    let config_watcher = RuntimeConfigWatcher {
        path: runtime_config.config_path.clone(),
        last_checked: Instant::now(),
        last_modified: config_file_modified_time(runtime_config.config_path.as_path()),
    };

    // Build the WebView host. The tao event loop runs on the main
    // thread (it cannot be created off the main thread), reading from
    // the shared state and writing events to the `event_tx` channel.
    let shared_state = overlay.shared_state();
    let proxy_slot = overlay.proxy_slot();
    let is_running = overlay.is_running();
    let icon_cache = overlay.icon_cache();
    is_running.store(true, std::sync::atomic::Ordering::SeqCst);
    let host = Host {
        state: shared_state,
        proxy_slot,
        icon_cache,
        event_tx: event_tx.clone(),
        is_running: is_running.clone(),
    };

    // Diagnostic: if `--test-show` was passed, post a synthetic
    // `OverlayEvent::Hotkey(1)` 2 s after the WebView host event loop
    // is about to start. This exercises the full show/hide path
    // without depending on a physical keyboard, so the build is
    // verifiable in a CI / scripted environment.
    let test_show = matches!(
        std::env::var("NEX_TEST_SHOW").as_deref(),
        Ok("1") | Ok("true")
    );
    if test_show {
        let tx = event_tx.clone();
        std::thread::Builder::new()
            .name("nex-test-show".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let _ = tx.send(OverlayEvent::Hotkey(1));
            })
            .map_err(|e| RuntimeError::Overlay(format!("test-show thread: {e}")))?;
    }

    // Bundle the runtime's mutable state into a struct that the
    // worker thread owns. The worker thread runs the message pump
    // loop and calls `on_event` for every event from the channel.
    let worker = RuntimeWorker {
        overlay: overlay.clone(),
        service: service.clone(),
        runtime_config,
        shared_config,
        shared_plugin_registry,
        background_index_refresh,
        plugin_registry,
        search_worker,
        overlay_state,
        max_results,
        config_watcher,
        current_results: Vec::new(),
        current_rows: Vec::new(),
        suppressed_uninstall_titles: Vec::new(),
        pending_confirmation: None,
        selected_index: 0,
        last_query: String::new(),
        last_sent_generation: 0,
        search_session: OverlaySearchSession::default(),
        config_generation: 0,
        hotkey_issue_status,
        event_rx,
        is_running,
        tray_gm_tx,
        tray_hi_tx,
        hotkey_listener: hotkey_listener.clone(),
        event_tx: event_tx.clone(),
        hotkey_check_counter: 0,
        last_memory_log: Instant::now(),
        quick_launch_items: Vec::new(),
        quick_launch_loaded: false,
        apps_expanded: None,
        settings_open: false,
        pending_show_all: None,

    };

    let worker_overlay_for_panic = overlay.clone();
    let worker_join = std::thread::Builder::new()
        .name("nex-runtime".to_string())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker.run()
            }));
            match result {
                Ok(()) => {
                    log_info("[nex] runtime worker exited cleanly");
                }
                Err(payload) => {
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "(unknown panic payload)".to_string()
                    };
                    log_warn(&format!("[nex] runtime worker PANICKED: {msg}"));
                    // Signal the overlay to stop so the host event loop
                    // exits instead of hanging forever with no events.
                    worker_overlay_for_panic.stop();
                }
            }
        })
        .map_err(|e| RuntimeError::Overlay(format!("failed to spawn runtime thread: {e}")))?;

    // Run the WebView host event loop on the main thread (blocking).
    // tao, like winit, panics if the EventLoop is created on a
    // non-main thread. The host owns the tao window + wry WebView and
    // drives show/hide/warm-release from `UiCommand`s posted by the
    // shim. The runtime worker thread (above) drains `event_rx` and
    // calls `on_event` for each `OverlayEvent`; the host flips
    // `is_running` to `false` when its event loop exits.
    log_info("[nex] shutdown: host event loop returned");
    let host_result = crate::overlay::host::run(host);

    // Clear the console handler's sender so a late Ctrl+C does not
    // deliver ExternalQuit into a channel nobody reads anymore.
    crate::console_signal::clear();

    // Stop background threads that hold Arc<RwLock<CoreService>>
    // so they don't delay service drop on shutdown.
    if let Ok(guard) = service.read() {
        guard.stop_stale_pruner();
    }
    // Take the file watcher handle without holding the service read
    // lock. The consumer thread may be blocked on service.write(),
    // so we must not hold the RwLock guard while joining — deadlock.
    #[cfg(target_os = "windows")]
    let _watcher_handle = service.read().ok().and_then(|g| g.take_file_watchers());
    // _watcher_handle is dropped here (joins watcher threads) after
    // the RwLock read guard has been released.

    // Signal the worker thread to stop immediately instead of waiting
    // for the next recv tick (removes up to 50 ms jitter on shutdown).
    log_info("[nex] shutdown: stopping worker message pump");
    overlay.stop();

    // Drop the hotkey listener (unregisters the global hotkey) and
    // wait for the worker thread to finish its `run_message_pump`.
    log_info("[nex] shutdown: dropping hotkey listener");
    drop(hotkey_listener.lock().unwrap_or_else(|e| e.into_inner()).take());
    log_info("[nex] shutdown: joining worker thread");
    let _ = worker_join.join();
    log_info("[nex] shutdown: complete, process exiting");

    host_result.map_err(RuntimeError::Overlay)
}

/// All mutable state owned by the runtime worker thread. The
/// `on_event` method is the body of the legacy Win32 message-pump
/// callback, refactored from a closure into a method on a struct
/// so it can be called from the worker thread.
struct RuntimeWorker {
    overlay: NativeOverlayShell,
    service: Arc<RwLock<CoreService>>,
    runtime_config: Config,
    shared_config: Arc<RwLock<Config>>,
    shared_plugin_registry: Arc<RwLock<PluginRegistry>>,
    /// Monotonically increasing counter bumped on each config reload so
    /// subsystems (search worker, overlay, action executor) can detect
    /// that they should invalidate cached state derived from the old config.
    config_generation: u64,
    background_index_refresh: BackgroundIndexRefresh,
    plugin_registry: PluginRegistry,
    search_worker: SearchWorker,
    overlay_state: OverlayState,
    max_results: usize,
    config_watcher: RuntimeConfigWatcher,
    current_results: Vec<crate::model::SearchItem>,
    current_rows: Vec<crate::overlay::OverlayRow>,
    suppressed_uninstall_titles: Vec<String>,
    pending_confirmation: Option<PendingConfirmation>,
    selected_index: usize,
    last_query: String,
    last_sent_generation: u64,
    search_session: OverlaySearchSession,
    hotkey_issue_status: Option<String>,
    event_rx: crossbeam_channel::Receiver<OverlayEvent>,
    is_running: Arc<AtomicBool>,
    tray_gm_tx: crossbeam_channel::Sender<bool>,
    tray_hi_tx: crossbeam_channel::Sender<bool>,
    hotkey_listener: Arc<Mutex<Option<HotkeyListener>>>,
    event_tx: crossbeam_channel::Sender<OverlayEvent>,
    hotkey_check_counter: u32,
    last_memory_log: Instant,
    /// Quick Launch items for idle state display.
    quick_launch_items: Vec<crate::overlay::model::QuickLaunchItem>,
    /// Whether Quick Launch items have been loaded for current session.
    quick_launch_loaded: bool,
    /// Query for which the "Show all apps" expansion is active. When set,
    /// the entry is suppressed in rebuilt rows until the query changes.
    apps_expanded: Option<String>,
    /// Full expanded app list waiting to be pushed after the first page
    /// has painted (see `expand_all_apps`).
    pending_show_all: Option<Vec<crate::model::SearchItem>>,
    /// Set while the settings window is open — hotkey toggles are ignored.
    settings_open: bool,
}

impl RuntimeWorker {
    /// Activate the "Show all apps" entry — synchronous, two-phase, and
    /// order-stable: the currently visible app rows are kept exactly as
    /// rendered (no regroup, no TopHit promotion), index apps are appended
    /// after them. Phase 1 pushes enough rows to fill the window; phase 2
    /// (`ShowAllAppsFillRest`) appends the remainder once the window is
    /// already at its capped height — no resize, no flash.
    fn expand_all_apps(&mut self) {
        const FIRST_PAGE: usize = 60;
        let query = self.overlay.query_text().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.apps_expanded = Some(query);

        // Existing app rows, verbatim — positions never move.
        let mut rows: Vec<crate::overlay::OverlayRow> = self
            .current_rows
            .iter()
            .filter(|r| {
                (r.role == OverlayRowRole::Item || r.role == OverlayRowRole::TopHit)
                    && r.kind.eq_ignore_ascii_case("app")
                    && r.result_index.is_some()
            })
            .cloned()
            .collect();
        let mut results: Vec<crate::model::SearchItem> = rows
            .iter()
            .map(|r| {
                let idx = r.result_index.unwrap_or(0);
                self.current_results[idx].clone()
            })
            .collect();

        // Paths already on screen (any kind) — never duplicated.
        let mut known: std::collections::HashSet<String> = self
            .current_results
            .iter()
            .map(|r| r.path.replace('/', "\\").to_ascii_lowercase())
            .collect();

        let all_apps = {
            let guard = self.service.read().unwrap_or_else(|e| e.into_inner());
            match crate::index_store::get_all_apps(&guard.db_ref()) {
                Ok(apps) => apps,
                Err(error) => {
                    log_warn(&format!("[nex] show-all-apps index read failed: {error}"));
                    return;
                }
            }
        };

        // First page: fill up to FIRST_PAGE total rows now; queue the rest.
        let mut pending: Vec<crate::model::SearchItem> = Vec::new();
        for (id, title, path, subtitle) in all_apps {
            let key = path.replace('/', "\\").to_ascii_lowercase();
            if !known.insert(key) {
                continue;
            }
            let item = crate::model::SearchItem::new(&id, "app", &title, &path)
                .with_subtitle(&subtitle);
            if results.len() < FIRST_PAGE {
                results.push(item.clone());
                let idx = results.len() - 1;
                rows.push(result_row(&item, idx, OverlayRowRole::Item, false));
            } else {
                pending.push(item);
            }
        }

        self.current_results = results;
        self.current_rows = rows;
        self.selected_index = 0;
        self.overlay.set_results(&self.current_rows, self.selected_index);

        if !pending.is_empty() {
            self.pending_show_all = Some(pending);
            let event_tx = self.event_tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(80));
                let _ = event_tx.send(OverlayEvent::ShowAllAppsFillRest);
            });
        }
    }

    /// Load Quick Launch items from the database and config.
    /// Called every time the overlay shows idle state to ensure fresh data.
    fn load_quick_launch_items(&mut self) {
        if !self.runtime_config.quick_launch.enabled {
            self.quick_launch_items.clear();
            self.quick_launch_loaded = true;
            return;
        }

        let max_items = self.runtime_config.quick_launch.max_items as usize;
        let pinned = &self.runtime_config.quick_launch.pinned;
        log_info(&format!("[nex] quick_launch loading pinned={:?}", pinned));

        // Query the database for Quick Launch items. Use try_read so the
        // worker never blocks behind the background indexer; on contention
        // the load is retried by the tick when the lock frees up.
        if let Ok(guard) = self.service.try_read() {
            let db = guard.db_ref();
            match crate::index_store::get_quick_launch_items(&db, pinned, max_items, self.runtime_config.quick_launch.auto_fill) {
                Ok(items) => {
                    self.quick_launch_items = items
                        .into_iter()
                        .map(|(id, _kind, title, path, subtitle, icon_path, is_pinned)| {
                            crate::overlay::model::QuickLaunchItem {
                                title,
                                path,
                                subtitle,
                                icon_path,
                                is_pinned,
                            }
                        })
                        .collect();
                    self.quick_launch_loaded = true;
                    self.overlay.set_quick_launch_items(self.quick_launch_items.clone());
                    log_info(&format!(
                        "[nex] quick_launch loaded items={}",
                        self.quick_launch_items.len()
                    ));
                }
                Err(error) => {
                    log_warn(&format!("[nex] quick_launch load failed: {error}"));
                    self.quick_launch_loaded = true;
                }
            }
        } else {
            // Indexer busy — leave quick_launch_loaded false; the tick
            // retries the load once the service lock is free.
            log_info("[nex] quick_launch load deferred (index busy)");
        }
    }

    /// Load Quick Launch items from in-memory config (no disk read).
    /// Used after pin/unpin to avoid race with config reloader.
    fn load_quick_launch_items_from_config(&mut self) {
        if !self.runtime_config.quick_launch.enabled {
            self.quick_launch_items.clear();
            return;
        }

        let max_items = self.runtime_config.quick_launch.max_items as usize;
        // Use the in-memory pinned list (already updated)
        let pinned = self.runtime_config.quick_launch.pinned.clone();

        // Query the database for Quick Launch items
        if let Ok(guard) = self.service.try_read() {
            let db = guard.db_ref();
            match crate::index_store::get_quick_launch_items(&db, &pinned, max_items, self.runtime_config.quick_launch.auto_fill) {
                Ok(items) => {
                    self.quick_launch_items = items
                        .into_iter()
                        .map(|(id, _kind, title, path, subtitle, icon_path, is_pinned)| {
                            crate::overlay::model::QuickLaunchItem {
                                title,
                                path,
                                subtitle,
                                icon_path,
                                is_pinned,
                            }
                        })
                        .collect();
                    self.overlay.set_quick_launch_items(self.quick_launch_items.clone());
                    log_info(&format!(
                        "[nex] quick_launch reloaded items={} pinned={}",
                        self.quick_launch_items.len(),
                        pinned.len()
                    ));
                }
                Err(error) => {
                    log_warn(&format!("[nex] quick_launch reload failed: {error}"));
                }
            }
        }
    }

    /// Show Quick Launch items in idle state if available.
    fn show_idle_or_quick_launch(&mut self) {
        if self.runtime_config.quick_launch.enabled
            && !self.quick_launch_items.is_empty()
        {
            crate::runtime_overlay_rows::set_quick_launch_overlay_state(
                &self.overlay,
                &self.quick_launch_items,
            );
        } else {
            set_idle_overlay_state(&self.overlay);
        }
        if let Some(issue) = self.hotkey_issue_status.as_deref() {
            self.overlay.set_status_text(issue);
        }
    }

    /// Pin an app to Quick Launch by title.
    fn pin_app_to_quick_launch(&mut self, title: &str) {
        // Find the app path from search results or Quick Launch items
        let app_path = self.current_results.iter()
            .find(|item| item.title.eq_ignore_ascii_case(title) && item.kind.eq_ignore_ascii_case("app"))
            .map(|item| item.path.clone())
            .or_else(|| {
                self.quick_launch_items.iter()
                    .find(|item| item.title.eq_ignore_ascii_case(title))
                    .map(|item| item.path.clone())
            });

        let Some(path) = app_path else {
            log_warn(&format!("[nex] quick_launch pin failed: app '{}' not found", title));
            return;
        };

        // Normalize the path for comparison
        let normalized = path.replace('/', "\\").to_ascii_lowercase();

        // Add to config pinned list if not already there
        let already_pinned = self.runtime_config.quick_launch.pinned.iter().any(|p| {
            p.replace('/', "\\").to_ascii_lowercase() == normalized
        });

        if !already_pinned {
            self.runtime_config.quick_launch.pinned.push(path);

            // Persist to config file and prevent reloader from overwriting
            if let Err(error) = self.save_config_and_prevent_reload() {
                log_warn(&format!("[nex] quick_launch pin save failed: {error}"));
            } else {
                log_info(&format!("[nex] quick_launch pinned '{}'", title));
                log_info(&format!("[nex] quick_launch pinned_list={:?}", self.runtime_config.quick_launch.pinned));
                // Reload Quick Launch items from in-memory config FIRST
                self.load_quick_launch_items_from_config();
                // If in Quick Launch mode (empty query), rebuild the rows
                if self.overlay.query_text().trim().is_empty() {
                    self.show_idle_or_quick_launch();
                } else {
                    // In search mode — just push state (includes updated quickLaunch array for pin icons)
                    self.overlay.set_status_text_and_refresh(&format!("Pinned '{}' to Quick Launch", title));
                }
            }
        }
    }

    /// Unpin an app from Quick Launch by title.
    fn unpin_app_from_quick_launch(&mut self, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }

        // First, try to find by title in quick_launch_items (idle state).
        let app_path = self.quick_launch_items.iter()
            .find(|item| item.title.eq_ignore_ascii_case(title) && item.is_pinned)
            .map(|item| item.path.clone());

        // If not found, try to find by title in current_results (search mode).
        let path_to_remove = if let Some(path) = app_path {
            path
        } else {
            // Also try matching directly against entries in the config pinned list
            let from_config = self.runtime_config.quick_launch.pinned.iter()
                .find(|p| p.eq_ignore_ascii_case(title))
                .cloned();
            if let Some(path) = from_config {
                path
            } else {
                let from_results = self.current_results.iter()
                    .find(|item| item.title.eq_ignore_ascii_case(title))
                    .map(|item| item.path.clone());
                match from_results {
                    Some(path) => path,
                    None => {
                        log_warn(&format!("[nex] quick_launch unpin failed: app '{}' not found in results", title));
                        return;
                    }
                }
            }
        };

        // Normalize the path for comparison
        let normalized = path_to_remove.replace('/', "\\").to_ascii_lowercase();

        // Remove from config pinned list
        self.runtime_config.quick_launch.pinned.retain(|p| {
            p.replace('/', "\\").to_ascii_lowercase() != normalized
        });

        // Persist to config file and prevent reloader from overwriting
        if let Err(error) = self.save_config_and_prevent_reload() {
            log_warn(&format!("[nex] quick_launch unpin save failed: {error}"));
        } else {
            log_info(&format!("[nex] quick_launch unpinned '{}'", title));
            log_info(&format!("[nex] quick_launch pinned_list={:?}", self.runtime_config.quick_launch.pinned));
            // Reload Quick Launch items from in-memory config FIRST
            self.load_quick_launch_items_from_config();
            // If in Quick Launch mode (empty query), rebuild the rows
            if self.overlay.query_text().trim().is_empty() {
                self.show_idle_or_quick_launch();
            } else {
                // In search mode — just push state (includes updated quickLaunch array for pin icons)
                self.overlay.set_status_text_and_refresh(&format!("Unpinned '{}' from Quick Launch", title));
            }
        }
    }

    /// Save config and update watcher timestamp to prevent reloader from overwriting.
    fn save_config_and_prevent_reload(&mut self) -> Result<(), String> {
        crate::config::save(&self.runtime_config)
            .map_err(|e| format!("save failed: {e}"))?;
        // Update the watcher's last_modified to the current file time
        // so the config reloader doesn't see it as "changed" and overwrite our in-memory state.
        if let Some(modified) = crate::runtime_index::config_file_modified_time(&self.runtime_config.config_path) {
            self.config_watcher.last_modified = Some(modified);
        }
        Ok(())
    }

    /// Add an app to Quick Launch by path (from search results).
    fn add_to_quick_launch(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }

        // Normalize the path for comparison
        let normalized = trimmed.replace('/', "\\").to_ascii_lowercase();

        // Add to config pinned list if not already there
        let already_pinned = self.runtime_config.quick_launch.pinned.iter().any(|p| {
            p.replace('/', "\\").to_ascii_lowercase() == normalized
        });

        if !already_pinned {
            self.runtime_config.quick_launch.pinned.push(trimmed.to_string());

            // Persist to config file and prevent reloader from overwriting
            if let Err(error) = self.save_config_and_prevent_reload() {
                log_warn(&format!("[nex] quick_launch add save failed: {error}"));
            } else {
                log_info(&format!("[nex] quick_launch added '{}'", trimmed));
                log_info(&format!("[nex] quick_launch pinned_list={:?}", self.runtime_config.quick_launch.pinned));
                // Reload Quick Launch items from in-memory config FIRST
                self.load_quick_launch_items_from_config();
                // If in Quick Launch mode (empty query), rebuild the rows
                if self.overlay.query_text().trim().is_empty() {
                    self.show_idle_or_quick_launch();
                } else {
                    // In search mode — just push state (includes updated quickLaunch array for pin icons)
                    self.overlay.set_status_text_and_refresh(&format!("Added to Quick Launch"));
                }
            }
        } else {
            log_info(&format!("[nex] quick_launch already pinned '{}'", trimmed));
        }
    }

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

    fn handle_context_action(&mut self, action: &str, _title: &str, path: &str) {
        match action {
            "runas" => {
                let target = if !path.is_empty() { path } else { _title };
                self.overlay.hide_sync();
                self.overlay_state.on_escape();
                match crate::action_executor::launch_as_admin(target) {
                    Ok(()) => {
                        log_info(&format!("[nex] launched as admin: '{target}'"));
                        reset_overlay_session(
                            &self.overlay,
                            &mut self.current_results,
                            &mut self.selected_index,
                        );
                        self.last_query.clear();
                        self.search_session.clear();
                        self.search_worker.clear_session();
                    }
                    Err(error) => {
                        self.overlay.set_status_text(&format!("Run as admin error: {error}"));
                    }
                }
            }
            "openfolder" => {
                if path.is_empty() { return; }
                let parent = std::path::Path::new(path).parent();
                let folder = parent
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());
                if folder.is_empty() { return; }
                // Use explorer /select to highlight the file/folder, works for
                // both filesystem paths and shell: URIs.
                let folder_arg = if folder.starts_with("shell:") {
                    folder.clone()
                } else {
                    folder.to_string()
                };
                self.overlay.hide_sync();
                self.overlay_state.on_escape();
                match crate::action_executor::launch_open_target(&folder_arg) {
                    Ok(()) => {
                        log_info(&format!("[nex] opened file location: '{}'", folder));
                        reset_overlay_session(
                            &self.overlay,
                            &mut self.current_results,
                            &mut self.selected_index,
                        );
                        self.last_query.clear();
                        self.search_session.clear();
                        self.search_worker.clear_session();
                    }
                    Err(error) => {
                        self.overlay.set_status_text(&format!("Open location error: {error}"));
                    }
                }
            }
            "copypath" => {
                if path.is_empty() { return; }
                if copy_to_clipboard(path) {
                    self.overlay.set_status_text("Path copied to clipboard");
                } else {
                    self.overlay.set_status_text("Failed to copy path");
                }
            }
            "uninstall" => {
                if _title.is_empty() { return; }
                let items = crate::uninstall_registry::search_uninstall_actions(_title, 5);
                if let Some(item) = items.first() {
                    log_info(&format!("[nex] uninstalling '{}' via command '{}'", _title, item.id));
                    match crate::uninstall_registry::execute_uninstall_action(&item.id) {
                        Ok(()) => {
                            track_uninstall_title_suppression(
                                &mut self.suppressed_uninstall_titles,
                                &format!("Uninstall {}", _title),
                            );
                            let lookup = if !path.is_empty() { path.to_string() } else { _title.to_string() };
                            if let Ok(guard) = self.service.write() {
                                if let Ok(Some((stale_id, _, _, _, _))) = crate::index_store::find_item_by_path_or_title(&guard.db_ref(), &lookup) {
                                    let _ = guard.delete_item_by_id(&stale_id);
                                }
                            }
                            filter_suppressed_uninstall_results(&mut self.current_results, &self.suppressed_uninstall_titles);
                            if self.current_results.is_empty() {
                                set_status_row_overlay_state(&self.overlay, STATUS_ROW_NO_RESULTS);
                            } else {
                                let rows = overlay_rows_ext(
                                    &self.current_results,
                                    false,
                                    self.apps_expanded.is_none(),
                                );
                                self.current_rows = rows;
                                self.overlay.set_results(&self.current_rows, self.selected_index.min(self.current_results.len().saturating_sub(1)));
                            }
                            self.overlay.set_status_text(&format!("Uninstalling '{}'...", _title));
                        }
                        Err(error) => {
                            let msg = format!("Uninstall failed: {error}");
                            log_warn(&format!("[nex] {msg}"));
                            self.overlay.set_status_text(&msg);
                        }
                    }
                } else {
                    let msg = format!("No uninstall entry found for '{}'", _title);
                    log_info(&format!("[nex] {msg}"));
                    self.overlay.set_status_text(&msg);
                }
            }
            "pin" => {
                if !path.is_empty() { self.add_to_quick_launch(path); }
                else { self.pin_app_to_quick_launch(_title); }
            }
            "unpin" => {
                self.unpin_app_from_quick_launch(_title);
            }
            _ => {
                log_warn(&format!("[nex] unknown context action: {action}"));
            }
        }
    }

    fn on_event(&mut self, event: OverlayEvent) {
        self.hotkey_check_counter = self.hotkey_check_counter.wrapping_add(1);

        // Diagnostic: heartbeat every 128 events (Tick or otherwise)
        if self.hotkey_check_counter % 128 == 0 {
            log_info(&format!(
                "[nex::diag] worker alive counter={} event={:?}",
                self.hotkey_check_counter,
                event,
            ));
        }

        if self.last_memory_log.elapsed() >= Duration::from_secs(30) {
            if let Ok(guard) = self.service.try_read() {
                guard.log_memory_stats();
            }
            self.last_memory_log = Instant::now();
        }

        if self.hotkey_check_counter % 32 == 0 && !RE_REGISTERING.load(std::sync::atomic::Ordering::SeqCst) && !QUIT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            let needs_restart = match self.hotkey_listener.lock() {
                Ok(guard) => match guard.as_ref() {
                    Some(listener) => !listener.is_alive(),
                    None => false,
                },
                Err(_) => false,
            };
            if needs_restart {
                log_warn("[nex] hotkey listener thread died; attempting restart");
                // Drop the dead listener.
                if let Ok(mut guard) = self.hotkey_listener.lock() {
                    *guard = None;
                }
                // Spawn recovery on a separate thread so the runtime
                // worker never blocks — same pattern as config-change
                // re-registration.
                let recovery_hotkey = self.runtime_config.hotkey.clone();
                let recovery_etx = self.event_tx.clone();
                let recovery_lh = self.hotkey_listener.clone();
                std::thread::Builder::new()
                    .name("nex-hotkey-recover".into())
                    .spawn(move || {
                        let _ = std::process::Command::new("schtasks")
                            .args(["/end", "/tn", "NexHelperV2"])
                            .output();
                        let _ = std::process::Command::new("taskkill")
                            .args(["/F", "/IM", "NexHelper.exe"])
                            .creation_flags(0x08000000)
                            .output();
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        if QUIT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
                            log_info("[nex] skipping hotkey listener restart: quit in progress");
                            return;
                        }
                        match HotkeyListener::start(&recovery_hotkey, recovery_etx) {
                            Ok(listener) => {
                                log_info("[nex] hotkey listener restarted successfully");
                                if let Ok(mut guard) = recovery_lh.lock() {
                                    *guard = Some(listener);
                                }
                            }
                            Err(error) => {
                                log_warn(&format!(
                                    "[nex] hotkey listener restart failed: {error}"
                                ));
                            }
                        }
                    })
                    .ok();
            }
        }

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

        if let Ok(svc) = self.service.try_write() {
            let hotkey_changed = maybe_apply_runtime_config_reload(
                &self.overlay,
                &*svc,
                &mut self.runtime_config,
                &mut self.plugin_registry,
                &mut self.search_session,
                &mut self.pending_confirmation,
                &mut self.max_results,
                &mut self.config_watcher,
                &mut self.background_index_refresh,
            );
            if hotkey_changed {
                log_info(&format!(
                    "[nex] config hotkey changed, re-registering listener for '{}'",
                    self.runtime_config.hotkey,
                ));
                // Gate: only one re-registration at a time — drop old
                // listener inside the gate so we don't lose it if another
                // re-registration is already in progress.
                if RE_REGISTERING.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
                    if let Ok(mut guard) = self.hotkey_listener.lock() {
                        *guard = None;
                    }
                    // Re-register on a separate thread — old helper is killed
                    // async by Drop, so the new start() may fall back to
                    // in-process hook if the old helper's pipe lingers.
                    let new_hotkey = self.runtime_config.hotkey.clone();
                    let event_tx = self.event_tx.clone();
                    let lh = self.hotkey_listener.clone();
                    std::thread::Builder::new()
                        .name("nex-hotkey-reconfig".into())
                        .spawn(move || {
                            // Stop the scheduled task and kill the helper
                            // process so the new start() can write config
                            // and re-launch a clean helper.
                            let _ = std::process::Command::new("schtasks")
                                .args(["/end", "/tn", "NexHelperV2"])
                                .output();
                            let _ = std::process::Command::new("taskkill")
                                .args(["/F", "/IM", "NexHelper.exe"])
                                .creation_flags(0x08000000)
                                .output();
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            // start() writes helper-config.json, relaunches
                            // the task, and connects or falls back to
                            // in-process hook. All on this thread — runtime
                            // worker is never blocked.
                            match HotkeyListener::start(&new_hotkey, event_tx) {
                                Ok(listener) => {
                                    log_info("[nex] hotkey re-registered successfully");
                                    match lh.lock() {
                                        Ok(mut guard) => {
                                            *guard = Some(listener);
                                            log_info("[nex::diag] re-registration: listener stored in hotkey_listener");
                                        }
                                        Err(e) => {
                                            log_warn("[nex::diag] re-registration: hotkey_listener mutex poisoned, listener lost!");
                                        }
                                    }
                                }
                                Err(error) => {
                                    log_warn(&format!(
                                        "[nex] hotkey re-registration failed: {error}"
                                    ));
                                }
                            }
                            RE_REGISTERING.store(false, std::sync::atomic::Ordering::SeqCst);
                        })
                        .ok();
                } else {
                    log_info("[nex] hotkey re-registration already in progress, skipping");
                }
                // Status text already set by maybe_apply_runtime_config_reload.
            }
            // Keep the search worker's config in sync so it doesn't
            // serve stale results with old show_files/show_folders etc.
            if let Ok(mut cfg) = self.shared_config.write() {
                *cfg = self.runtime_config.clone();
            }
            if let Ok(mut reg) = self.shared_plugin_registry.write() {
                *reg = self.plugin_registry.clone();
            }
            // Invalidate search worker caches on every config reload so
            // the worker picks up the new show_files, show_folders, dsl,
            // and plugin toggles right away instead of serving stale
            // results from the previous session.
            self.config_generation += 1;
            self.search_worker.clear_session();
            maybe_apply_background_index_refresh(
                &*svc,
                &mut self.background_index_refresh,
                &self.runtime_config,
            );

            // Start per-root file watchers the first time the index
            // cache becomes usable. The handle is idempotent: it is a
            // no-op if a watcher is already running.
            if self.background_index_refresh.cache_applied {
                svc.start_stale_pruner(&self.service);
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
                self.config_generation,
                self.max_results,
                &mut self.pending_confirmation,
                &mut self.current_results,
                &mut self.selected_index,
                &mut self.last_query,
                &mut self.last_sent_generation,
            );
        }

        // Retry a quick-launch load deferred by a busy indexer. The show
        // path opens the overlay immediately even when the service lock is
        // contended, so fill the idle view as soon as the lock frees.
        if !self.quick_launch_loaded
            && self.runtime_config.quick_launch.enabled
            && self.overlay.is_visible()
            && self.overlay.query_text().trim().is_empty()
        {
            self.load_quick_launch_items();
            if self.quick_launch_loaded {
                self.show_idle_or_quick_launch();
            }
        }
        match event {
            OverlayEvent::Hotkey(_) => {
                if self.settings_open {
                    return; // Settings owns the moment — don't summon the launcher over it
                }
                // Debounce: skip if we just processed a hotkey within 50ms.
                // The WH_KEYBOARD_LL hook and WM_INPUT (raw input sink) can
                // both fire for the same keystroke.  This deduplicates them.
                if !HOTKEY_DEBOUNCE.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
                    log_info("[nex::debug] Hotkey: debounced (within 50ms window)");
                    return;
                }
                // Auto-reset debounce gate after 50ms.
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    HOTKEY_DEBOUNCE.store(false, std::sync::atomic::Ordering::SeqCst);
                });
                log_info("[nex] hotkey_event received");
                let overlay_visible = self.overlay.is_visible();
                let state_before = self.overlay_state.is_visible();
                self.overlay_state.set_visible(overlay_visible);
                let state_after = self.overlay_state.is_visible();
                log_info(&format!(
                    "[nex::debug] Hotkey: shim_visible={} overlay_state_before={} overlay_state_after={}",
                    overlay_visible, state_before, state_after,
                ));
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
                    crate::overlay::hotkey::finish_bare_win_press();
                    return;
                }
                if !overlay_visible
                    && should_suppress_hotkey_for_game_mode(&self.runtime_config)
                {
                    log_info(
                        "[nex] hotkey ignored because game mode is active for the foreground app",
                    );
                    crate::overlay::hotkey::finish_bare_win_press();
                    return;
                }
                let action = self.overlay_state.on_hotkey(self.overlay.has_focus());
                log_info(&format!(
                    "[nex::debug] Hotkey: action={:?} has_focus={}",
                    action, self.overlay.has_focus(),
                ));
                match action {
                    HotkeyAction::ShowAndFocus | HotkeyAction::FocusExisting => {
                        // Warm search indexes synchronously before showing the
                        // overlay.  The user can't type until the window appears
                        // (~160ms animation) and the IPC channel is live, so
                        // the ~50ms warmup finishes well before the first char.
                        // Uses try_read() — if the background indexer holds the
                        // write lock (e.g. first-run rebuild), skip the warmup
                        // and show immediately; the query path re-warms on the
                        // first keystroke.
                        match self.service.try_read() {
                            Ok(guard) => guard.warm_search_cache(),
                            Err(_) => log_info("[nex] warm_search_cache deferred (index busy)"),
                        }
                        reconcile_suppressed_uninstall_titles(
                            &mut self.suppressed_uninstall_titles,
                        );
                        if self.overlay.query_text().trim().is_empty() {
                            self.load_quick_launch_items();
                            self.show_idle_or_quick_launch();
                        }
                        self.overlay.show_and_focus();
                        if self.runtime_config.clipboard_enabled {
                            let _ = clipboard_history::maybe_capture_latest(&self.runtime_config);
                        }
                    }
                    HotkeyAction::Hide => {
                        self.overlay.hide();
                        reset_overlay_session(
                            &self.overlay,
                            &mut self.current_results,
                            &mut self.selected_index,
                        );
                        self.pending_confirmation = None;
                        self.last_query.clear();
                        self.last_sent_generation = 0;
                        self.search_session.clear();
                        self.search_worker.clear_session();
                        // Drain any in-flight results still sitting in the
                        // channel so they don't overwrite the idle state on
                        // the next show.
                        while self.search_worker.try_recv().is_some() {}
                        maybe_apply_background_index_refresh(
                            &*self.service.write().unwrap_or_else(|e| e.into_inner()),
                            &mut self.background_index_refresh,
                            &self.runtime_config,
                        );
                    }
                }
                crate::overlay::hotkey::finish_bare_win_press();
            }
            OverlayEvent::ExternalShow => {
                // Same sync warmup as ShowAndFocus — user can't type until
                // window appears + IPC handler is live.
                if let Ok(guard) = self.service.read() {
                    guard.warm_search_cache();
                }
                reconcile_suppressed_uninstall_titles(&mut self.suppressed_uninstall_titles);
                if self.overlay.query_text().trim().is_empty() {
                    self.load_quick_launch_items();
                    self.show_idle_or_quick_launch();
                }
                self.overlay.show_and_focus();
                self.overlay_state.set_visible(true);
                if self.runtime_config.clipboard_enabled {
                    let _ = clipboard_history::maybe_capture_latest(&self.runtime_config);
                }
            }
            OverlayEvent::ExternalQuit => {
                QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
                self.overlay.hide_now();
                self.last_query.clear();
                self.last_sent_generation = 0;
                self.search_session.clear();
                self.search_worker.clear_session();
                while self.search_worker.try_recv().is_some() {}
                self.overlay.quit_if_running();
            }
            OverlayEvent::TrayToggleGameMode => {
                toggle_game_mode_from_tray(&self.overlay, &mut self.runtime_config);
                let _ = self.tray_gm_tx.send(self.runtime_config.game_mode_enabled);
            }
            OverlayEvent::OpenSettings => {
                self.overlay.hide_now();
                self.settings_open = true;
                let snap = crate::settings_snapshot::build(&self.runtime_config);
                self.overlay.open_settings(snap);
            }
            OverlayEvent::SaveSettings(raw) => {
                match crate::settings_snapshot::apply(&self.runtime_config, &raw).and_then(|updated| crate::settings_snapshot::save(&updated)) {
                    Ok(()) => self.overlay.settings_save_result("{\"saved\":true}".into()),
                    Err(e) => self.overlay.settings_save_result(serde_json::json!({ "saved": false, "error": e}).to_string(),
                    ),
                }
            }
            OverlayEvent::SettingsClosed => {
                self.settings_open = false;
            }
            OverlayEvent::TrayCheckForUpdates => {
                self.overlay.set_status_text("Checking for updates...");
                let event_tx = self.event_tx.clone();
                std::thread::Builder::new()
                    .name("nex-updater-check".into())
                    .spawn(move || {
                        let message = match crate::updater::run_updater_capture(
                            crate::updater::UpdateChannel::Stable,
                        ) {
                            Ok(output) => crate::updater::summarize_update_output(&output),
                            Err(error) => {
                                format!("Could not launch updater: {error}")
                            }
                        };
                        log_info(&format!("[nex] updater result: {message}"));
                        let _ = event_tx.send(OverlayEvent::UpdateStatus(message));
                    })
                    .ok();
            }
            OverlayEvent::UpdateStatus(text) => {
                self.overlay.set_status_text(&text);
            }
            OverlayEvent::TrayLock => {
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::lock() {
                        log_warn(&format!("[nex] tray lock failed: {error}"));
                    }
                });
            }
            OverlayEvent::TraySleep => {
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::sleep() {
                        log_warn(&format!("[nex] tray sleep failed: {error}"));
                    }
                });
            }
            OverlayEvent::TrayShutdown => {
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::confirm_and_shutdown() {
                        log_warn(&format!("[nex] tray shutdown failed: {error}"));
                    }
                });
            }
            OverlayEvent::TrayRestart => {
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::confirm_and_restart() {
                        log_warn(&format!("[nex] tray restart failed: {error}"));
                    }
                });
            }
            OverlayEvent::TraySignOut => {
                // Session ends — hide the overlay before DWM teardown.
                self.overlay.hide_sync();
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::sign_out() {
                        log_warn(&format!("[nex] tray sign out failed: {error}"));
                    }
                });
            }
            // Confirmed in the overlay — execute immediately.
            // Hide first and wait for it: during a shutdown/restart the
            // DWM/COM teardown can hit WebView2 with E_OUTOFMEMORY-style
            // errors while the window is still composited.
            OverlayEvent::PowerMenuShutdown => {
                self.overlay.hide_sync();
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::shutdown() {
                        log_warn(&format!("[nex] power menu shutdown failed: {error}"));
                    }
                });
            }
            OverlayEvent::PowerMenuRestart => {
                self.overlay.hide_sync();
                std::thread::spawn(move || {
                    if let Err(error) = crate::power_actions::restart() {
                        log_warn(&format!("[nex] power menu restart failed: {error}"));
                    }
                });
            }
            OverlayEvent::FocusSearchInput => {
                self.overlay.focus_search_input();
            }
            OverlayEvent::Escape => {
                let before_shim = self.overlay.is_visible();
                let before_overlay = self.overlay_state.is_visible();
                let action = self.overlay_state.on_escape();
                log_info(&format!(
                    "[nex::debug] Escape: before(shim={} overlay={}) on_escape={}",
                    before_shim, before_overlay, action,
                ));
                if action {
                    self.overlay.hide();
                    let after_shim = self.overlay.is_visible();
                    let after_overlay = self.overlay_state.is_visible();
                    log_info(&format!(
                        "[nex::debug] Escape: after(shim={} overlay={})",
                        after_shim, after_overlay,
                    ));
                    reset_overlay_session(
                        &self.overlay,
                        &mut self.current_results,
                        &mut self.selected_index,
                    );
                    self.pending_confirmation = None;
                    self.apps_expanded = None;
                    self.pending_show_all = None;
                    self.last_query.clear();
                    self.last_sent_generation = 0;
                    self.search_session.clear();
                    self.search_worker.clear_session();
                    while self.search_worker.try_recv().is_some() {}
                }
            }
            OverlayEvent::QueryChanged(query) => {
                let trimmed = query.trim();
                if trimmed.is_empty() {
                    // Query cleared — show Quick Launch or idle state
                    self.current_results.clear();
                    self.current_rows.clear();
                    self.selected_index = 0;
                    self.last_query.clear();
                    self.last_sent_generation = self.last_sent_generation.wrapping_add(1);
                    self.pending_confirmation = None;
                    self.apps_expanded = None;
                    self.pending_show_all = None;
                    // Reload Quick Launch items to ensure fresh data
                    self.load_quick_launch_items();
                    self.show_idle_or_quick_launch();
                    return;
                }
                // New query text collapses the "Show all apps" expansion.
                if self.apps_expanded.as_deref() != Some(trimmed) {
                    self.apps_expanded = None;
                }
                apply_query_change(
                    query,
                    &self.overlay,
                    &self.search_worker,
                    &self.runtime_config,
                    self.config_generation,
                    self.max_results,
                    &mut self.pending_confirmation,
                    &mut self.current_results,
                    &mut self.selected_index,
                    &mut self.last_query,
                    &mut self.last_sent_generation,
                );
            }
            OverlayEvent::SearchResultsReady => {
                // Don't apply results from stale pre-hide searches
                // that complete after the overlay is re-shown with
                // an empty query.
                if self.overlay.query_text().trim().is_empty() {
                    let _ = self.search_worker.try_recv();
                    return;
                }
                apply_search_results(
                    &self.search_worker,
                    &self.overlay,
                    &self.runtime_config,
                    &self.background_index_refresh,
                    &self.suppressed_uninstall_titles,
                    &self.runtime_config.quick_launch.pinned,
                    &mut self.current_results,
                    &mut self.current_rows,
                    &mut self.selected_index,
                    self.last_sent_generation,
                    self.apps_expanded.as_deref(),
                );
            }
            OverlayEvent::ShowAllAppsFillRest => {
                // Expansion was cancelled (query changed / escaped) before
                // the remainder timer fired — drop it.
                let Some(pending) = self.pending_show_all.take() else {
                    return;
                };
                if self.apps_expanded.is_none() {
                    return;
                }
                // Append-only: existing rows keep their exact positions.
                for item in pending {
                    self.current_results.push(item.clone());
                    let idx = self.current_results.len() - 1;
                    self.current_rows.push(result_row(
                        &item,
                        idx,
                        OverlayRowRole::Item,
                        false,
                    ));
                }
                self.overlay
                    .set_results(&self.current_rows, self.selected_index);
            }
            OverlayEvent::Submit => {
                // Check if we're in Quick Launch mode (empty query, Quick Launch visible)
                if self.overlay.query_text().trim().is_empty() && !self.quick_launch_items.is_empty() {
                    if let Some(list_selection) = self.overlay.selected_index() {
                        if list_selection < self.quick_launch_items.len() {
                            let item = &self.quick_launch_items[list_selection];
                            let path = item.path.clone();
                            self.overlay.hide_sync();
                            self.overlay_state.on_escape();
                            // Launch the Quick Launch item
                            match crate::action_executor::launch_open_target(&path) {
                                Ok(()) => {
                                    log_info(&format!("[nex] quick_launch launched '{}'", item.title));
                                    // Record the launch
                                    let now = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs() as i64)
                                        .unwrap_or(0);
                                    let guard = self.service.read().unwrap_or_else(|e| e.into_inner());
                                    let db = guard.db_ref();
                                    // Find the item ID by path to record launch
                                    if let Ok(Some((id, _, _, _, _))) = crate::index_store::find_item_by_path_or_title(&db, &path) {
                                        if let Err(error) = crate::index_store::record_launch(&db, &id, now) {
                                            log_warn(&format!("[nex] record_launch failed: {error}"));
                                        }
                                    }
                                    reset_overlay_session(
                                        &self.overlay,
                                        &mut self.current_results,
                                        &mut self.selected_index,
                                    );
                                    self.last_query.clear();
                                    self.search_session.clear();
                                    self.search_worker.clear_session();
                                }
                                Err(error) => {
                                    self.overlay.set_status_text(&format!("Launch error: {error}"));
                                }
                            }
                            return;
                        }
                    }
                }

                // "Show all apps" entry — re-run the query apps-only.
                if let Some(list_selection) = self.overlay.selected_index() {
                    if self.current_rows.get(list_selection).map(|r| r.role)
                        == Some(OverlayRowRole::ShowAllApps)
                    {
                        self.expand_all_apps();
                        return;
                    }
                }

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
                    let mapped = self
                        .current_rows
                        .get(list_selection)
                        .and_then(|r| r.result_index)
                        .unwrap_or(list_selection);
                    self.selected_index = mapped.min(self.current_results.len() - 1);
                }

                let selected = &self.current_results[self.selected_index];
                if self.pending_confirmation.is_some() {
                    let selected_id = selected.id.clone();
                    let is_confirm = selected_id == ACTION_UNINSTALL_CONFIRM_ID
                        || selected_id == ACTION_POWER_CONFIRM_ID;
                    let is_cancel = selected_id == ACTION_UNINSTALL_CANCEL_ID
                        || selected_id == ACTION_POWER_CANCEL_ID;

                    if is_confirm {
                        let Some(pending) = self.pending_confirmation.take() else {
                            return;
                        };
                        match pending.kind {
                            ConfirmationKind::Uninstall => {
                                let Some(uninstall_action) = pending.uninstall_action.as_ref() else {
                                    log_warn("[nex] confirm uninstall without pending uninstall action; ignoring");
                                    return;
                                };
                                self.overlay.hide_now();
                                self.overlay_state.on_escape();
                                match execute_action_selection(
                                    &*self.service.write().unwrap_or_else(|e| e.into_inner()),
                                    &self.runtime_config,
                                    &self.plugin_registry,
                                    uninstall_action,
                                ) {
                                    Ok(()) => {
                                        track_uninstall_title_suppression(
                                            &mut self.suppressed_uninstall_titles,
                                            uninstall_action.title.as_str(),
                                        );
                                        if let Some(target_title) = uninstall_target_title_from_action_title(uninstall_action.title.as_str()) {
                                            if let Ok(guard) = self.service.write() {
                                                if let Ok(Some((stale_id, _, _, _, _))) = crate::index_store::find_item_by_path_or_title(&guard.db_ref(), &target_title) {
                                                    let _ = guard.delete_item_by_id(&stale_id);
                                                }
                                            }
                                        }
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
                                                uninstall_action.title.as_str(),
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
                                                let rows = overlay_rows_ext(
                                                    &self.current_results,
                                                    pending.previous_command_mode,
                                                    !pending.previous_command_mode
                                                        && self.apps_expanded.is_none(),
                                                );
                                                self.current_rows = rows;
                                                self.overlay.set_results(&self.current_rows, self.selected_index);
                                            }
                                            self.overlay.set_status_text(
                                                "Uninstall entry is stale and was hidden",
                                            );
                                        } else {
                                            self.pending_confirmation = Some(pending);
                                            self.overlay.show_and_focus();
                                            self.overlay
                                                .set_status_text(&format!("Launch error: {error}"));
                                        }
                                    }
                                }
                            }
                            ConfirmationKind::Shutdown | ConfirmationKind::Restart | ConfirmationKind::SignOut => {
                                self.overlay.hide_now();
                                self.overlay_state.on_escape();
                                let action_id = match pending.kind {
                                    ConfirmationKind::Shutdown => crate::action_registry::ACTION_SHUTDOWN_ID,
                                    ConfirmationKind::Restart => crate::action_registry::ACTION_RESTART_ID,
                                    ConfirmationKind::SignOut => crate::action_registry::ACTION_SIGN_OUT_ID,
                                    _ => unreachable!(),
                                };
                                let power_item = SearchItem::new(action_id, "action", "", "");
                                match execute_action_selection(
                                    &*self.service.write().unwrap_or_else(|e| e.into_inner()),
                                    &self.runtime_config,
                                    &self.plugin_registry,
                                    &power_item,
                                ) {
                                    Ok(()) => {
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
                                    Err(_) => {
                                        self.pending_confirmation = Some(pending);
                                        self.overlay.show_and_focus();
                                        self.overlay.set_status_text("Power action failed");
                                    }
                                }
                            }
                        }
                        return;
                    }

                    if is_cancel {
                        let Some(pending) = self.pending_confirmation.take() else {
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
                            let rows = overlay_rows_ext(
                                &self.current_results,
                                pending.previous_command_mode,
                                !pending.previous_command_mode && self.apps_expanded.is_none(),
                            );
                            self.current_rows = rows;
                            self.overlay.set_results(&self.current_rows, self.selected_index);
                        }
                        self.overlay.set_status_text("");
                        return;
                    }

                    self.pending_confirmation = None;
                }

                let selected_is_uninstall = selected
                    .id
                    .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX);

                if selected_is_uninstall {
                    let parsed_query = ParsedQuery::parse(
                        self.overlay.query_text().trim(),
                        self.runtime_config.search_dsl_enabled,
                    );
                    self.pending_confirmation = Some(PendingConfirmation {
                        kind: ConfirmationKind::Uninstall,
                        uninstall_action: Some(selected.clone()),
                        previous_results: self.current_results.clone(),
                        previous_selected_index: self.selected_index,
                        previous_command_mode: parsed_query.command_mode,
                    });
                    self.current_results = uninstall_confirmation_results(selected);
                    self.selected_index = 0;
                    let rows = overlay_rows(&self.current_results, true);
                    self.current_rows = rows;
                    self.overlay.set_results(&self.current_rows, self.selected_index);
                    self.overlay.set_status_text("");
                    return;
                }

                // Power actions going through confirmation
                let selected_id = selected.id.as_str();
                if selected_id == crate::action_registry::ACTION_SHUTDOWN_ID
                    || selected_id == crate::action_registry::ACTION_RESTART_ID
                    || selected_id == crate::action_registry::ACTION_SIGN_OUT_ID
                {
                    let kind = match selected_id {
                        crate::action_registry::ACTION_SHUTDOWN_ID => ConfirmationKind::Shutdown,
                        crate::action_registry::ACTION_RESTART_ID => ConfirmationKind::Restart,
                        crate::action_registry::ACTION_SIGN_OUT_ID => ConfirmationKind::SignOut,
                        _ => unreachable!(),
                    };
                    let parsed_query = ParsedQuery::parse(
                        self.overlay.query_text().trim(),
                        self.runtime_config.search_dsl_enabled,
                    );
                    self.pending_confirmation = Some(PendingConfirmation {
                        kind,
                        uninstall_action: None,
                        previous_results: self.current_results.clone(),
                        previous_selected_index: self.selected_index,
                        previous_command_mode: parsed_query.command_mode,
                    });
                    self.current_results = power_confirmation_results(kind);
                    self.selected_index = 0;
                    let rows = overlay_rows(&self.current_results, true);
                    self.current_rows = rows;
                    self.overlay.set_results(&self.current_rows, self.selected_index);
                    self.overlay.set_status_text("");
                    return;
                }

                // Lock and Sleep are non-destructive — execute immediately
                if selected_id == crate::action_registry::ACTION_LOCK_ID
                    || selected_id == crate::action_registry::ACTION_SLEEP_ID
                {
                    self.overlay.hide_now();
                    self.overlay_state.on_escape();
                    match execute_action_selection(
                        &*self.service.write().unwrap_or_else(|e| e.into_inner()),
                        &self.runtime_config,
                        &self.plugin_registry,
                        selected,
                    ) {
                        Ok(()) => {
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
                            self.overlay.show_and_focus();
                            self.overlay.set_status_text(&format!("Action failed: {error}"));
                        }
                    }
                    return;
                }

                if selected.id == crate::action_registry::ACTION_TRIM_MEMORY_ID {
                    self.search_session.clear();
                    self.overlay.trim_runtime_memory();
                    self.overlay.set_status_text("Memory caches trimmed");
                    return;
                }

                self.overlay.hide_sync();
                self.overlay_state.on_escape();
                match launch_overlay_selection(
                    &*self.service.write().unwrap_or_else(|e| e.into_inner()),
                    &self.runtime_config,
                    &self.plugin_registry,
                    &self.current_results,
                    self.selected_index,
                    self.last_query.as_str(),
                ) {
                    Ok(()) => {
                        self.overlay.set_status_text("");
                        reset_overlay_session(
                            &self.overlay,
                            &mut self.current_results,
                            &mut self.selected_index,
                        );
                        self.pending_confirmation = None;
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
            OverlayEvent::PinApp(title) => {
                self.pin_app_to_quick_launch(&title);
            }
            OverlayEvent::UnpinApp(title) => {
                self.unpin_app_from_quick_launch(&title);
            }
            OverlayEvent::AddToQuickLaunch(path) => {
                self.add_to_quick_launch(&path);
            }
            OverlayEvent::ContextAction(action, title, path) => {
                self.handle_context_action(&action, &title, &path);
            }
            _ => {} // MoveSelection is handled locally by JS; other variants are forward-compat
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
    config_generation: u64,
    max_results: usize,
    pending_confirmation: &mut Option<PendingConfirmation>,
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
    last_query: &mut String,
    last_sent_generation: &mut u64,
) {
    *pending_confirmation = None;
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
        *pending_confirmation = None;
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

    let generation = search_worker.send_request(config_generation, parsed_query, query_result_limit);
    *last_sent_generation = generation;
}

#[cfg(target_os = "windows")]
fn apply_search_results(
    search_worker: &SearchWorker,
    overlay: &NativeOverlayShell,
    _runtime_config: &Config,
    background_index_refresh: &BackgroundIndexRefresh,
    suppressed_uninstall_titles: &[String],
    pinned_paths: &[String],
    current_results: &mut Vec<crate::model::SearchItem>,
    current_rows: &mut Vec<crate::overlay::OverlayRow>,
    selected_index: &mut usize,
    last_sent_generation: u64,
    apps_expanded_query: Option<&str>,
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
        current_rows.clear();
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

    // Sort pinned items to the top ONLY during idle quick-launch view
    // (empty query). When query is non-empty, keep natural rank order.
    let is_idle = overlay.query_text().trim().is_empty();
    if !pinned_paths.is_empty() && is_idle {
        let pinned_normalized: Vec<String> = pinned_paths.iter()
            .map(|p| p.replace('/', "\\").to_ascii_lowercase())
            .collect();
        results.sort_by(|a, b| {
            let a_pinned = pinned_normalized.iter().any(|p| {
                a.path.replace('/', "\\").to_ascii_lowercase() == *p
            });
            let b_pinned = pinned_normalized.iter().any(|p| {
                b.path.replace('/', "\\").to_ascii_lowercase() == *p
            });
            b_pinned.cmp(&a_pinned) // true (pinned) comes before false
        });
    }

    *current_results = results;
    *selected_index = 0;

    if current_results.is_empty() {
        let indexing_in_progress = !background_index_refresh
            .completed
            .load(std::sync::atomic::Ordering::Acquire);
        // Only show "Indexing, please wait…" on first-time indexing
        // (empty cache).  Async background refreshes on a populated
        // index are invisible — the existing cache is queryable.
        if indexing_in_progress
            && background_index_refresh.initial_cache_empty
            && !command_mode
        {
            set_status_row_overlay_state(overlay, "Indexing, please wait...");
        } else if command_mode {
            set_status_row_overlay_state(overlay, STATUS_ROW_NO_COMMAND_RESULTS);
        } else if apps_expanded_query.is_some() {
            // "Show all apps" expansion found zero app matches — leave the
            // results empty so the caller can restore the pre-expansion
            // view instead of collapsing to a web-search row.
            current_results.clear();
            current_rows.clear();
        } else {
            let query = overlay.query_text();
            // Reuse the existing web search action system — respects
            // the user's configured search provider (Google, DuckDuckGo,
            // Bing, or custom template) and proper URL encoding.
            if let Some(search_item) = crate::action_registry::dynamic_provider_web_search_action(
                query.trim(),
                _runtime_config,
            ) {
                *current_results = vec![search_item];
                *selected_index = 0;
                let rows = overlay_rows(current_results, command_mode);
                *current_rows = rows;
                overlay.set_results(current_rows, *selected_index);
            } else {
                set_status_row_overlay_state(overlay, STATUS_ROW_NO_RESULTS);
            }
        }
    } else {
        let expanded = apps_expanded_query == Some(overlay.query_text().trim());
        let rows = overlay_rows_ext(current_results, command_mode, !command_mode && !is_idle && !expanded);
        *current_rows = rows;
        // During expansion the intermediate apps-only view is NOT pushed —
        // it's smaller than the pre-click list, so pushing it would make
        // the window visibly shorten before the index append grows it
        // back. The caller pushes once after appending.
        if !expanded {
            overlay.set_results(current_rows, *selected_index);
        }
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
