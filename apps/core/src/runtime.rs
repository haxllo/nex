use crate::action_registry::{
    search_actions_with_mode, ACTION_CLEAR_CLIPBOARD_ID, ACTION_DIAGNOSTICS_BUNDLE_ID,
    ACTION_OPEN_CONFIG_ID, ACTION_OPEN_LOGS_ID, ACTION_REBUILD_INDEX_ID, ACTION_TRIM_MEMORY_ID,
    ACTION_WEB_SEARCH_PREFIX,
};
use crate::clipboard_history;
use crate::config::{self, Config, ConfigError};
use crate::core_service::{CoreService, LaunchTarget, ServiceError};
use crate::hotkey_runtime::HotkeyRuntimeError;
#[cfg(target_os = "windows")]
use crate::hotkey_runtime::{default_hotkey_registrar, HotkeyRegistration};
#[cfg(target_os = "windows")]
use crate::overlay_state::{HotkeyAction, OverlayState};
use crate::plugin_sdk::{PluginActionKind, PluginRegistry};
use crate::query_dsl::ParsedQuery;
use crate::search::SearchFilter;
#[cfg(target_os = "windows")]
use crate::windows_overlay::{
    is_instance_window_present, signal_existing_instance_quit, signal_existing_instance_show,
    NativeOverlayShell, OverlayEvent, OverlayRow, OverlayRowRole,
};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};
use std::time::Instant;
#[cfg(target_os = "windows")]
use std::time::{Duration, SystemTime};

#[cfg(target_os = "windows")]
const STATUS_ROW_NO_RESULTS: &str = "No results";
#[cfg(target_os = "windows")]
const STATUS_ROW_NO_COMMAND_RESULTS: &str = "No command matches";
#[cfg(target_os = "windows")]
const STATUS_ROW_TYPE_TO_SEARCH: &str = "Start typing to search";
#[cfg(target_os = "windows")]
const STATUS_ROW_INDEXING: &str = "Indexing in background...";
#[cfg(target_os = "windows")]
const STATUS_TEXT_INDEX_READY: &str = "Index ready";
const QUERY_PROFILE_LOG_THRESHOLD_MS: u128 = 35;
const SHORT_QUERY_APP_BIAS_MAX_LEN: usize = 2;
const INDEXED_PREFIX_CACHE_MIN_QUERY_LEN: usize = 1;
const INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT: usize = 120;
const INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT: usize = 480;
const QUERY_PROFILE_STATUS_SAMPLE_WINDOW: usize = 400;
const FINAL_QUERY_CACHE_MAX_ENTRIES: usize = 32;
const ADAPTIVE_INDEXED_LATENCY_WINDOW: usize = 24;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
const UNINSTALL_QUERY_RESULT_LIMIT: usize = 160;
#[cfg(target_os = "windows")]
const CONFIG_RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(500);
const ACTION_UNINSTALL_CONFIRM_ID: &str = "action:uninstall:confirm";
const ACTION_UNINSTALL_CANCEL_ID: &str = "action:uninstall:cancel";
static STDIO_LOGGING_ENABLED: AtomicBool = AtomicBool::new(true);
#[cfg(target_os = "windows")]
const CURRENT_RUNTIME_EXE_NAME: &str = "nex.exe";
#[cfg(target_os = "windows")]
const LEGACY_RUNTIME_EXE_NAMES: &[&str] = &["nex-core.exe", "swiftfind-core.exe"];
const CURRENT_LOG_PREFIX: &str = "[nex]";
const LEGACY_LOG_PREFIXES: &[&str] = &["[nex-core]", "[swiftfind-core]"];

fn env_var_with_legacy(current: &str, legacy: &str) -> Result<String, std::env::VarError> {
    std::env::var(current).or_else(|_| std::env::var(legacy))
}

#[cfg(target_os = "windows")]
fn runtime_executable_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(CURRENT_RUNTIME_EXE_NAME).chain(LEGACY_RUNTIME_EXE_NAMES.iter().copied())
}

fn runtime_log_prefixes() -> impl Iterator<Item = &'static str> {
    std::iter::once(CURRENT_LOG_PREFIX).chain(LEGACY_LOG_PREFIXES.iter().copied())
}

fn runtime_log_marker(prefix: &str, marker: &str) -> String {
    format!("{prefix} {marker}")
}

fn rfind_runtime_log_marker(content: &str, marker: &str) -> Option<usize> {
    runtime_log_prefixes()
        .filter_map(|prefix| content.rfind(&runtime_log_marker(prefix, marker)))
        .max()
}

fn line_contains_runtime_log_marker(line: &str, marker: &str) -> bool {
    runtime_log_prefixes().any(|prefix| line.contains(&runtime_log_marker(prefix, marker)))
}

#[derive(Debug, Clone, Default)]
struct OverlaySearchSession {
    indexed_prefix_cache: Option<IndexedPrefixCache>,
    final_query_cache: HashMap<String, Vec<crate::model::SearchItem>>,
    final_query_cache_lru: VecDeque<String>,
    indexed_latency_ms: VecDeque<u128>,
}

impl OverlaySearchSession {
    fn clear(&mut self) {
        self.indexed_prefix_cache = None;
        self.final_query_cache.clear();
        self.final_query_cache_lru.clear();
        self.indexed_latency_ms.clear();
    }
}

#[derive(Debug, Clone)]
struct IndexedPrefixCache {
    normalized_query: String,
    indexed_filter: SearchFilter,
    seed_items: Vec<crate::model::SearchItem>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct PendingUninstallConfirmation {
    uninstall_action: crate::model::SearchItem,
    previous_results: Vec<crate::model::SearchItem>,
    previous_selected_index: usize,
    previous_command_mode: bool,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct RuntimeConfigWatcher {
    path: std::path::PathBuf,
    last_checked: Instant,
    last_modified: Option<SystemTime>,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct BackgroundIndexRefresh {
    completed: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<crate::core_service::IndexRefreshReport, String>>>>,
    cache_applied: bool,
    initial_cache_empty: bool,
    pending_discovery_reindex: bool,
    ready_notice_pending: bool,
    started_at: Instant,
    startup_started_at: Instant,
}

#[derive(Debug)]
pub enum RuntimeError {
    Args(String),
    Config(ConfigError),
    Service(ServiceError),
    Hotkey(HotkeyRuntimeError),
    Overlay(String),
    Startup(crate::startup::StartupError),
    Io(std::io::Error),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Args(error) => write!(f, "argument error: {error}"),
            Self::Config(error) => write!(f, "config error: {error}"),
            Self::Service(error) => write!(f, "service error: {error}"),
            Self::Hotkey(error) => write!(f, "hotkey runtime error: {error:?}"),
            Self::Overlay(error) => write!(f, "overlay error: {error}"),
            Self::Startup(error) => write!(f, "startup error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<ConfigError> for RuntimeError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<ServiceError> for RuntimeError {
    fn from(value: ServiceError) -> Self {
        Self::Service(value)
    }
}

impl From<HotkeyRuntimeError> for RuntimeError {
    fn from(value: HotkeyRuntimeError) -> Self {
        Self::Hotkey(value)
    }
}

impl From<crate::startup::StartupError> for RuntimeError {
    fn from(value: crate::startup::StartupError) -> Self {
        Self::Startup(value)
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCommand {
    Run,
    Status,
    StatusJson,
    Quit,
    Restart,
    EnsureConfig,
    SyncStartup,
    SetLaunchAtStartup(bool),
    DiagnosticsBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub command: RuntimeCommand,
    pub background: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            command: RuntimeCommand::Run,
            background: false,
        }
    }
}

pub fn parse_cli_args(args: &[String]) -> Result<RuntimeOptions, String> {
    let mut options = RuntimeOptions::default();
    for arg in args {
        if let Some(value) = arg.strip_prefix("--set-launch-at-startup=") {
            let enabled = match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => {
                    return Err(format!(
                        "invalid value for --set-launch-at-startup: {value} (expected true/false)"
                    ));
                }
            };
            options.command = RuntimeCommand::SetLaunchAtStartup(enabled);
            continue;
        }

        match arg.as_str() {
            "--background" => options.background = true,
            "--foreground" => options.background = false,
            "--status" => options.command = RuntimeCommand::Status,
            "--status-json" => options.command = RuntimeCommand::StatusJson,
            "--quit" => options.command = RuntimeCommand::Quit,
            "--restart" => options.command = RuntimeCommand::Restart,
            "--ensure-config" => options.command = RuntimeCommand::EnsureConfig,
            "--sync-startup" => options.command = RuntimeCommand::SyncStartup,
            "--diagnostics-bundle" => options.command = RuntimeCommand::DiagnosticsBundle,
            "--help" | "-h" => {
                return Err(
                    "usage: nex [--background|--foreground] [--status|--status-json|--quit|--restart|--ensure-config|--sync-startup|--set-launch-at-startup=true|false|--diagnostics-bundle]".to_string(),
                )
            }
            unknown => return Err(format!("unknown argument: {unknown}")),
        }
    }

    if options.command != RuntimeCommand::Run && options.background {
        return Err("background mode is only valid with normal run mode".to_string());
    }

    Ok(options)
}

pub fn run() -> Result<(), RuntimeError> {
    run_with_options(RuntimeOptions::default())
}

pub fn run_with_options(options: RuntimeOptions) -> Result<(), RuntimeError> {
    configure_stdio_logging(options);

    if let Err(error) = crate::logging::init() {
        log_warn(&format!("[nex] logging init warning: {error}"));
    }

    #[cfg(target_os = "windows")]
    if options.background && options.command == RuntimeCommand::Run {
        return spawn_background_process();
    }

    match options.command {
        RuntimeCommand::Status => return command_status(),
        RuntimeCommand::StatusJson => return command_status_json(),
        RuntimeCommand::Quit => return command_quit(),
        RuntimeCommand::Restart => return command_restart(),
        RuntimeCommand::EnsureConfig => return command_ensure_config(),
        RuntimeCommand::SyncStartup => return command_sync_startup(),
        RuntimeCommand::SetLaunchAtStartup(enabled) => {
            return command_set_launch_at_startup(enabled);
        }
        RuntimeCommand::DiagnosticsBundle => return command_diagnostics_bundle(),
        RuntimeCommand::Run => {}
    }

    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    let startup_started_at = Instant::now();
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut runtime_config = config::load(None)?;
    if !runtime_config.config_path.exists() {
        config::write_user_template(&runtime_config, &runtime_config.config_path)?;
        log_info(&format!(
            "[nex] wrote user config template to {}",
            runtime_config.config_path.display()
        ));
    }
    log_info(&format!(
        "[nex] startup mode={} hotkey={} config_path={} index_db_path={}",
        runtime_mode(),
        runtime_config.hotkey,
        runtime_config.config_path.display(),
        runtime_config.index_db_path.display(),
    ));

    let service = CoreService::new(runtime_config.clone())?.with_runtime_providers();
    #[cfg(target_os = "windows")]
    let mut background_index_refresh = {
        let initial_cached_items = service.cached_items_len();
        log_info(&format!(
            "[nex] startup cached_items={} (async indexing scheduled)",
            initial_cached_items
        ));
        log_info(&format!(
            "[nex] startup_phase phase=indexing_started elapsed_ms={} initial_cache_empty={} cached_items={}",
            startup_started_at.elapsed().as_millis(),
            initial_cached_items == 0,
            initial_cached_items
        ));
        start_background_index_refresh(
            &runtime_config,
            initial_cached_items == 0,
            startup_started_at,
        )
    };
    #[cfg(not(target_os = "windows"))]
    {
        let index_report = service.rebuild_index_incremental_with_report()?;
        log_info(&format!(
            "[nex] startup indexed_items={} discovered={} upserted={} removed={}",
            index_report.indexed_total,
            index_report.discovered_total,
            index_report.upserted_total,
            index_report.removed_total,
        ));
        for provider in &index_report.providers {
            log_info(&format!(
                "[nex] index_provider name={} discovered={} upserted={} removed={} skipped={} elapsed_ms={}",
                provider.provider,
                provider.discovered,
                provider.upserted,
                provider.removed,
                provider.skipped,
                provider.elapsed_ms,
            ));
        }
    }
    #[cfg(target_os = "windows")]
    let mut plugin_registry = PluginRegistry::load_from_config(&runtime_config);
    #[cfg(target_os = "windows")]
    {
        for warning in &plugin_registry.load_warnings {
            log_warn(&format!("[nex] plugin_warning {warning}"));
        }
        log_info(&format!(
            "[nex] plugins loaded provider_items={} action_items={}",
            plugin_registry.provider_items.len(),
            plugin_registry.action_items.len()
        ));
    }

    #[cfg(target_os = "windows")]
    {
        // Opt into per-monitor DPI awareness to avoid bitmap-scaled blur on high-DPI systems.
        unsafe {
            let _ = windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Err(error) = crate::startup::set_enabled(runtime_config.launch_at_startup, &exe)
            {
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

        let mut registrar = default_hotkey_registrar();
        let registration = registrar.register_hotkey(&runtime_config.hotkey)?;
        log_registration(&registration);
        log_info(&format!(
            "[nex] startup_phase phase=hotkey_ready elapsed_ms={} hotkey={}",
            startup_started_at.elapsed().as_millis(),
            runtime_config.hotkey
        ));
        log_info("[nex] event loop running (native overlay)");

        let mut max_results = runtime_config.max_results as usize;
        let mut config_watcher = RuntimeConfigWatcher {
            path: runtime_config.config_path.clone(),
            last_checked: Instant::now(),
            last_modified: config_file_modified_time(runtime_config.config_path.as_path()),
        };
        let mut current_results: Vec<crate::model::SearchItem> = Vec::new();
        let mut suppressed_uninstall_titles: Vec<String> = Vec::new();
        let mut pending_uninstall_confirmation: Option<PendingUninstallConfirmation> = None;
        let mut pending_delayed_query: Option<String> = None;
        let mut selected_index = 0_usize;
        let mut last_query = String::new();
        let mut search_session = OverlaySearchSession::default();

        overlay
            .run_message_loop_with_events(|event| {
                maybe_apply_runtime_config_reload(
                    &overlay,
                    &service,
                    &mut runtime_config,
                    &mut plugin_registry,
                    &mut search_session,
                    &mut pending_uninstall_confirmation,
                    &mut max_results,
                    &mut config_watcher,
                    &mut background_index_refresh,
                );
                maybe_apply_background_index_refresh(
                    &service,
                    &mut background_index_refresh,
                    &runtime_config,
                );
                maybe_show_background_index_ready_notice(&overlay, &mut background_index_refresh);
                match event {
                    OverlayEvent::Hotkey(_) => {
                        log_info("[nex] hotkey_event received");
                        let overlay_visible = overlay.is_visible();
                        overlay_state.set_visible(overlay_visible);
                        if !overlay_visible
                            && should_suppress_hotkey_for_game_mode(&runtime_config)
                        {
                            log_info(
                                "[nex] hotkey ignored because game mode is active for the foreground app",
                            );
                            return;
                        }
                        let action = overlay_state.on_hotkey(overlay.has_focus());
                        match action {
                            HotkeyAction::ShowAndFocus | HotkeyAction::FocusExisting => {
                                reconcile_suppressed_uninstall_titles(
                                    &mut suppressed_uninstall_titles,
                                );
                                overlay.show_and_focus();
                                if runtime_config.clipboard_enabled {
                                    let _ =
                                        clipboard_history::maybe_capture_latest(&runtime_config);
                                }
                                if overlay.query_text().trim().is_empty() {
                                    set_idle_overlay_state(&overlay);
                                    maybe_show_background_index_ready_notice(
                                        &overlay,
                                        &mut background_index_refresh,
                                    );
                                }
                            }
                            HotkeyAction::Hide => {
                                pending_delayed_query = None;
                                overlay.cancel_query_delay();
                                overlay.hide();
                                reset_overlay_session(
                                    &overlay,
                                    &mut current_results,
                                    &mut selected_index,
                                );
                                pending_uninstall_confirmation = None;
                                last_query.clear();
                                search_session.clear();
                                maybe_apply_background_index_refresh(
                                    &service,
                                    &mut background_index_refresh,
                                    &runtime_config,
                                );
                            }
                        }
                    }
                    OverlayEvent::ExternalShow => {
                        overlay_state.set_visible(overlay.is_visible());
                        reconcile_suppressed_uninstall_titles(&mut suppressed_uninstall_titles);
                        overlay.show_and_focus();
                        if runtime_config.clipboard_enabled {
                            let _ = clipboard_history::maybe_capture_latest(&runtime_config);
                        }
                        if overlay.query_text().trim().is_empty() {
                            set_idle_overlay_state(&overlay);
                            maybe_show_background_index_ready_notice(
                                &overlay,
                                &mut background_index_refresh,
                            );
                        }
                    }
                    OverlayEvent::ExternalQuit => {
                        pending_delayed_query = None;
                        overlay.cancel_query_delay();
                        overlay.hide_now();
                        last_query.clear();
                        search_session.clear();
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                        }
                    }
                    OverlayEvent::TrayToggleGameMode => {
                        toggle_game_mode_from_tray(&overlay, &mut runtime_config);
                    }
                    OverlayEvent::Escape => {
                        if overlay_state.on_escape() {
                            pending_delayed_query = None;
                            overlay.cancel_query_delay();
                            overlay.hide_now();
                            reset_overlay_session(
                                &overlay,
                                &mut current_results,
                                &mut selected_index,
                            );
                            pending_uninstall_confirmation = None;
                            last_query.clear();
                            search_session.clear();
                        }
                    }
                    OverlayEvent::QueryChanged(query) => {
                        if runtime_config.search_query_results_with_delay {
                            pending_delayed_query = Some(query);
                            overlay
                                .schedule_query_delay(runtime_config.search_delay_time_ms as u32);
                        } else {
                            pending_delayed_query = None;
                            overlay.cancel_query_delay();
                            apply_query_change(
                                query,
                                &overlay,
                                &service,
                                &runtime_config,
                                &plugin_registry,
                                max_results,
                                &background_index_refresh,
                                &mut search_session,
                                &mut pending_uninstall_confirmation,
                                &suppressed_uninstall_titles,
                                &mut current_results,
                                &mut selected_index,
                                &mut last_query,
                            );
                        }
                    }
                    OverlayEvent::QueryDelayElapsed => {
                        if let Some(query) = pending_delayed_query.take() {
                            apply_query_change(
                                query,
                                &overlay,
                                &service,
                                &runtime_config,
                                &plugin_registry,
                                max_results,
                                &background_index_refresh,
                                &mut search_session,
                                &mut pending_uninstall_confirmation,
                                &suppressed_uninstall_titles,
                                &mut current_results,
                                &mut selected_index,
                                &mut last_query,
                            );
                        }
                    }
                    OverlayEvent::MoveSelection(direction) => {
                        if current_results.is_empty() {
                            return;
                        }

                        selected_index =
                            next_selection_index(selected_index, current_results.len(), direction);
                        overlay.set_selected_index(selected_index);
                    }
                    OverlayEvent::Submit => {
                        if let Some(query) = pending_delayed_query.take() {
                            overlay.cancel_query_delay();
                            apply_query_change(
                                query,
                                &overlay,
                                &service,
                                &runtime_config,
                                &plugin_registry,
                                max_results,
                                &background_index_refresh,
                                &mut search_session,
                                &mut pending_uninstall_confirmation,
                                &suppressed_uninstall_titles,
                                &mut current_results,
                                &mut selected_index,
                                &mut last_query,
                            );
                        }
                        if current_results.is_empty() {
                            if overlay.query_text().trim().is_empty() {
                                set_idle_overlay_state(&overlay);
                                overlay.show_placeholder_hint(STATUS_ROW_TYPE_TO_SEARCH);
                            } else if should_show_indexing_status(&background_index_refresh) {
                                set_status_row_overlay_state(&overlay, STATUS_ROW_INDEXING);
                            } else {
                                let parsed_query = ParsedQuery::parse(
                                    overlay.query_text().trim(),
                                    runtime_config.search_dsl_enabled,
                                );
                                set_status_row_overlay_state(
                                    &overlay,
                                    if parsed_query.command_mode {
                                        STATUS_ROW_NO_COMMAND_RESULTS
                                    } else {
                                        STATUS_ROW_NO_RESULTS
                                    },
                                );
                            }
                            return;
                        }

                        if let Some(list_selection) = overlay.selected_index() {
                            selected_index = list_selection.min(current_results.len() - 1);
                        }

                        let selected = &current_results[selected_index];
                        if pending_uninstall_confirmation.is_some() {
                            let selected_id = selected.id.clone();
                            if selected_id == ACTION_UNINSTALL_CONFIRM_ID {
                                let Some(pending) = pending_uninstall_confirmation.take() else {
                                    return;
                                };
                                overlay.hide_now();
                                overlay_state.on_escape();
                                match execute_action_selection(
                                    &service,
                                    &runtime_config,
                                    &plugin_registry,
                                    &pending.uninstall_action,
                                ) {
                                    Ok(()) => {
                                        track_uninstall_title_suppression(
                                            &mut suppressed_uninstall_titles,
                                            pending.uninstall_action.title.as_str(),
                                        );
                                        overlay.set_status_text("");
                                        reset_overlay_session(
                                            &overlay,
                                            &mut current_results,
                                            &mut selected_index,
                                        );
                                        last_query.clear();
                                        search_session.clear();
                                    }
                                    Err(error) => {
                                        if should_suppress_failed_uninstall(error.as_str()) {
                                            track_uninstall_title_suppression(
                                                &mut suppressed_uninstall_titles,
                                                pending.uninstall_action.title.as_str(),
                                            );
                                            current_results = pending.previous_results;
                                            filter_suppressed_uninstall_results(
                                                &mut current_results,
                                                &suppressed_uninstall_titles,
                                            );
                                            selected_index = pending
                                                .previous_selected_index
                                                .min(current_results.len().saturating_sub(1));
                                            if current_results.is_empty() {
                                                set_status_row_overlay_state(
                                                    &overlay,
                                                    if pending.previous_command_mode {
                                                        STATUS_ROW_NO_COMMAND_RESULTS
                                                    } else {
                                                        STATUS_ROW_NO_RESULTS
                                                    },
                                                );
                                            } else {
                                                let rows = overlay_rows(
                                                    &current_results,
                                                    pending.previous_command_mode,
                                                );
                                                overlay.set_results(&rows, selected_index);
                                            }
                                            overlay.set_status_text(
                                                "Uninstall entry is stale and was hidden",
                                            );
                                        } else {
                                            pending_uninstall_confirmation = Some(pending);
                                            overlay.show_and_focus();
                                            overlay
                                                .set_status_text(&format!("Launch error: {error}"));
                                        }
                                    }
                                }
                                return;
                            }

                            if selected_id == ACTION_UNINSTALL_CANCEL_ID {
                                let Some(pending) = pending_uninstall_confirmation.take() else {
                                    return;
                                };
                                current_results = pending.previous_results;
                                selected_index = pending
                                    .previous_selected_index
                                    .min(current_results.len().saturating_sub(1));
                                if current_results.is_empty() {
                                    set_status_row_overlay_state(
                                        &overlay,
                                        if pending.previous_command_mode {
                                            STATUS_ROW_NO_COMMAND_RESULTS
                                        } else {
                                            STATUS_ROW_NO_RESULTS
                                        },
                                    );
                                } else {
                                    let rows = overlay_rows(
                                        &current_results,
                                        pending.previous_command_mode,
                                    );
                                    overlay.set_results(&rows, selected_index);
                                }
                                overlay.set_status_text("");
                                return;
                            }

                            pending_uninstall_confirmation = None;
                        }

                        let selected_is_uninstall = selected
                            .id
                            .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX);

                        if selected_is_uninstall {
                            let parsed_query = ParsedQuery::parse(
                                overlay.query_text().trim(),
                                runtime_config.search_dsl_enabled,
                            );
                            pending_uninstall_confirmation = Some(PendingUninstallConfirmation {
                                uninstall_action: selected.clone(),
                                previous_results: current_results.clone(),
                                previous_selected_index: selected_index,
                                previous_command_mode: parsed_query.command_mode,
                            });
                            current_results = uninstall_confirmation_results(selected);
                            selected_index = 0;
                            let rows = overlay_rows(&current_results, true);
                            overlay.set_results(&rows, selected_index);
                            overlay.set_status_text("");
                            return;
                        }

                        if selected.id == ACTION_TRIM_MEMORY_ID {
                            search_session.clear();
                            overlay.trim_runtime_memory();
                            overlay.set_status_text("Memory caches trimmed");
                            return;
                        }

                        match launch_overlay_selection(
                            &service,
                            &runtime_config,
                            &plugin_registry,
                            &current_results,
                            selected_index,
                            last_query.as_str(),
                        ) {
                            Ok(()) => {
                                overlay.set_status_text("");
                                overlay.hide_now();
                                overlay_state.on_escape();
                                reset_overlay_session(
                                    &overlay,
                                    &mut current_results,
                                    &mut selected_index,
                                );
                                pending_uninstall_confirmation = None;
                                last_query.clear();
                                search_session.clear();
                            }
                            Err(error) => {
                                overlay.set_status_text(&format!("Launch error: {error}"));
                            }
                        }
                    }
                }
            })
            .map_err(RuntimeError::Overlay)?;
        registrar.unregister_all()?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] non-windows runtime mode: no global hotkey loop");
        Ok(())
    }
}

fn command_ensure_config() -> Result<(), RuntimeError> {
    let cfg = config::load(None)?;
    if !cfg.config_path.exists() {
        config::write_user_template(&cfg, &cfg.config_path)?;
        log_info(&format!(
            "[nex] wrote user config template to {}",
            cfg.config_path.display()
        ));
    }
    log_info(&format!(
        "[nex] config ready at {}",
        cfg.config_path.display()
    ));
    Ok(())
}

fn command_sync_startup() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let cfg = config::load(None)?;
        let exe = std::env::current_exe()?;
        crate::startup::set_enabled(cfg.launch_at_startup, &exe)?;
        log_info(&format!(
            "[nex] startup registration synced: enabled={}",
            cfg.launch_at_startup
        ));
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] startup sync is unsupported on this platform");
        Ok(())
    }
}

fn command_set_launch_at_startup(enabled: bool) -> Result<(), RuntimeError> {
    let mut cfg = config::load(None)?;
    cfg.launch_at_startup = enabled;
    config::save(&cfg)?;

    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe()?;
        crate::startup::set_enabled(enabled, &exe)?;
    }

    log_info(&format!(
        "[nex] launch_at_startup updated: enabled={} (can be changed in config)",
        enabled
    ));
    Ok(())
}

fn command_status() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let state = inspect_runtime_process_state();
        let running = state.has_overlay_window;
        log_info(&format!(
            "[nex] status: {}",
            if running {
                "running"
            } else if !state.other_runtime_pids.is_empty() {
                "degraded (process without overlay window)"
            } else {
                "stopped"
            }
        ));
        if !state.other_runtime_pids.is_empty() {
            log_warn(&format!(
                "[nex] status detected runtime_pids_without_window={:?} recommendation=run --restart",
                state.other_runtime_pids
            ));
        }
        if let Some(snapshot) = load_status_diagnostics_snapshot() {
            if let Some(line) = snapshot.overlay_ready_line {
                log_info(&format!("[nex] status last_overlay_ready {line}"));
            }
            if let Some(line) = snapshot.hotkey_ready_line {
                log_info(&format!("[nex] status last_hotkey_ready {line}"));
            }
            if let Some(line) = snapshot.indexing_started_line {
                log_info(&format!("[nex] status last_indexing_started {line}"));
            }
            if let Some(line) = snapshot.indexing_completed_line {
                log_info(&format!("[nex] status last_indexing_completed {line}"));
            }
            if let Some(line) = snapshot.cache_applied_line {
                log_info(&format!("[nex] status last_cache_applied {line}"));
            }
            if let Some(line) = snapshot.startup_index_line {
                log_info(&format!("[nex] status last_indexing {line}"));
            }
            if let Some(line) = snapshot.last_provider_line {
                log_info(&format!("[nex] status last_provider {line}"));
            }
            if let Some(line) = snapshot.last_cache_compaction_line {
                log_info(&format!("[nex] status last_cache_compaction {line}"));
            }
            if let Some(line) = snapshot.last_icon_cache_line {
                log_info(&format!("[nex] status last_icon_cache {line}"));
            }
            if let Some(line) = snapshot.last_overlay_tuning_line {
                log_info(&format!(
                    "[nex] status last_overlay_tuning {line}"
                ));
            }
            if let Some(line) = snapshot.last_memory_snapshot_line {
                log_info(&format!(
                    "[nex] status last_memory_snapshot {line}"
                ));
            }
            if let Some(line) = snapshot.last_config_reload_line {
                log_info(&format!(
                    "[nex] status last_config_reload {line}"
                ));
            }
        }
        if let Some(report) = load_query_profile_status_report() {
            if let Some(recent) = report.recent {
                log_info(&format!(
                    "[nex] status query_latency_recent samples={} p50_ms={} p95_ms={} p99_ms={} max_ms={} avg_ms={} indexed_p95_ms={} short_q_samples={} short_q_p95_ms={} short_q_app_bias_rate={}%",
                    recent.samples,
                    recent.p50_total_ms,
                    recent.p95_total_ms,
                    recent.p99_total_ms,
                    recent.max_total_ms,
                    recent.avg_total_ms,
                    recent.p95_indexed_ms,
                    recent.short_query_samples,
                    recent.short_query_p95_total_ms,
                    recent.short_query_app_bias_rate_pct
                ));
            }
            if let Some(historical) = report.historical {
                log_info(&format!(
                    "[nex] status query_latency_historical samples={} p50_ms={} p95_ms={} p99_ms={} max_ms={} avg_ms={} indexed_p95_ms={} short_q_samples={} short_q_p95_ms={} short_q_app_bias_rate={}%",
                    historical.samples,
                    historical.p50_total_ms,
                    historical.p95_total_ms,
                    historical.p99_total_ms,
                    historical.max_total_ms,
                    historical.avg_total_ms,
                    historical.p95_indexed_ms,
                    historical.short_query_samples,
                    historical.short_query_p95_total_ms,
                    historical.short_query_app_bias_rate_pct
                ));
            }
            log_info(&format!(
                "[nex] status query_guard recent_skipped_symbol_queries={} historical_skipped_symbol_queries={}",
                report.recent_skipped_symbol_queries, report.historical_skipped_symbol_queries
            ));
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] status: unsupported on this platform");
        Ok(())
    }
}

fn command_status_json() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        let state = inspect_runtime_process_state();
        let lifecycle = if state.has_overlay_window {
            "running"
        } else if !state.other_runtime_pids.is_empty() {
            "degraded"
        } else {
            "stopped"
        };

        let snapshot = load_status_diagnostics_snapshot();
        let report = load_query_profile_status_report();
        let diagnostics = snapshot
            .as_ref()
            .map(build_status_diagnostics_json)
            .unwrap_or_else(|| serde_json::json!({}));

        let payload = serde_json::json!({
            "runtime_state": lifecycle,
            "has_overlay_window": state.has_overlay_window,
            "other_runtime_pids": state.other_runtime_pids,
            "diagnostics": diagnostics,
            "query_latency": report.map(query_profile_report_json),
        });
        let encoded = serde_json::to_string_pretty(&payload)
            .map_err(|error| RuntimeError::Args(format!("status-json encode error: {error}")))?;
        println!("{encoded}");
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let payload = serde_json::json!({
            "runtime_state": "unsupported_platform",
            "has_overlay_window": false,
            "other_runtime_pids": Vec::<u32>::new(),
            "diagnostics": serde_json::json!({}),
            "query_latency": serde_json::Value::Null,
        });
        let encoded = serde_json::to_string_pretty(&payload)
            .map_err(|error| RuntimeError::Args(format!("status-json encode error: {error}")))?;
        println!("{encoded}");
        Ok(())
    }
}

fn command_diagnostics_bundle() -> Result<(), RuntimeError> {
    let cfg = config::load(None)?;
    let output_dir = write_diagnostics_bundle(&cfg)?;
    log_info(&format!(
        "[nex] diagnostics bundle written to {}",
        output_dir.display()
    ));
    Ok(())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct StatusDiagnosticsSnapshot {
    overlay_ready_line: Option<String>,
    hotkey_ready_line: Option<String>,
    indexing_started_line: Option<String>,
    indexing_completed_line: Option<String>,
    cache_applied_line: Option<String>,
    startup_index_line: Option<String>,
    last_provider_line: Option<String>,
    last_cache_compaction_line: Option<String>,
    last_icon_cache_line: Option<String>,
    last_overlay_tuning_line: Option<String>,
    last_memory_snapshot_line: Option<String>,
    last_config_reload_line: Option<String>,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueryProfileSample {
    total_ms: u128,
    indexed_ms: u128,
    query_len: usize,
    short_app_bias: bool,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueryProfileSummary {
    samples: usize,
    p50_total_ms: u128,
    p95_total_ms: u128,
    p99_total_ms: u128,
    max_total_ms: u128,
    avg_total_ms: u128,
    p95_indexed_ms: u128,
    short_query_samples: usize,
    short_query_p95_total_ms: u128,
    short_query_app_bias_rate_pct: u8,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryProfileStatusReport {
    recent: Option<QueryProfileSummary>,
    historical: Option<QueryProfileSummary>,
    recent_skipped_symbol_queries: usize,
    historical_skipped_symbol_queries: usize,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn load_status_diagnostics_snapshot() -> Option<StatusDiagnosticsSnapshot> {
    let content = crate::logging::candidate_log_paths()
        .into_iter()
        .find_map(|log_path| std::fs::read_to_string(log_path).ok())?;
    parse_status_diagnostics_snapshot(&content)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn load_query_profile_status_report() -> Option<QueryProfileStatusReport> {
    let content = crate::logging::candidate_log_paths()
        .into_iter()
        .find_map(|log_path| std::fs::read_to_string(log_path).ok())?;
    summarize_query_profile_status_report(&content)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_status_diagnostics_snapshot(content: &str) -> Option<StatusDiagnosticsSnapshot> {
    let overlay_ready_line = latest_line_with_token(content, "startup_phase phase=overlay_ready ");
    let hotkey_ready_line = latest_line_with_token(content, "startup_phase phase=hotkey_ready ");
    let indexing_started_line =
        latest_line_with_token(content, "startup_phase phase=indexing_started ");
    let indexing_completed_line =
        latest_line_with_token(content, "startup_phase phase=indexing_completed ");
    let cache_applied_line = latest_line_with_token(content, "startup_phase phase=cache_applied ");
    let startup_index_line = latest_line_with_token(content, "startup indexed_items=");
    let last_provider_line = latest_line_with_token(content, "index_provider name=");
    let last_cache_compaction_line = latest_line_with_token(content, "cache_compaction ");
    let last_icon_cache_line = latest_line_with_token(content, "overlay_icon_cache reason=");
    let last_overlay_tuning_line = latest_line_with_token(content, "overlay_tuning ");
    let last_memory_snapshot_line = latest_line_with_token(content, "memory_snapshot reason=");
    let last_config_reload_line = latest_line_with_token(content, "config reloaded ");

    if overlay_ready_line.is_none()
        && hotkey_ready_line.is_none()
        && indexing_started_line.is_none()
        && indexing_completed_line.is_none()
        && cache_applied_line.is_none()
        && startup_index_line.is_none()
        && last_provider_line.is_none()
        && last_cache_compaction_line.is_none()
        && last_icon_cache_line.is_none()
        && last_overlay_tuning_line.is_none()
        && last_memory_snapshot_line.is_none()
        && last_config_reload_line.is_none()
    {
        return None;
    }

    Some(StatusDiagnosticsSnapshot {
        overlay_ready_line,
        hotkey_ready_line,
        indexing_started_line,
        indexing_completed_line,
        cache_applied_line,
        startup_index_line,
        last_provider_line,
        last_cache_compaction_line,
        last_icon_cache_line,
        last_overlay_tuning_line,
        last_memory_snapshot_line,
        last_config_reload_line,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn latest_line_with_token(content: &str, token: &str) -> Option<String> {
    content
        .lines()
        .rev()
        .find(|line| line.contains(token))
        .map(str::to_string)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn summarize_query_profile_status_report(content: &str) -> Option<QueryProfileStatusReport> {
    let recent_samples = parse_recent_query_profile_samples(content);
    let historical_samples = parse_query_profile_samples(content);
    let recent = summarize_query_profile_samples(&recent_samples);
    let historical = summarize_query_profile_samples(&historical_samples);
    if recent.is_none() && historical.is_none() {
        return None;
    }

    let recent_lines = recent_runtime_log_slice(content);
    let recent_skipped_symbol_queries = count_skipped_symbol_query_guards(recent_lines);
    let historical_skipped_symbol_queries = count_skipped_symbol_query_guards(content);

    Some(QueryProfileStatusReport {
        recent,
        historical,
        recent_skipped_symbol_queries,
        historical_skipped_symbol_queries,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_status_diagnostics_json(snapshot: &StatusDiagnosticsSnapshot) -> serde_json::Value {
    let overlay_ready = build_phase_status_json(snapshot.overlay_ready_line.as_ref());
    let hotkey_ready = build_phase_status_json(snapshot.hotkey_ready_line.as_ref());
    let indexing_started = build_phase_status_json(snapshot.indexing_started_line.as_ref());
    let indexing_completed = build_phase_status_json(snapshot.indexing_completed_line.as_ref());
    let cache_applied = build_phase_status_json(snapshot.cache_applied_line.as_ref());
    let startup_indexing = snapshot
        .startup_index_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let provider = snapshot
        .last_provider_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let cache_compaction = snapshot
        .last_cache_compaction_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let icon_cache = snapshot
        .last_icon_cache_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let overlay_tuning = snapshot
        .last_overlay_tuning_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let memory_snapshot = snapshot
        .last_memory_snapshot_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let config_reload = snapshot
        .last_config_reload_line
        .as_ref()
        .and_then(|line| parse_key_value_tokens(line));
    let config_reload_epoch_secs = snapshot
        .last_config_reload_line
        .as_ref()
        .and_then(|line| parse_log_line_epoch_secs(line));

    serde_json::json!({
        "startup_lifecycle": {
            "overlay_ready": overlay_ready,
            "hotkey_ready": hotkey_ready,
            "indexing_started": indexing_started,
            "indexing_completed": indexing_completed,
            "cache_applied": cache_applied,
        },
        "startup_indexing": startup_indexing,
        "provider": provider,
        "cache_compaction": cache_compaction,
        "icon_cache": icon_cache,
        "overlay_tuning": overlay_tuning,
        "memory_snapshot": memory_snapshot,
        "config_reload": config_reload,
        "config_reload_epoch_secs": config_reload_epoch_secs,
        "raw": {
            "overlay_ready_line": snapshot.overlay_ready_line,
            "hotkey_ready_line": snapshot.hotkey_ready_line,
            "indexing_started_line": snapshot.indexing_started_line,
            "indexing_completed_line": snapshot.indexing_completed_line,
            "cache_applied_line": snapshot.cache_applied_line,
            "startup_indexing_line": snapshot.startup_index_line,
            "provider_line": snapshot.last_provider_line,
            "cache_compaction_line": snapshot.last_cache_compaction_line,
            "icon_cache_line": snapshot.last_icon_cache_line,
            "overlay_tuning_line": snapshot.last_overlay_tuning_line,
            "memory_snapshot_line": snapshot.last_memory_snapshot_line,
            "config_reload_line": snapshot.last_config_reload_line,
        }
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_phase_status_json(line: Option<&String>) -> serde_json::Value {
    let tokens = line.and_then(|value| parse_key_value_tokens(value));
    let epoch_secs = line.and_then(|value| parse_log_line_epoch_secs(value));
    serde_json::json!({
        "tokens": tokens,
        "epoch_secs": epoch_secs,
        "line": line.cloned(),
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn query_profile_report_json(report: QueryProfileStatusReport) -> serde_json::Value {
    serde_json::json!({
        "recent": report.recent.map(query_profile_summary_json),
        "historical": report.historical.map(query_profile_summary_json),
        "recent_skipped_symbol_queries": report.recent_skipped_symbol_queries,
        "historical_skipped_symbol_queries": report.historical_skipped_symbol_queries,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn query_profile_summary_json(summary: QueryProfileSummary) -> serde_json::Value {
    serde_json::json!({
        "samples": summary.samples,
        "p50_total_ms": summary.p50_total_ms,
        "p95_total_ms": summary.p95_total_ms,
        "p99_total_ms": summary.p99_total_ms,
        "max_total_ms": summary.max_total_ms,
        "avg_total_ms": summary.avg_total_ms,
        "p95_indexed_ms": summary.p95_indexed_ms,
        "short_query_samples": summary.short_query_samples,
        "short_query_p95_total_ms": summary.short_query_p95_total_ms,
        "short_query_app_bias_rate_pct": summary.short_query_app_bias_rate_pct,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_log_line_epoch_secs(line: &str) -> Option<u64> {
    let trimmed = line.trim();
    let start = trimmed.find('[')? + 1;
    let end = trimmed[start..].find(']')? + start;
    trimmed[start..end].parse::<u64>().ok()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_key_value_tokens(line: &str) -> Option<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for token in line.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_end_matches(':');
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_end_matches(',');
        if value.is_empty() {
            continue;
        }
        if let Ok(number) = value.parse::<u64>() {
            map.insert(key.to_string(), serde_json::json!(number));
            continue;
        }
        if let Ok(number) = value.parse::<f64>() {
            map.insert(key.to_string(), serde_json::json!(number));
            continue;
        }
        if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
            map.insert(
                key.to_string(),
                serde_json::json!(value.eq_ignore_ascii_case("true")),
            );
            continue;
        }
        map.insert(
            key.to_string(),
            serde_json::json!(value.trim_matches('"').to_string()),
        );
    }
    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

#[cfg(test)]
fn summarize_query_profiles(content: &str) -> Option<QueryProfileSummary> {
    let samples = parse_recent_query_profile_samples(content);
    summarize_query_profile_samples(&samples)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn summarize_query_profile_samples(samples: &[QueryProfileSample]) -> Option<QueryProfileSummary> {
    let mut samples = samples.to_vec();
    if samples.is_empty() {
        return None;
    }

    if samples.len() > QUERY_PROFILE_STATUS_SAMPLE_WINDOW {
        samples.drain(0..(samples.len() - QUERY_PROFILE_STATUS_SAMPLE_WINDOW));
    }
    if samples.is_empty() {
        return None;
    }

    let mut total_ms: Vec<u128> = samples.iter().map(|sample| sample.total_ms).collect();
    let mut indexed_ms: Vec<u128> = samples.iter().map(|sample| sample.indexed_ms).collect();
    let max_total_ms = total_ms.iter().copied().max().unwrap_or(0);
    let avg_total_ms = total_ms.iter().sum::<u128>() / (total_ms.len() as u128);
    let p50_total_ms = percentile_u128(&mut total_ms, 0.50);
    let p95_total_ms = percentile_u128(&mut total_ms, 0.95);
    let p99_total_ms = percentile_u128(&mut total_ms, 0.99);
    let p95_indexed_ms = percentile_u128(&mut indexed_ms, 0.95);

    let short_query_samples: Vec<QueryProfileSample> = samples
        .iter()
        .copied()
        .filter(|sample| sample.query_len <= SHORT_QUERY_APP_BIAS_MAX_LEN)
        .collect();
    let short_query_samples_count = short_query_samples.len();
    let mut short_total_ms: Vec<u128> = short_query_samples
        .iter()
        .map(|sample| sample.total_ms)
        .collect();
    let short_query_p95_total_ms = percentile_u128(&mut short_total_ms, 0.95);
    let short_query_app_bias_count = short_query_samples
        .iter()
        .filter(|sample| sample.short_app_bias)
        .count();
    let short_query_app_bias_rate_pct = if short_query_samples_count == 0 {
        0
    } else {
        ((short_query_app_bias_count * 100) / short_query_samples_count) as u8
    };

    Some(QueryProfileSummary {
        samples: samples.len(),
        p50_total_ms,
        p95_total_ms,
        p99_total_ms,
        max_total_ms,
        avg_total_ms,
        p95_indexed_ms,
        short_query_samples: short_query_samples_count,
        short_query_p95_total_ms,
        short_query_app_bias_rate_pct,
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn recent_runtime_log_slice(content: &str) -> &str {
    let Some(pos) = rfind_runtime_log_marker(content, "startup mode=") else {
        return content;
    };
    let line_start = content[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(pos);
    &content[line_start..]
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn count_skipped_symbol_query_guards(content: &str) -> usize {
    content
        .lines()
        .filter(|line| line.contains("query_guard skip=non_searchable_symbol_only"))
        .count()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_recent_query_profile_samples(content: &str) -> Vec<QueryProfileSample> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let start_index = lines
        .iter()
        .rposition(|line| line_contains_runtime_log_marker(line, "startup mode="))
        .unwrap_or(0);
    parse_query_profile_samples(&lines[start_index..].join("\n"))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_query_profile_samples(content: &str) -> Vec<QueryProfileSample> {
    content
        .lines()
        .filter(|line| line_contains_runtime_log_marker(line, "query_profile "))
        .filter_map(|line| {
            let total_ms = parse_u128_field(line, "total_ms=")?;
            let indexed_ms = parse_u128_field(line, "indexed_ms=").unwrap_or(0);
            let query = parse_quoted_field(line, "q=").unwrap_or_default();
            let query_len = query.chars().count();
            let short_app_bias = parse_bool_field(line, "short_app_bias=").unwrap_or(false);
            Some(QueryProfileSample {
                total_ms,
                indexed_ms,
                query_len,
                short_app_bias,
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_u128_field(line: &str, key: &str) -> Option<u128> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let value = tail
        .split_whitespace()
        .next()
        .map(|part| part.trim_end_matches(','))
        .unwrap_or_default();
    value.parse::<u128>().ok()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_bool_field(line: &str, key: &str) -> Option<bool> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let value = tail
        .split_whitespace()
        .next()
        .map(|part| part.trim_end_matches(','))
        .unwrap_or_default();
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_quoted_field(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    if !tail.starts_with('"') {
        return None;
    }
    let end = tail[1..].find('"')?;
    Some(tail[1..(1 + end)].to_string())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn percentile_u128(values: &mut [u128], percentile: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let last = values.len().saturating_sub(1);
    let idx = ((last as f64) * percentile.clamp(0.0, 1.0)).round() as usize;
    values[idx.min(last)]
}

fn command_quit() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        match stop_runtime_instance(std::time::Duration::from_secs(3))? {
            StopRuntimeOutcome::AlreadyStopped => {
                log_info("[nex] quit skipped (not running)");
                Ok(())
            }
            StopRuntimeOutcome::Graceful => {
                log_info("[nex] quit completed (graceful)");
                Ok(())
            }
            StopRuntimeOutcome::Forced => {
                log_warn("[nex] quit required forced process termination");
                Ok(())
            }
            StopRuntimeOutcome::Failed => Err(RuntimeError::Overlay(
                "quit failed: runtime is still active after graceful and forced attempts"
                    .to_string(),
            )),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        log_info("[nex] quit is unsupported on this platform");
        Ok(())
    }
}

fn command_restart() -> Result<(), RuntimeError> {
    #[cfg(target_os = "windows")]
    {
        match stop_runtime_instance(std::time::Duration::from_secs(3))? {
            StopRuntimeOutcome::Failed => {
                return Err(RuntimeError::Overlay(
                    "restart failed: existing runtime could not be stopped".to_string(),
                ));
            }
            StopRuntimeOutcome::Forced => {
                log_warn("[nex] restart required forced process termination");
            }
            StopRuntimeOutcome::Graceful | StopRuntimeOutcome::AlreadyStopped => {}
        }
        run_with_options(RuntimeOptions::default())
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_with_options(RuntimeOptions::default())
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeProcessState {
    has_overlay_window: bool,
    other_runtime_pids: Vec<u32>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopRuntimeOutcome {
    AlreadyStopped,
    Graceful,
    Forced,
    Failed,
}

#[cfg(target_os = "windows")]
fn inspect_runtime_process_state() -> RuntimeProcessState {
    RuntimeProcessState {
        has_overlay_window: is_instance_window_present(),
        other_runtime_pids: runtime_process_pids_excluding_current().unwrap_or_default(),
    }
}

#[cfg(target_os = "windows")]
fn stop_runtime_instance(timeout: std::time::Duration) -> Result<StopRuntimeOutcome, RuntimeError> {
    let mut state = inspect_runtime_process_state();
    if !state.has_overlay_window && state.other_runtime_pids.is_empty() {
        return Ok(StopRuntimeOutcome::AlreadyStopped);
    }

    if state.has_overlay_window {
        let _ = signal_existing_instance_quit().map_err(RuntimeError::Overlay)?;
        if wait_until_overlay_window_closed(timeout) {
            state = inspect_runtime_process_state();
            if state.other_runtime_pids.is_empty() {
                return Ok(StopRuntimeOutcome::Graceful);
            }
        }
    }

    let forced = force_terminate_other_runtime_processes()?;
    std::thread::sleep(std::time::Duration::from_millis(250));
    let post = inspect_runtime_process_state();
    if !post.has_overlay_window && post.other_runtime_pids.is_empty() {
        if forced {
            Ok(StopRuntimeOutcome::Forced)
        } else {
            Ok(StopRuntimeOutcome::Graceful)
        }
    } else {
        Ok(StopRuntimeOutcome::Failed)
    }
}

#[cfg(target_os = "windows")]
fn wait_until_overlay_window_closed(timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !is_instance_window_present() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    !is_instance_window_present()
}

#[cfg(target_os = "windows")]
fn force_terminate_other_runtime_processes() -> Result<bool, RuntimeError> {
    let current_pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let mut terminated_any = false;
    for exe_name in runtime_executable_names() {
        let command = format!(
            "taskkill /F /T /FI \"IMAGENAME eq {exe_name}\" /FI \"PID ne {}\" >NUL 2>&1",
            current_pid
        );
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg(command)
            .status()
            .map_err(RuntimeError::Io)?;
        terminated_any |= status.success();
    }
    Ok(terminated_any)
}

#[cfg(target_os = "windows")]
fn runtime_process_pids_excluding_current() -> Result<Vec<u32>, RuntimeError> {
    let current_pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let mut pids = Vec::new();
    for exe_name in runtime_executable_names() {
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg(format!("tasklist /FI \"IMAGENAME eq {exe_name}\" /FO LIST /NH"))
            .output()
            .map_err(RuntimeError::Io)?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        pids.extend(parse_tasklist_pid_lines(&stdout));
    }
    pids.retain(|pid| *pid != current_pid);
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_tasklist_pid_lines(content: &str) -> Vec<u32> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.to_ascii_lowercase().starts_with("pid:") {
                return None;
            }
            let value = trimmed.split(':').nth(1)?.trim();
            value.parse::<u32>().ok()
        })
        .collect()
}

fn write_diagnostics_bundle(cfg: &config::Config) -> Result<std::path::PathBuf, RuntimeError> {
    let support_dir = config::stable_app_data_dir().join("support");
    std::fs::create_dir_all(&support_dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bundle_dir = support_dir.join(format!("diagnostics-{stamp}"));
    std::fs::create_dir_all(&bundle_dir)?;

    let running_state = runtime_state_summary();
    let summary = format!(
        "nex diagnostics bundle\ngenerated_epoch_secs={stamp}\nruntime_state={running_state}\nconfig_path={}\nindex_db_path={}\nlogs_dir={}\n",
        cfg.config_path.display(),
        cfg.index_db_path.display(),
        crate::logging::logs_dir().display()
    );
    std::fs::write(bundle_dir.join("summary.txt"), summary)?;

    if cfg.config_path.exists() {
        let raw_ext = cfg
            .config_path
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|ext| !ext.trim().is_empty())
            .unwrap_or("txt");
        let _ = std::fs::copy(
            &cfg.config_path,
            bundle_dir.join(format!("config.raw.{raw_ext}")),
        );
    }

    let sanitized_cfg = serde_json::json!({
        "version": cfg.version,
        "max_results": cfg.max_results,
        "hotkey": cfg.hotkey,
        "launch_at_startup": cfg.launch_at_startup,
        "search_mode_default": cfg.search_mode_default,
        "search_dsl_enabled": cfg.search_dsl_enabled,
        "search_query_results_with_delay": cfg.search_query_results_with_delay,
        "search_delay_time_ms": cfg.search_delay_time_ms,
        "uninstall_actions_enabled": cfg.uninstall_actions_enabled,
        "web_search_provider": cfg.web_search_provider,
        "clipboard_enabled": cfg.clipboard_enabled,
        "clipboard_retention_minutes": cfg.clipboard_retention_minutes,
        "clipboard_exclude_sensitive_patterns_count": cfg.clipboard_exclude_sensitive_patterns.len(),
        "plugins_enabled": cfg.plugins_enabled,
        "plugin_paths_count": cfg.plugin_paths.len(),
        "plugins_safe_mode": cfg.plugins_safe_mode,
        "game_mode_enabled": cfg.game_mode_enabled,
        "idle_cache_trim_ms": cfg.idle_cache_trim_ms,
        "active_memory_target_mb": cfg.active_memory_target_mb,
        "index_max_items_total": cfg.index_max_items_total,
        "index_max_items_per_root": cfg.index_max_items_per_root,
        "index_max_items_per_query_seed": cfg.index_max_items_per_query_seed,
        "discovery_roots_count": cfg.discovery_roots.len(),
        "discovery_exclude_roots_count": cfg.discovery_exclude_roots.len(),
        "windows_search_enabled": cfg.windows_search_enabled,
        "windows_search_fallback_filesystem": cfg.windows_search_fallback_filesystem,
        "show_files": cfg.show_files,
        "show_folders": cfg.show_folders
    });
    let encoded = serde_json::to_string_pretty(&sanitized_cfg)
        .map_err(|e| RuntimeError::Args(format!("failed to encode sanitized config: {e}")))?;
    std::fs::write(bundle_dir.join("config.sanitized.json"), encoded)?;

    copy_recent_logs_to_bundle(&crate::logging::logs_dir(), &bundle_dir.join("logs"))?;

    Ok(bundle_dir)
}

fn copy_recent_logs_to_bundle(
    source_logs_dir: &std::path::Path,
    target_logs_dir: &std::path::Path,
) -> Result<(), RuntimeError> {
    std::fs::create_dir_all(target_logs_dir)?;
    if !source_logs_dir.exists() {
        return Ok(());
    }

    let mut entries = std::fs::read_dir(source_logs_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    entries.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    entries.reverse();

    for path in entries.into_iter().take(5) {
        if let Some(name) = path.file_name() {
            let _ = std::fs::copy(&path, target_logs_dir.join(name));
        }
    }

    Ok(())
}

fn runtime_state_summary() -> String {
    #[cfg(target_os = "windows")]
    {
        let state = inspect_runtime_process_state();
        if state.has_overlay_window {
            return "running".to_string();
        }
        if !state.other_runtime_pids.is_empty() {
            return format!(
                "degraded(process_without_overlay_window pids={:?})",
                state.other_runtime_pids
            );
        }
        "stopped".to_string()
    }

    #[cfg(not(target_os = "windows"))]
    {
        "unsupported_platform".to_string()
    }
}

#[cfg(target_os = "windows")]
fn spawn_background_process() -> Result<(), RuntimeError> {
    use std::os::windows::process::CommandExt;

    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command.arg("--foreground");
    command.env("NEX_SUPPRESS_STDIO", "1");
    command.creation_flags(0x00000008 | 0x00000200 | 0x08000000);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    command.spawn()?;
    log_info("[nex] background process started");
    Ok(())
}

fn runtime_mode() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "windows-hotkey-runtime"
    }

    #[cfg(not(target_os = "windows"))]
    {
        "non-windows-noop"
    }
}

#[cfg(target_os = "windows")]
fn overlay_rows(results: &[crate::model::SearchItem], command_mode: bool) -> Vec<OverlayRow> {
    if results.is_empty() {
        return Vec::new();
    }

    if command_mode {
        return results
            .iter()
            .enumerate()
            .map(|(index, item)| result_row(item, index, OverlayRowRole::Item, command_mode))
            .collect();
    }

    let mut rows = Vec::new();
    rows.push(result_row(
        &results[0],
        0,
        OverlayRowRole::TopHit,
        command_mode,
    ));

    let mut app_indices = Vec::new();
    let mut file_indices = Vec::new();
    let mut action_indices = Vec::new();
    let mut clipboard_indices = Vec::new();
    let mut other_indices = Vec::new();

    for (index, item) in results.iter().enumerate().skip(1) {
        if item.kind.eq_ignore_ascii_case("app") {
            app_indices.push(index);
        } else if item.kind.eq_ignore_ascii_case("file") || item.kind.eq_ignore_ascii_case("folder")
        {
            file_indices.push(index);
        } else if item.kind.eq_ignore_ascii_case("action") {
            action_indices.push(index);
        } else if item.kind.eq_ignore_ascii_case("clipboard") {
            clipboard_indices.push(index);
        } else {
            other_indices.push(index);
        }
    }

    append_group_rows(&mut rows, &app_indices, results, command_mode);
    append_group_rows(&mut rows, &file_indices, results, command_mode);
    append_group_rows(&mut rows, &action_indices, results, command_mode);
    append_group_rows(&mut rows, &clipboard_indices, results, command_mode);
    append_group_rows(&mut rows, &other_indices, results, command_mode);
    rows
}

#[cfg(target_os = "windows")]
fn append_group_rows(
    rows: &mut Vec<OverlayRow>,
    indices: &[usize],
    results: &[crate::model::SearchItem],
    command_mode: bool,
) {
    if indices.is_empty() {
        return;
    }
    for index in indices {
        rows.push(result_row(
            &results[*index],
            *index,
            OverlayRowRole::Item,
            command_mode,
        ));
    }
}

#[cfg(target_os = "windows")]
fn result_row(
    item: &crate::model::SearchItem,
    result_index: usize,
    role: OverlayRowRole,
    command_mode: bool,
) -> OverlayRow {
    OverlayRow {
        role,
        result_index: result_index as i32,
        kind: item.kind.clone(),
        title: item.title.clone(),
        path: overlay_subtitle(item, command_mode),
        icon_path: item.path.clone(),
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn dedupe_overlay_results(results: &mut Vec<crate::model::SearchItem>) {
    let app_title_keys: std::collections::HashSet<String> = results
        .iter()
        .filter(|item| item.kind.eq_ignore_ascii_case("app"))
        .filter(|item| !should_hide_known_start_menu_doc_sample_entry(item))
        .filter_map(|item| {
            let key = normalize_title_key(&item.title);
            if key.is_empty() {
                None
            } else {
                Some(key)
            }
        })
        .collect();

    let mut seen_app_titles = std::collections::HashSet::new();
    let mut seen_other_paths = std::collections::HashSet::new();

    results.retain(|item| {
        if item.kind.eq_ignore_ascii_case("app") {
            if should_hide_known_start_menu_doc_sample_entry(item) {
                return false;
            }
            let key = normalize_title_key(&item.title);
            if key.is_empty() {
                return true;
            }
            return seen_app_titles.insert(key);
        }

        if item.kind.eq_ignore_ascii_case("file")
            && is_windows_shortcut_path(&item.path)
            && app_title_keys.contains(&shortcut_base_title_key(&item.title))
        {
            return false;
        }

        let key = normalize_path_key(&item.path);
        if key.is_empty() {
            return true;
        }
        seen_other_paths.insert(key)
    });
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn should_hide_known_start_menu_doc_sample_entry(item: &crate::model::SearchItem) -> bool {
    if !item.kind.eq_ignore_ascii_case("app") {
        return false;
    }

    let lower = item.title.trim().to_ascii_lowercase();
    let path_lower = item.path.trim().replace('/', "\\").to_ascii_lowercase();
    let is_shell_appsfolder = path_lower.starts_with("shell:appsfolder\\");

    if path_lower.contains("\\windows kits\\10\\shortcuts\\") && path_lower.ends_with(".url") {
        return true;
    }
    if has_non_app_document_extension(path_lower.as_str()) {
        return true;
    }
    if is_shell_appsfolder && path_lower.contains("://") {
        return true;
    }

    if lower.is_empty() {
        return false;
    }
    if has_non_app_document_extension(lower.as_str()) {
        return true;
    }

    let has_docs = lower.contains("documentation") || lower.contains(" docs");
    let has_sample = lower.contains("sample");
    let has_tools_for = lower.contains("tools for");
    let has_help_content = lower.contains("manual")
        || lower.contains("faq")
        || lower.contains("website")
        || lower.contains("web page")
        || lower.contains("webpage")
        || lower.contains("guide")
        || lower.contains("readme")
        || lower.contains("release notes")
        || lower.contains("changelog");
    let has_apps = lower.contains(" app") || lower.contains("apps");
    let has_platform =
        lower.contains("desktop") || lower.contains("uwp") || lower.contains("winui");

    (has_docs && has_apps)
        || (has_sample && (has_apps || has_platform))
        || (has_tools_for && has_apps && has_platform)
        || (has_help_content && (path_lower.ends_with(".lnk") || is_shell_appsfolder))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn has_non_app_document_extension(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    [
        ".url", ".pdf", ".htm", ".html", ".xhtml", ".mht", ".mhtml", ".chm", ".txt", ".md", ".rtf",
        ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".csv", ".xml", ".json", ".yaml",
        ".yml", ".ini", ".log", ".php",
    ]
    .iter()
    .any(|ext| normalized.ends_with(ext))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn normalize_title_key(title: &str) -> String {
    crate::model::normalize_for_search(title.trim())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn shortcut_base_title_key(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.len() >= 4 && trimmed[trimmed.len() - 4..].eq_ignore_ascii_case(".lnk") {
        normalize_title_key(&trimmed[..trimmed.len() - 4])
    } else {
        normalize_title_key(trimmed)
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_windows_shortcut_path(path: &str) -> bool {
    let trimmed = path.trim();
    trimmed.len() >= 4 && trimmed[trimmed.len() - 4..].eq_ignore_ascii_case(".lnk")
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn normalize_path_key(path: &str) -> String {
    let trimmed = path.trim();
    let mut normalized = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch == '/' {
            normalized.push('\\');
        } else if ch.is_ascii_uppercase() {
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push(ch);
        }
    }
    normalized
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn track_uninstall_title_suppression(
    suppressed_uninstall_titles: &mut Vec<String>,
    action_title: &str,
) {
    let Some(target_title) = uninstall_target_title_from_action_title(action_title) else {
        return;
    };
    if suppressed_uninstall_titles
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(target_title.as_str()))
    {
        return;
    }
    suppressed_uninstall_titles.push(target_title);
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn reconcile_suppressed_uninstall_titles(suppressed_uninstall_titles: &mut Vec<String>) {
    if suppressed_uninstall_titles.is_empty() {
        return;
    }

    suppressed_uninstall_titles.retain(|title| {
        match crate::uninstall_registry::is_display_name_registered(title.as_str()) {
            // Keep suppression while the uninstall entry still exists in the registry.
            // Drop suppression only after the entry disappears.
            Ok(still_registered) => still_registered,
            Err(error) => {
                log_warn(&format!(
                    "[nex] uninstall suppression registry check failed for '{}': {}",
                    title, error
                ));
                true
            }
        }
    });
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn filter_suppressed_uninstall_results(
    results: &mut Vec<crate::model::SearchItem>,
    suppressed_uninstall_titles: &[String],
) {
    if results.is_empty() || suppressed_uninstall_titles.is_empty() {
        return;
    }

    let suppressed_keys: Vec<String> = suppressed_uninstall_titles
        .iter()
        .map(|title| crate::model::normalize_for_search(title.as_str()))
        .filter(|key| !key.is_empty())
        .collect();
    if suppressed_keys.is_empty() {
        return;
    }

    results.retain(|item| {
        let title_key = if item.kind.eq_ignore_ascii_case("app") {
            item.normalized_title().to_string()
        } else if item.kind.eq_ignore_ascii_case("action")
            && item
                .id
                .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX)
        {
            uninstall_target_title_from_action_title(item.title.as_str())
                .map(|title| crate::model::normalize_for_search(title.as_str()))
                .unwrap_or_default()
        } else {
            return true;
        };
        if title_key.is_empty() {
            return true;
        }

        !suppressed_keys
            .iter()
            .any(|suppressed| uninstall_title_matches(title_key.as_str(), suppressed.as_str()))
    });
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn uninstall_target_title_from_action_title(action_title: &str) -> Option<String> {
    let trimmed = action_title.trim();
    if trimmed.len() <= "Uninstall ".len() {
        return None;
    }
    if !trimmed
        .get(.."Uninstall ".len())
        .map(|prefix| prefix.eq_ignore_ascii_case("Uninstall "))
        .unwrap_or(false)
    {
        return None;
    }

    let target = trimmed["Uninstall ".len()..].trim();
    if target.is_empty() {
        None
    } else {
        Some(target.to_string())
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn uninstall_title_matches(app_title_key: &str, suppressed_key: &str) -> bool {
    if app_title_key.is_empty() || suppressed_key.is_empty() {
        return false;
    }
    if app_title_key == suppressed_key {
        return true;
    }

    if suppressed_key.len() >= 6
        && (app_title_key.starts_with(suppressed_key) || suppressed_key.starts_with(app_title_key))
    {
        return true;
    }

    suppressed_key.len() >= 10 && app_title_key.contains(suppressed_key)
}

#[cfg(target_os = "windows")]
fn overlay_subtitle(item: &crate::model::SearchItem, command_mode: bool) -> String {
    if command_mode
        && item.kind.eq_ignore_ascii_case("action")
        && !item
            .id
            .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX)
    {
        return String::new();
    }
    if item.kind.eq_ignore_ascii_case("app") {
        return item.subtitle.trim().to_string();
    }
    if item.kind.eq_ignore_ascii_case("action") {
        if item.path.trim().is_empty() {
            return "Nex action".to_string();
        }
        return item.path.trim().to_string();
    }
    abbreviate_path(&item.path)
}

#[cfg(target_os = "windows")]
fn abbreviate_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains("://") {
        return trimmed.to_string();
    }

    let normalized = trimmed.replace('/', "\\");
    let mut parts: Vec<&str> = normalized.split('\\').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return normalized;
    }

    // Strip filesystem roots (e.g. "C:") so the subtitle remains relative-looking.
    if parts.first().is_some_and(|part| part.ends_with(':')) {
        parts.remove(0);
    }

    if parts.is_empty() {
        return String::new();
    }

    let tail_count = parts.len().min(3);
    let joined_tail = parts[parts.len() - tail_count..].join("\\");
    if parts.len() > 3 {
        format!("...\\{joined_tail}")
    } else {
        joined_tail
    }
}

#[cfg(target_os = "windows")]
fn set_idle_overlay_state(overlay: &NativeOverlayShell) {
    overlay.clear_placeholder_hint();
    overlay.set_results(&[], 0);
    overlay.set_status_text("");
}

#[cfg(target_os = "windows")]
fn set_status_row_overlay_state(overlay: &NativeOverlayShell, message: &str) {
    overlay.clear_placeholder_hint();
    let rows = [OverlayRow {
        role: OverlayRowRole::Status,
        result_index: -1,
        kind: "status".to_string(),
        title: message.to_string(),
        path: String::new(),
        icon_path: String::new(),
    }];
    overlay.set_results(&rows, 0);
    overlay.set_status_text("");
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

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn next_selection_index(current: usize, len: usize, direction: i32) -> usize {
    if len == 0 {
        return 0;
    }

    let max = len - 1;
    if direction < 0 {
        current.saturating_sub(1)
    } else if direction > 0 {
        (current + 1).min(max)
    } else {
        current.min(max)
    }
}

#[cfg(target_os = "windows")]
fn log_registration(registration: &HotkeyRegistration) {
    match registration {
        HotkeyRegistration::Native(id) => {
            log_info(&format!(
                "[nex] hotkey registered native_id={id}"
            ));
        }
        HotkeyRegistration::Noop(label) => {
            log_info(&format!("[nex] hotkey registered noop={label}"));
        }
    }
}

#[cfg(target_os = "windows")]
struct SingleInstanceGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(target_os = "windows")]
fn acquire_single_instance_guard() -> Result<Option<SingleInstanceGuard>, String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mutex_name = to_wide("Local\\NexRuntimeSingleton");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        return Err(format!("CreateMutexW failed with error {error}"));
    }

    // ERROR_ALREADY_EXISTS
    let error = unsafe { GetLastError() };
    if error == 183 {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Ok(None);
    }

    Ok(Some(SingleInstanceGuard { handle }))
}

#[cfg(target_os = "windows")]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ForegroundWindowSnapshot {
    class_name: String,
    process_name: String,
    process_path: String,
    covers_monitor: bool,
    has_standard_frame: bool,
    maximized: bool,
}

#[cfg(target_os = "windows")]
fn toggle_game_mode_from_tray(overlay: &NativeOverlayShell, runtime_config: &mut Config) {
    let previous = runtime_config.game_mode_enabled;
    let next = !previous;
    runtime_config.game_mode_enabled = next;
    overlay.set_game_mode_enabled(next);

    if let Err(error) = config::write_user_template(runtime_config, &runtime_config.config_path) {
        runtime_config.game_mode_enabled = previous;
        overlay.set_game_mode_enabled(previous);
        log_warn(&format!(
            "[nex] failed to persist game mode toggle: {error}"
        ));
        overlay.set_status_text("Could not update Game Mode setting");
        return;
    }

    log_info(&format!(
        "[nex] game mode updated from tray: enabled={next}"
    ));
    overlay.set_status_text(if next {
        "Game Mode enabled"
    } else {
        "Game Mode disabled"
    });
}

#[cfg(target_os = "windows")]
fn should_suppress_hotkey_for_game_mode(cfg: &Config) -> bool {
    if !cfg.game_mode_enabled {
        return false;
    }

    collect_foreground_window_snapshot()
        .is_some_and(|snapshot| should_block_hotkey_for_foreground_window(&snapshot))
}

#[cfg(target_os = "windows")]
fn collect_foreground_window_snapshot() -> Option<ForegroundWindowSnapshot> {
    use windows_sys::Win32::Foundation::{CloseHandle, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_STYLE, SW_SHOWMAXIMIZED,
        WINDOWPLACEMENT, WS_CAPTION, WS_MAXIMIZE, WS_SYSMENU, WS_THICKFRAME,
    };

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }
    if unsafe { IsWindowVisible(foreground) } == 0 || unsafe { IsIconic(foreground) } != 0 {
        return None;
    }

    let mut class_buf = [0u16; 128];
    let class_len =
        unsafe { GetClassNameW(foreground, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    let class_name = if class_len > 0 {
        String::from_utf16_lossy(&class_buf[..class_len as usize])
    } else {
        String::new()
    };
    if is_shell_surface_class_name(&class_name) {
        return None;
    }

    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }

    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rcWork: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        dwFlags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut monitor_info as *mut MONITORINFO) } == 0 {
        return None;
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(foreground, &mut rect as *mut RECT) } == 0 {
        return None;
    }

    let fuzz = 2;
    let covers_monitor = rect.left <= monitor_info.rcMonitor.left + fuzz
        && rect.top <= monitor_info.rcMonitor.top + fuzz
        && rect.right >= monitor_info.rcMonitor.right - fuzz
        && rect.bottom >= monitor_info.rcMonitor.bottom - fuzz;

    let style = unsafe { GetWindowLongPtrW(foreground, GWL_STYLE) as u32 };
    let has_standard_frame =
        style & ((WS_CAPTION | WS_THICKFRAME | WS_SYSMENU) as u32) != 0;

    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        flags: 0,
        showCmd: 0,
        ptMinPosition: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        ptMaxPosition: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        rcNormalPosition: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    };
    let placement_reports_maximized =
        unsafe { GetWindowPlacement(foreground, &mut placement as *mut WINDOWPLACEMENT) } != 0
            && placement.showCmd == SW_SHOWMAXIMIZED as u32;
    let maximized = placement_reports_maximized || (style & (WS_MAXIMIZE as u32) != 0);

    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(foreground, &mut pid as *mut u32);
    }

    let mut process_path = String::new();
    let mut process_name = String::new();
    if pid != 0 {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if !process.is_null() {
            let mut buffer = vec![0u16; 1024];
            let mut length = buffer.len() as u32;
            let ok = unsafe {
                QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length as *mut u32)
            };
            if ok != 0 && length > 0 {
                process_path = String::from_utf16_lossy(&buffer[..length as usize]);
                process_name = std::path::Path::new(&process_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
            }
            unsafe {
                CloseHandle(process);
            }
        }
    }

    Some(ForegroundWindowSnapshot {
        class_name,
        process_name,
        process_path,
        covers_monitor,
        has_standard_frame,
        maximized,
    })
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn should_block_hotkey_for_foreground_window(snapshot: &ForegroundWindowSnapshot) -> bool {
    if !snapshot.covers_monitor {
        return false;
    }
    if snapshot.maximized && snapshot.has_standard_frame {
        return false;
    }

    let process_name = snapshot.process_name.trim().to_ascii_lowercase();
    let process_path = snapshot.process_path.trim().to_ascii_lowercase();
    if is_known_non_game_process(&process_name) || is_known_non_game_path(&process_path) {
        return false;
    }
    if is_known_game_process(&process_name) || is_likely_game_path(&process_path) {
        return true;
    }

    !snapshot.has_standard_frame
}

#[allow(dead_code)]
fn is_shell_surface_class_name(class_name: &str) -> bool {
    matches!(
        class_name.trim().to_ascii_lowercase().as_str(),
        "progman" | "workerw" | "shell_traywnd"
    )
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn is_known_non_game_process(process_name: &str) -> bool {
    matches!(
        process_name,
        ""
            | "explorer.exe"
            | "taskmgr.exe"
            | "chrome.exe"
            | "msedge.exe"
            | "firefox.exe"
            | "waterfox.exe"
            | "code.exe"
            | "devenv.exe"
            | "wezterm-gui.exe"
            | "windowsterminal.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "cmd.exe"
            | "notepad.exe"
            | "notepad++.exe"
            | "vlc.exe"
            | "mpv.exe"
            | "obs64.exe"
            | "applicationframehost.exe"
            | "searchhost.exe"
            | "startmenuexperiencehost.exe"
            | "lockapp.exe"
    )
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn is_known_non_game_path(process_path: &str) -> bool {
    process_path.contains("\\microsoft\\edge\\application\\")
        || process_path.contains("\\google\\chrome\\application\\")
        || process_path.contains("\\mozilla firefox\\")
        || process_path.contains("\\microsoft\\windowsapps\\")
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn is_known_game_process(process_name: &str) -> bool {
    process_name.contains("valorant")
        || process_name == "cs2.exe"
        || process_name == "csgo.exe"
        || process_name == "cod.exe"
        || process_name == "modernwarfare.exe"
        || process_name.ends_with("-shipping.exe")
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn is_likely_game_path(process_path: &str) -> bool {
    process_path.contains("\\steamapps\\common\\")
        || process_path.contains("\\riot games\\")
        || process_path.contains("\\epic games\\")
        || process_path.contains("\\battle.net\\")
        || process_path.contains("\\blizzard entertainment\\")
        || process_path.contains("\\ubisoft\\")
        || process_path.contains("\\rockstar games\\")
        || process_path.contains("\\gog galaxy\\games\\")
        || process_path.contains("\\ea games\\")
        || process_path.contains("\\electronic arts\\")
}

#[cfg(target_os = "windows")]
fn start_background_index_refresh(
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
        let outcome = CoreService::new(worker_config)
            .map(|service| service.with_runtime_providers())
            .and_then(|service| service.rebuild_index_incremental_with_report())
            .map_err(|error| format!("background indexing failed: {error}"));
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
        initial_cache_empty,
        pending_discovery_reindex: false,
        ready_notice_pending: false,
        started_at: Instant::now(),
        startup_started_at,
    }
}

#[cfg(target_os = "windows")]
fn maybe_apply_background_index_refresh(
    service: &CoreService,
    state: &mut BackgroundIndexRefresh,
    runtime_config: &Config,
) {
    if state.cache_applied {
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
                        "[nex] startup indexed_items={} discovered={} upserted={} removed={} elapsed_ms={} cached_items={}",
                        report.indexed_total,
                        report.discovered_total,
                        report.upserted_total,
                        report.removed_total,
                        elapsed_ms,
                        cached_items
                    ));
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
                }
                Err(error) => {
                    log_warn(&format!(
                        "[nex] background indexing cache refresh failed: {error}"
                    ));
                }
            }
            if state.initial_cache_empty {
                state.ready_notice_pending = true;
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
            "[nex] discovery settings queued during indexing; starting pending reindex",
        );
        log_info(&format!(
            "[nex] startup_phase phase=indexing_started elapsed_ms={} initial_cache_empty=false cached_items={}",
            state.startup_started_at.elapsed().as_millis(),
            service.cached_items_len()
        ));
        *state = start_background_index_refresh(runtime_config, false, state.startup_started_at);
    }
}

#[cfg(target_os = "windows")]
fn should_show_indexing_status(state: &BackgroundIndexRefresh) -> bool {
    state.initial_cache_empty && !state.cache_applied
}

#[cfg(target_os = "windows")]
fn maybe_show_background_index_ready_notice(
    overlay: &NativeOverlayShell,
    state: &mut BackgroundIndexRefresh,
) {
    if !state.ready_notice_pending || !overlay.is_visible() {
        return;
    }
    if !overlay.query_text().trim().is_empty() {
        return;
    }

    set_idle_overlay_state(overlay);
    overlay.show_placeholder_hint(STATUS_ROW_TYPE_TO_SEARCH);
    overlay.set_status_text(STATUS_TEXT_INDEX_READY);
    state.ready_notice_pending = false;
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[cfg_attr(not(test), allow(dead_code))]
fn search_overlay_results(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    parsed_query: &ParsedQuery,
    result_limit: usize,
) -> Result<Vec<crate::model::SearchItem>, String> {
    let mut session = OverlaySearchSession::default();
    search_overlay_results_with_session(
        service,
        cfg,
        plugins,
        parsed_query,
        result_limit,
        &mut session,
    )
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn search_overlay_results_with_session(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    parsed_query: &ParsedQuery,
    result_limit: usize,
    session: &mut OverlaySearchSession,
) -> Result<Vec<crate::model::SearchItem>, String> {
    if result_limit == 0 {
        return Ok(Vec::new());
    }

    let filter = build_search_filter(cfg, parsed_query);
    let text_query = parsed_query.free_text.trim();
    let normalized_query = crate::model::normalize_for_search(text_query);
    if should_skip_non_searchable_query(parsed_query, &normalized_query) {
        log_info(&format!(
            "[nex] query_guard skip=non_searchable_symbol_only q=\"{}\"",
            sanitize_query_for_profile_log(parsed_query.raw.as_str())
        ));
        session.clear();
        return Ok(Vec::new());
    }
    let cache_key = final_query_cache_key(parsed_query, &filter, &normalized_query, result_limit);
    if let Some(cached) = cached_final_query_results(session, &cache_key) {
        return Ok(cached);
    }
    let candidate_limit = candidate_limit_for_query(
        result_limit,
        &filter,
        &normalized_query,
        parsed_query.command_mode,
    );
    let base_indexed_seed_limit = indexed_seed_limit(candidate_limit, normalized_query.len());
    let seed_cap = (cfg.index_max_items_per_query_seed as usize).max(candidate_limit);
    let indexed_seed_limit = adaptive_indexed_seed_limit(
        session,
        candidate_limit,
        normalized_query.len(),
        base_indexed_seed_limit,
    )
    .min(seed_cap);
    let short_query_app_bias =
        should_use_short_query_app_mode(parsed_query, &filter, &normalized_query);
    let mut indexed_filter = filter.clone();
    if short_query_app_bias {
        indexed_filter.mode = crate::config::SearchMode::Apps;
    }

    let search_started = Instant::now();
    let mut merged = Vec::new();
    let indexed_started = Instant::now();
    let mut indexed_cache_hit = false;
    let prefix_cache_eligible = is_prefix_cache_eligible_query(parsed_query, short_query_app_bias);
    let indexed_seed_items = if let Some(cache) =
        session.indexed_prefix_cache.as_ref().filter(|cache| {
            can_use_indexed_prefix_cache(
                cache,
                prefix_cache_eligible,
                &normalized_query,
                &indexed_filter,
            )
        }) {
        indexed_cache_hit = true;
        crate::search::search_with_filter(
            &cache.seed_items,
            text_query,
            indexed_seed_limit,
            &indexed_filter,
        )
    } else {
        service
            .search_with_filter_uncapped(text_query, indexed_seed_limit, &indexed_filter)
            .map_err(|error| format!("indexed search failed: {error}"))?
    };
    let indexed_ms = indexed_started.elapsed().as_millis();
    if !indexed_cache_hit {
        record_indexed_latency_sample(session, indexed_ms);
    }
    let indexed_count = indexed_seed_items.len();
    merged.extend(indexed_seed_items.iter().take(candidate_limit).cloned());
    if prefix_cache_eligible && normalized_query.len() >= INDEXED_PREFIX_CACHE_MIN_QUERY_LEN {
        session.indexed_prefix_cache = Some(IndexedPrefixCache {
            normalized_query: normalized_query.clone(),
            indexed_filter: indexed_filter.clone(),
            seed_items: indexed_seed_items,
        });
    } else {
        session.clear();
    }

    let mut provider_ms = 0_u128;
    let mut provider_count = 0_usize;
    if !short_query_app_bias {
        let provider_started = Instant::now();
        let provider_results = crate::search::search_with_filter(
            &plugins.provider_items,
            text_query,
            candidate_limit,
            &filter,
        );
        provider_ms = provider_started.elapsed().as_millis();
        provider_count = provider_results.len();
        merged.extend(provider_results);
    }

    let actions_started = Instant::now();
    let mut action_items =
        search_actions_with_mode(text_query, candidate_limit, parsed_query.command_mode, cfg);
    let built_in_actions_count = action_items.len();
    let mut plugin_action_count = 0_usize;
    if !plugins.action_items.is_empty() {
        let plugin_actions = crate::search::search_with_filter(
            &plugins.action_items,
            text_query,
            candidate_limit,
            &SearchFilter {
                mode: crate::config::SearchMode::Actions,
                ..SearchFilter::default()
            },
        );
        plugin_action_count = plugin_actions.len();
        action_items.extend(plugin_actions);
    }
    let action_results =
        crate::search::search_with_filter(&action_items, text_query, candidate_limit, &filter);
    let actions_ms = actions_started.elapsed().as_millis();
    let action_count = action_results.len();
    merged.extend(action_results);

    let mut clipboard_ms = 0_u128;
    let mut clipboard_count = 0_usize;
    if !short_query_app_bias {
        let clipboard_started = Instant::now();
        let clipboard_results =
            clipboard_history::search_history(cfg, text_query, &filter, candidate_limit.min(120));
        clipboard_ms = clipboard_started.elapsed().as_millis();
        clipboard_count = clipboard_results.len();
        merged.extend(clipboard_results);
    }

    let rank_started = Instant::now();
    let ranked = crate::search::search_with_filter(&merged, text_query, result_limit, &filter);
    let rank_ms = rank_started.elapsed().as_millis();
    let total_ms = search_started.elapsed().as_millis();
    if total_ms >= QUERY_PROFILE_LOG_THRESHOLD_MS {
        log_info(&format!(
            "[nex] query_profile q=\"{}\" mode={} candidate_limit={} indexed_seed_limit={} short_app_bias={} indexed_cache_hit={} indexed_count={} indexed_ms={} provider_count={} provider_ms={} action_count={} action_ms={} built_in_actions={} plugin_actions={} clipboard_count={} clipboard_ms={} rank_ms={} total_ms={}",
            sanitize_query_for_profile_log(text_query),
            format!("{:?}", filter.mode).to_ascii_lowercase(),
            candidate_limit,
            indexed_seed_limit,
            short_query_app_bias,
            indexed_cache_hit,
            indexed_count,
            indexed_ms,
            provider_count,
            provider_ms,
            action_count,
            actions_ms,
            built_in_actions_count,
            plugin_action_count,
            clipboard_count,
            clipboard_ms,
            rank_ms,
            total_ms
        ));
    }
    store_final_query_results(session, cache_key, ranked.as_slice());
    Ok(ranked)
}

fn build_search_filter(cfg: &Config, parsed_query: &ParsedQuery) -> SearchFilter {
    let mode = resolved_mode_for_query(cfg, parsed_query);
    SearchFilter {
        mode,
        kind_filter: parsed_query.kind_filter.clone(),
        extension_filter: parsed_query.extension_filter.clone(),
        include_files: cfg.show_files,
        include_folders: cfg.show_folders,
        include_groups: parsed_query.include_groups.clone(),
        exclude_terms: parsed_query.exclude_terms.clone(),
        modified_within: parsed_query.modified_within,
        created_within: parsed_query.created_within,
    }
}

fn resolved_mode_for_query(cfg: &Config, parsed_query: &ParsedQuery) -> crate::config::SearchMode {
    let mut mode = parsed_query
        .mode_override
        .unwrap_or(cfg.search_mode_default);
    if parsed_query.command_mode {
        mode = crate::config::SearchMode::Actions;
    }
    mode
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn should_use_short_query_app_mode(
    parsed_query: &ParsedQuery,
    filter: &SearchFilter,
    normalized_query: &str,
) -> bool {
    if normalized_query.is_empty() || normalized_query.len() > SHORT_QUERY_APP_BIAS_MAX_LEN {
        return false;
    }
    if parsed_query.command_mode {
        return false;
    }
    if filter.mode != crate::config::SearchMode::All {
        return false;
    }
    parsed_query.kind_filter.is_none()
        && parsed_query.extension_filter.is_none()
        && parsed_query.exclude_terms.is_empty()
        && parsed_query.modified_within.is_none()
        && parsed_query.created_within.is_none()
}

fn should_skip_non_searchable_query(parsed_query: &ParsedQuery, normalized_query: &str) -> bool {
    if !normalized_query.is_empty() {
        return false;
    }
    if parsed_query.command_mode {
        return false;
    }
    if parsed_query.mode_override.is_some() {
        return false;
    }
    parsed_query.kind_filter.is_none()
        && parsed_query.extension_filter.is_none()
        && parsed_query.include_groups.is_empty()
        && parsed_query.exclude_terms.is_empty()
        && parsed_query.modified_within.is_none()
        && parsed_query.created_within.is_none()
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn result_limit_for_query(base_limit: usize, parsed_query: &ParsedQuery) -> usize {
    if base_limit == 0 {
        return 0;
    }
    if parsed_query.command_mode
        && crate::uninstall_registry::has_uninstall_intent(parsed_query.free_text.as_str())
    {
        return base_limit.max(UNINSTALL_QUERY_RESULT_LIMIT);
    }
    base_limit
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn maybe_expand_uninstall_quick_shortcut(query: &str, last_query: &str) -> Option<String> {
    let raw = query.trim_start();
    let remainder = raw.strip_prefix('>')?;
    if remainder.eq_ignore_ascii_case("u") {
        let last_trimmed = last_query.trim();
        if last_trimmed.is_empty() || last_trimmed == ">" {
            return Some(">u ".to_string());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn apply_query_change(
    mut query: String,
    overlay: &NativeOverlayShell,
    service: &CoreService,
    runtime_config: &Config,
    plugin_registry: &PluginRegistry,
    max_results: usize,
    background_index_refresh: &BackgroundIndexRefresh,
    search_session: &mut OverlaySearchSession,
    pending_uninstall_confirmation: &mut Option<PendingUninstallConfirmation>,
    suppressed_uninstall_titles: &[String],
    current_results: &mut Vec<crate::model::SearchItem>,
    selected_index: &mut usize,
    last_query: &mut String,
) {
    *pending_uninstall_confirmation = None;
    if let Some(expanded) = maybe_expand_uninstall_quick_shortcut(&query, last_query.as_str()) {
        overlay.set_query_text(&expanded);
        query = expanded;
    }

    let trimmed = query.trim();
    if trimmed.is_empty() {
        current_results.clear();
        *selected_index = 0;
        last_query.clear();
        search_session.clear();
        *pending_uninstall_confirmation = None;
        set_idle_overlay_state(overlay);
        return;
    }
    if trimmed == last_query {
        return;
    }
    *last_query = trimmed.to_string();
    let parsed_query = ParsedQuery::parse(trimmed, runtime_config.search_dsl_enabled);
    let query_result_limit = result_limit_for_query(max_results, &parsed_query);

    match search_overlay_results_with_session(
        service,
        runtime_config,
        plugin_registry,
        &parsed_query,
        query_result_limit,
        search_session,
    ) {
        Ok(mut results) => {
            dedupe_overlay_results(&mut results);
            if !suppressed_uninstall_titles.is_empty() {
                filter_suppressed_uninstall_results(&mut results, suppressed_uninstall_titles);
            }
            *current_results = results;
            *selected_index = 0;
            if current_results.is_empty() {
                if should_show_indexing_status(background_index_refresh) {
                    set_status_row_overlay_state(overlay, STATUS_ROW_INDEXING);
                } else {
                    set_status_row_overlay_state(
                        overlay,
                        if parsed_query.command_mode {
                            STATUS_ROW_NO_COMMAND_RESULTS
                        } else {
                            STATUS_ROW_NO_RESULTS
                        },
                    );
                }
            } else {
                let rows = overlay_rows(current_results, parsed_query.command_mode);
                overlay.set_results(&rows, *selected_index);
            }
        }
        Err(error) => {
            current_results.clear();
            *selected_index = 0;
            search_session.clear();
            overlay.set_results(&[], 0);
            overlay.set_status_text(&format!("Search error: {error}"));
        }
    }
}

#[cfg(target_os = "windows")]
fn config_file_modified_time(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(target_os = "windows")]
fn maybe_apply_runtime_config_reload(
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
                || next_config.windows_search_enabled != previous.windows_search_enabled
                || next_config.windows_search_fallback_filesystem
                    != previous.windows_search_fallback_filesystem
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
                log_warn(
                    "[nex] config index_db_path changed; restart required to apply",
                );
            }
            if discovery_config_changed {
                if let Err(error) = service.reconfigure_runtime_providers(runtime_config) {
                    log_warn(&format!(
                        "[nex] provider reconfigure failed after config reload: {error}"
                    ));
                } else {
                    if background_index_refresh.cache_applied {
                        *background_index_refresh =
                            start_background_index_refresh(
                                runtime_config,
                                false,
                                background_index_refresh.startup_started_at,
                            );
                        log_info("[nex] discovery settings changed; background reindex started");
                    } else {
                        background_index_refresh.pending_discovery_reindex = true;
                        discovery_reindex_queued = true;
                        log_info("[nex] discovery settings changed while indexing is active; reindex queued");
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
                    overlay.set_status_text("Discovery settings queued; reindex starts next");
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

#[cfg(target_os = "windows")]
fn should_suppress_failed_uninstall(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("shell_code=2")
        || lower.contains(" code 2")
        || lower.contains("no longer available")
        || lower.contains("file not found")
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn uninstall_confirmation_results(
    uninstall_action: &crate::model::SearchItem,
) -> Vec<crate::model::SearchItem> {
    let target = uninstall_target_title_from_action_title(uninstall_action.title.as_str())
        .unwrap_or_else(|| uninstall_action.title.trim().to_string());
    let confirm_title = if target.is_empty() {
        "Confirm uninstall".to_string()
    } else {
        format!("Confirm uninstall {}", target.trim())
    };

    vec![
        crate::model::SearchItem::new(
            ACTION_UNINSTALL_CONFIRM_ID,
            "action",
            confirm_title.as_str(),
            "Open app uninstaller",
        ),
        crate::model::SearchItem::new(
            ACTION_UNINSTALL_CANCEL_ID,
            "action",
            "Cancel",
            "Return to previous results",
        ),
    ]
}

fn candidate_limit_for_query(
    result_limit: usize,
    filter: &SearchFilter,
    normalized_query: &str,
    command_mode: bool,
) -> usize {
    if result_limit == 0 {
        return 0;
    }

    let base = result_limit.saturating_mul(6).max(60);
    if command_mode || filter.mode == crate::config::SearchMode::Actions {
        return result_limit
            .saturating_mul(4)
            .max(48)
            .min(160)
            .max(result_limit);
    }

    match normalized_query.len() {
        0 => result_limit
            .saturating_mul(2)
            .max(24)
            .min(64)
            .max(result_limit),
        1 => match filter.mode {
            crate::config::SearchMode::All => result_limit
                .saturating_mul(3)
                .max(45)
                .min(96)
                .max(result_limit),
            crate::config::SearchMode::Files => result_limit
                .saturating_mul(5)
                .max(70)
                .min(200)
                .max(result_limit),
            _ => result_limit
                .saturating_mul(4)
                .max(56)
                .min(180)
                .max(result_limit),
        },
        2 => match filter.mode {
            crate::config::SearchMode::All => result_limit
                .saturating_mul(4)
                .max(56)
                .min(140)
                .max(result_limit),
            crate::config::SearchMode::Files => result_limit
                .saturating_mul(5)
                .max(70)
                .min(200)
                .max(result_limit),
            _ => result_limit
                .saturating_mul(4)
                .max(56)
                .min(180)
                .max(result_limit),
        },
        _ => base.min(280).max(result_limit),
    }
}

fn indexed_seed_limit(candidate_limit: usize, normalized_query_len: usize) -> usize {
    let multiplier = match normalized_query_len {
        0 | 1 => 4,
        2 => 2,
        _ => 2,
    };
    candidate_limit.saturating_mul(multiplier).clamp(
        INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT,
        INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT,
    )
}

fn adaptive_indexed_seed_limit(
    session: &OverlaySearchSession,
    candidate_limit: usize,
    normalized_query_len: usize,
    base_seed_limit: usize,
) -> usize {
    let mut samples: Vec<u128> = session.indexed_latency_ms.iter().copied().collect();
    if samples.len() < 6 {
        return base_seed_limit;
    }

    let p95 = percentile_u128(&mut samples, 0.95);
    let scaled = if p95 >= 160 {
        (base_seed_limit.saturating_mul(60)) / 100
    } else if p95 >= 120 {
        (base_seed_limit.saturating_mul(72)) / 100
    } else if p95 >= 95 {
        (base_seed_limit.saturating_mul(84)) / 100
    } else if p95 <= 50 && normalized_query_len >= 3 {
        (base_seed_limit.saturating_mul(108)) / 100
    } else {
        base_seed_limit
    };

    let minimum = candidate_limit.max(INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT / 2);
    scaled.clamp(minimum, INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT)
}

fn record_indexed_latency_sample(session: &mut OverlaySearchSession, indexed_ms: u128) {
    session.indexed_latency_ms.push_back(indexed_ms);
    while session.indexed_latency_ms.len() > ADAPTIVE_INDEXED_LATENCY_WINDOW {
        session.indexed_latency_ms.pop_front();
    }
}

fn final_query_cache_key(
    parsed_query: &ParsedQuery,
    filter: &SearchFilter,
    normalized_query: &str,
    result_limit: usize,
) -> String {
    format!(
        "q={};mode={:?};kind={};ext={};include={};exclude={};modified={:?};created={:?};cmd={};limit={}",
        normalized_query,
        filter.mode,
        filter.kind_filter.as_deref().unwrap_or("-"),
        filter.extension_filter.as_deref().unwrap_or("-"),
        encode_term_groups(&filter.include_groups),
        filter.exclude_terms.join(","),
        filter.modified_within,
        filter.created_within,
        parsed_query.command_mode,
        result_limit
    )
}

fn encode_term_groups(groups: &[Vec<String>]) -> String {
    if groups.is_empty() {
        return "-".to_string();
    }

    groups
        .iter()
        .map(|group| group.join("+"))
        .collect::<Vec<String>>()
        .join("|")
}

fn cached_final_query_results(
    session: &mut OverlaySearchSession,
    key: &str,
) -> Option<Vec<crate::model::SearchItem>> {
    let cached = session.final_query_cache.get(key).cloned()?;
    if let Some(position) = session
        .final_query_cache_lru
        .iter()
        .position(|entry| entry == key)
    {
        session.final_query_cache_lru.remove(position);
    }
    session.final_query_cache_lru.push_back(key.to_string());
    Some(cached)
}

fn store_final_query_results(
    session: &mut OverlaySearchSession,
    key: String,
    results: &[crate::model::SearchItem],
) {
    if results.is_empty() {
        return;
    }

    session
        .final_query_cache
        .insert(key.clone(), results.to_vec());
    if let Some(position) = session
        .final_query_cache_lru
        .iter()
        .position(|entry| entry == &key)
    {
        session.final_query_cache_lru.remove(position);
    }
    session.final_query_cache_lru.push_back(key);

    while session.final_query_cache.len() > FINAL_QUERY_CACHE_MAX_ENTRIES {
        let Some(oldest) = session.final_query_cache_lru.pop_front() else {
            break;
        };
        session.final_query_cache.remove(&oldest);
    }
}

fn can_use_indexed_prefix_cache(
    cache: &IndexedPrefixCache,
    prefix_cache_eligible: bool,
    normalized_query: &str,
    indexed_filter: &SearchFilter,
) -> bool {
    if !prefix_cache_eligible {
        return false;
    }
    if cache.seed_items.is_empty() || cache.normalized_query.is_empty() {
        return false;
    }
    if !indexed_filter_matches_for_prefix_cache(&cache.indexed_filter, indexed_filter) {
        return false;
    }
    normalized_query.len() > cache.normalized_query.len()
        && normalized_query.starts_with(&cache.normalized_query)
}

fn indexed_filter_matches_for_prefix_cache(a: &SearchFilter, b: &SearchFilter) -> bool {
    a.mode == b.mode
        && a.kind_filter == b.kind_filter
        && a.extension_filter == b.extension_filter
        && a.modified_within == b.modified_within
        && a.created_within == b.created_within
}

fn is_prefix_cache_eligible_query(parsed_query: &ParsedQuery, short_query_app_bias: bool) -> bool {
    if short_query_app_bias || parsed_query.command_mode {
        return false;
    }
    if parsed_query.mode_override.is_some()
        || parsed_query.kind_filter.is_some()
        || parsed_query.extension_filter.is_some()
        || !parsed_query.exclude_terms.is_empty()
        || parsed_query.modified_within.is_some()
        || parsed_query.created_within.is_some()
    {
        return false;
    }
    if parsed_query.free_text.trim().is_empty() {
        return false;
    }
    parsed_query.raw.trim() == parsed_query.free_text.trim()
}

fn sanitize_query_for_profile_log(query: &str) -> String {
    const MAX_QUERY_LOG_CHARS: usize = 48;
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return "-".to_string();
    }
    let mut cleaned = String::new();
    for ch in trimmed.chars().take(MAX_QUERY_LOG_CHARS) {
        if ch.is_control() {
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }
    cleaned.trim().to_string()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn launch_overlay_selection(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    results: &[crate::model::SearchItem],
    selected_index: usize,
    query_text: &str,
) -> Result<(), String> {
    if results.is_empty() {
        return Err("no result selected".to_string());
    }

    if selected_index >= results.len() {
        return Err(format!(
            "selected index out of range: {selected_index} (len={})",
            results.len()
        ));
    }

    let selected = &results[selected_index];
    if selected.kind.eq_ignore_ascii_case("action") {
        return execute_action_selection(service, cfg, plugins, selected);
    }
    if selected.kind.eq_ignore_ascii_case("clipboard") {
        return clipboard_history::copy_result_to_clipboard(cfg, &selected.id);
    }

    let parsed_query = ParsedQuery::parse(query_text.trim(), cfg.search_dsl_enabled);
    let mode = resolved_mode_for_query(cfg, &parsed_query);
    service
        .launch_with_query_context(LaunchTarget::Id(&selected.id), Some(query_text), Some(mode))
        .map_err(|error| format!("launch failed: {error}"))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn execute_action_selection(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    selected: &crate::model::SearchItem,
) -> Result<(), String> {
    if selected
        .id
        .starts_with(crate::uninstall_registry::ACTION_UNINSTALL_PREFIX)
    {
        return crate::uninstall_registry::execute_uninstall_action(&selected.id)
            .map_err(|error| format!("uninstall launch failed: {error}"));
    }

    if selected.id.starts_with(ACTION_WEB_SEARCH_PREFIX) {
        return crate::action_executor::launch_open_target(selected.path.trim())
            .map_err(|error| format!("web search launch failed: {error}"));
    }

    match selected.id.as_str() {
        ACTION_OPEN_LOGS_ID => crate::logging::open_logs_folder()
            .map_err(|error| format!("open logs folder failed: {error}")),
        ACTION_REBUILD_INDEX_ID => {
            let report = service
                .rebuild_index_with_report()
                .map_err(|error| format!("rebuild index failed: {error}"))?;
            log_info(&format!(
                "[nex] action_rebuild_index indexed={} discovered={} upserted={} removed={}",
                report.indexed_total, report.discovered_total, report.upserted_total, report.removed_total
            ));
            Ok(())
        }
        ACTION_CLEAR_CLIPBOARD_ID => clipboard_history::clear_history(cfg),
        ACTION_OPEN_CONFIG_ID => {
            crate::action_executor::launch_path(cfg.config_path.to_string_lossy().as_ref())
                .map_err(|error| format!("open config failed: {error}"))
        }
        ACTION_DIAGNOSTICS_BUNDLE_ID => {
            let output_dir = write_diagnostics_bundle(cfg)
                .map_err(|error| format!("diagnostics bundle failed: {error}"))?;
            log_info(&format!(
                "[nex] diagnostics bundle written to {}",
                output_dir.display()
            ));
            Ok(())
        }
        ACTION_TRIM_MEMORY_ID => {
            log_info("[nex] trim memory action invoked");
            Ok(())
        }
        _ => execute_plugin_action(cfg, plugins, &selected.id),
    }
}

fn execute_plugin_action(
    cfg: &Config,
    plugins: &PluginRegistry,
    result_id: &str,
) -> Result<(), String> {
    let action = plugins
        .actions_by_result_id
        .get(result_id)
        .ok_or_else(|| "unknown action".to_string())?;

    match &action.kind {
        PluginActionKind::OpenPath { path } => crate::action_executor::launch_path(path)
            .map_err(|error| format!("plugin open path failed: {error}")),
        PluginActionKind::Command { command, args } => {
            if cfg.plugins_safe_mode {
                return Err(
                    "plugin command execution blocked: plugins_safe_mode is enabled in config"
                        .to_string(),
                );
            }
            if command.trim().is_empty() {
                return Err("plugin command action missing command".to_string());
            }
            std::process::Command::new(command)
                .args(args)
                .spawn()
                .map_err(|e| format!("plugin command spawn failed: {e}"))?;
            Ok(())
        }
    }
}

fn configure_stdio_logging(options: RuntimeOptions) {
    let suppress_from_env = env_var_with_legacy("NEX_SUPPRESS_STDIO", "SWIFTFIND_SUPPRESS_STDIO")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let suppress_for_background = options.command == RuntimeCommand::Run && options.background;
    STDIO_LOGGING_ENABLED.store(
        !(suppress_from_env || suppress_for_background),
        Ordering::Relaxed,
    );
}

fn should_log_to_stdio() -> bool {
    STDIO_LOGGING_ENABLED.load(Ordering::Relaxed)
}

fn log_info(message: &str) {
    if should_log_to_stdio() {
        println!("{message}");
    }
    crate::logging::info(message);
}

fn log_warn(message: &str) {
    if should_log_to_stdio() {
        eprintln!("{message}");
    }
    crate::logging::warn(message);
}

#[cfg(test)]
mod tests {
    use super::{
        adaptive_indexed_seed_limit, build_status_diagnostics_json,
        can_use_indexed_prefix_cache, candidate_limit_for_query, dedupe_overlay_results,
        filter_suppressed_uninstall_results, launch_overlay_selection,
        maybe_expand_uninstall_quick_shortcut, next_selection_index, parse_cli_args,
        parse_status_diagnostics_snapshot, parse_tasklist_pid_lines, result_limit_for_query,
        search_overlay_results, search_overlay_results_with_session,
        should_block_hotkey_for_foreground_window, should_hide_known_start_menu_doc_sample_entry,
        should_skip_non_searchable_query, summarize_query_profiles,
        track_uninstall_title_suppression,
        uninstall_confirmation_results, uninstall_target_title_from_action_title,
        ForegroundWindowSnapshot, IndexedPrefixCache, OverlaySearchSession, RuntimeCommand,
        RuntimeOptions, ACTION_UNINSTALL_CANCEL_ID, ACTION_UNINSTALL_CONFIRM_ID,
        INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT, INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT,
        UNINSTALL_QUERY_RESULT_LIMIT,
    };
    use crate::action_registry::{ACTION_DIAGNOSTICS_BUNDLE_ID, ACTION_WEB_SEARCH_PREFIX};
    use crate::config::{Config, SearchMode};
    use crate::core_service::CoreService;
    use crate::index_store::open_memory;
    use crate::model::SearchItem;
    use crate::plugin_sdk::PluginRegistry;
    use crate::query_dsl::ParsedQuery;
    use crate::search::SearchFilter;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn overlay_search_returns_ranked_results() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nex-overlay-search-{unique}.tmp"));
        std::fs::write(&path, b"ok").expect("temp file should be created");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "item-1",
                "app",
                "Visual Studio Code",
                path.to_string_lossy().as_ref(),
            ))
            .expect("item should upsert");

        let parsed = ParsedQuery::parse("code", true);
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let results = search_overlay_results(&service, &cfg, &plugins, &parsed, 20)
            .expect("search should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "item-1");

        std::fs::remove_file(path).expect("temp file should be removed");
    }

    #[test]
    fn overlay_launch_selection_launches_selected_item() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let launch_path = std::env::temp_dir().join(format!("nex-launch-flow-{unique}.tmp"));
        std::fs::write(&launch_path, b"ok").expect("temp launch file should be created");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "item-1",
                "file",
                "Code Launcher",
                launch_path.to_string_lossy().as_ref(),
            ))
            .expect("item should upsert");

        let parsed = ParsedQuery::parse("code", true);
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let results = search_overlay_results(&service, &cfg, &plugins, &parsed, 20)
            .expect("search should succeed");
        launch_overlay_selection(&service, &cfg, &plugins, &results, 0, "launch target")
            .expect("launch should succeed");

        std::fs::remove_file(&launch_path).expect("temp launch file should be removed");
    }

    #[test]
    fn overlay_launch_selection_reports_error_for_missing_path() {
        let missing_path = std::env::temp_dir().join("nex-does-not-exist-launch-flow.exe");
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        let item = SearchItem::new(
            "missing",
            "file",
            "Missing Item",
            missing_path.to_string_lossy().as_ref(),
        );
        service
            .upsert_item(&SearchItem::new(
                "missing",
                "file",
                "Missing Item",
                missing_path.to_string_lossy().as_ref(),
            ))
            .expect("item should upsert");

        let results = vec![item];
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let error = launch_overlay_selection(&service, &cfg, &plugins, &results, 0, "missing")
            .expect_err("launch should fail");

        assert!(error.contains("launch failed:"));
    }

    #[test]
    fn overlay_launch_selection_rejects_out_of_range_index() {
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        let results = vec![SearchItem::new("item-1", "app", "One", "C:\\One.exe")];

        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let error = launch_overlay_selection(&service, &cfg, &plugins, &results, 1, "out")
            .expect_err("selection should fail");

        assert!(error.contains("selected index out of range"));
    }

    #[test]
    fn overlay_launch_selection_rejects_empty_results() {
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");

        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let error = launch_overlay_selection(&service, &cfg, &plugins, &[], 0, "")
            .expect_err("empty selection should fail");

        assert_eq!(error, "no result selected");
    }

    #[test]
    fn selection_index_bounds_are_stable() {
        assert_eq!(next_selection_index(0, 0, 1), 0);
        assert_eq!(next_selection_index(0, 3, -1), 0);
        assert_eq!(next_selection_index(1, 3, -1), 0);
        assert_eq!(next_selection_index(1, 3, 1), 2);
        assert_eq!(next_selection_index(2, 3, 1), 2);
        assert_eq!(next_selection_index(1, 3, 0), 1);
        assert_eq!(next_selection_index(5, 3, 0), 2);
    }

    #[test]
    fn candidate_limit_adapts_to_query_shape() {
        let all = SearchFilter::default();
        let empty_all = candidate_limit_for_query(20, &all, "", false);
        let short_all = candidate_limit_for_query(20, &all, "v", false);
        let medium_all = candidate_limit_for_query(20, &all, "vi", false);
        let long_all = candidate_limit_for_query(20, &all, "vivaldi", false);
        assert!(empty_all <= short_all);
        assert!(short_all < medium_all);
        assert!(medium_all <= long_all);

        let actions = SearchFilter {
            mode: SearchMode::Actions,
            ..SearchFilter::default()
        };
        let short_actions = candidate_limit_for_query(20, &actions, "v", true);
        assert!(short_actions < long_all);
    }

    #[test]
    fn uninstall_queries_use_expanded_result_limit() {
        let parsed = ParsedQuery::parse(">uninstall", true);
        let limit = result_limit_for_query(20, &parsed);
        assert_eq!(limit, UNINSTALL_QUERY_RESULT_LIMIT);

        let non_uninstall = ParsedQuery::parse(">web rust", true);
        let non_limit = result_limit_for_query(20, &non_uninstall);
        assert_eq!(non_limit, 20);
    }

    #[test]
    fn quick_uninstall_shortcut_expands_only_on_initial_u() {
        assert_eq!(
            maybe_expand_uninstall_quick_shortcut(">u", ">"),
            Some(">u ".to_string())
        );
        assert_eq!(maybe_expand_uninstall_quick_shortcut(">u", ">u"), None);
        assert_eq!(
            maybe_expand_uninstall_quick_shortcut(">u", ">u something"),
            None
        );
    }

    #[test]
    fn uninstall_action_title_extracts_target_name() {
        assert_eq!(
            uninstall_target_title_from_action_title("Uninstall Discord"),
            Some("Discord".to_string())
        );
        assert_eq!(
            uninstall_target_title_from_action_title("uninstall   Visual Studio Code  "),
            Some("Visual Studio Code".to_string())
        );
        assert_eq!(
            uninstall_target_title_from_action_title("Open Discord"),
            None
        );
    }

    #[test]
    fn uninstall_title_suppression_tracks_uniques() {
        let mut suppressed = Vec::new();
        track_uninstall_title_suppression(&mut suppressed, "Uninstall Discord");
        track_uninstall_title_suppression(&mut suppressed, "uninstall discord");
        track_uninstall_title_suppression(&mut suppressed, "Open Discord");
        assert_eq!(suppressed, vec!["Discord".to_string()]);
    }

    #[test]
    fn uninstall_confirmation_results_are_confirm_then_cancel() {
        let uninstall_action = SearchItem::new(
            "action:uninstall:discord",
            "action",
            "Uninstall Discord",
            "shell:AppsFolder\\Discord",
        );
        let results = uninstall_confirmation_results(&uninstall_action);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, ACTION_UNINSTALL_CONFIRM_ID);
        assert_eq!(results[1].id, ACTION_UNINSTALL_CANCEL_ID);
        assert!(results[0].title.contains("Discord"));
        assert_eq!(results[1].title, "Cancel");
    }

    #[test]
    fn suppressed_uninstall_results_are_filtered_from_results() {
        let mut results = vec![
            SearchItem::new("app-discord", "app", "Discord", "C:\\Discord\\Discord.exe"),
            SearchItem::new(
                "__nex_action_uninstall__:discord",
                "action",
                "Uninstall Discord",
                "Vendor application",
            ),
            SearchItem::new(
                "app-vscode",
                "app",
                "Visual Studio Code",
                "C:\\Code\\Code.exe",
            ),
            SearchItem::new("file-readme", "file", "readme.md", "C:\\repo\\readme.md"),
        ];
        let suppressed = vec!["Discord".to_string()];
        filter_suppressed_uninstall_results(&mut results, &suppressed);

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|item| item.id != "app-discord"));
        assert!(results
            .iter()
            .all(|item| item.id != "__nex_action_uninstall__:discord"));
        assert!(results.iter().any(|item| item.id == "app-vscode"));
        assert!(results.iter().any(|item| item.id == "file-readme"));
    }

    #[test]
    fn hides_known_start_menu_doc_and_sample_entries() {
        let docs = SearchItem::new(
            "app-docs",
            "app",
            "Documentation Desktop Apps",
            "shell:AppsFolder\\Contoso.DocumentationDesktopApps",
        );
        let sample = SearchItem::new(
            "app-sample",
            "app",
            "Sample UWP Apps",
            "shell:AppsFolder\\Contoso.SampleUwpApps",
        );
        let normal = SearchItem::new(
            "app-normal",
            "app",
            "Discord",
            "shell:AppsFolder\\Discord.Discord",
        );
        let non_shell = SearchItem::new(
            "app-nonshell",
            "app",
            "Sample Tool",
            "C:\\Tools\\SampleTool.exe",
        );
        let manual_lnk = SearchItem::new(
            "app-manual",
            "app",
            "User Manual",
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Tool\\User Manual.lnk",
        );
        let faq_pdf = SearchItem::new(
            "app-faq",
            "app",
            "Tool FAQ",
            "shell:AppsFolder\\Vendor.ToolFAQ.pdf",
        );
        let normal_lnk = SearchItem::new(
            "app-normal-lnk",
            "app",
            "Discord",
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Discord\\Discord.lnk",
        );

        assert!(should_hide_known_start_menu_doc_sample_entry(&docs));
        assert!(should_hide_known_start_menu_doc_sample_entry(&sample));
        assert!(should_hide_known_start_menu_doc_sample_entry(&manual_lnk));
        assert!(should_hide_known_start_menu_doc_sample_entry(&faq_pdf));
        assert!(!should_hide_known_start_menu_doc_sample_entry(&normal));
        assert!(!should_hide_known_start_menu_doc_sample_entry(&non_shell));
        assert!(!should_hide_known_start_menu_doc_sample_entry(&normal_lnk));
    }

    #[test]
    fn prefix_cache_predicate_requires_same_filter_and_extended_query() {
        let cache = IndexedPrefixCache {
            normalized_query: "vi".to_string(),
            indexed_filter: SearchFilter::default(),
            seed_items: vec![SearchItem::new(
                "app-1",
                "app",
                "Vivaldi",
                "C:\\Vivaldi.exe",
            )],
        };

        assert!(can_use_indexed_prefix_cache(
            &cache,
            true,
            "viv",
            &SearchFilter::default()
        ));
        assert!(!can_use_indexed_prefix_cache(
            &cache,
            true,
            "vi",
            &SearchFilter::default()
        ));
        assert!(!can_use_indexed_prefix_cache(
            &cache,
            true,
            "xvi",
            &SearchFilter::default()
        ));

        let different_mode = SearchFilter {
            mode: SearchMode::Apps,
            ..SearchFilter::default()
        };
        assert!(!can_use_indexed_prefix_cache(
            &cache,
            true,
            "viv",
            &different_mode
        ));
        assert!(!can_use_indexed_prefix_cache(
            &cache,
            false,
            "viv",
            &SearchFilter::default()
        ));
    }

    #[test]
    fn game_mode_does_not_block_standard_maximized_apps() {
        let snapshot = ForegroundWindowSnapshot {
            class_name: "Chrome_WidgetWin_1".to_string(),
            process_name: "chrome.exe".to_string(),
            process_path: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe".to_string(),
            covers_monitor: true,
            has_standard_frame: true,
            maximized: true,
        };

        assert!(!should_block_hotkey_for_foreground_window(&snapshot));
    }

    #[test]
    fn game_mode_blocks_known_game_like_borderless_windows() {
        let snapshot = ForegroundWindowSnapshot {
            class_name: "UnrealWindow".to_string(),
            process_name: "VALORANT-Win64-Shipping.exe".to_string(),
            process_path: "C:\\Riot Games\\VALORANT\\live\\ShooterGame\\Binaries\\Win64\\VALORANT-Win64-Shipping.exe".to_string(),
            covers_monitor: true,
            has_standard_frame: false,
            maximized: false,
        };

        assert!(should_block_hotkey_for_foreground_window(&snapshot));
    }

    #[test]
    fn repeated_overlay_query_uses_final_cache() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nex-overlay-cache-{unique}.tmp"));
        std::fs::write(&path, b"ok").expect("temp file should be created");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "item-1",
                "app",
                "Vivaldi",
                path.to_string_lossy().as_ref(),
            ))
            .expect("item should upsert");

        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let parsed = ParsedQuery::parse("vi", true);
        let mut session = OverlaySearchSession::default();

        let first = search_overlay_results_with_session(
            &service,
            &cfg,
            &plugins,
            &parsed,
            20,
            &mut session,
        )
        .expect("first query should succeed");
        let sample_count_after_first = session.indexed_latency_ms.len();

        let second = search_overlay_results_with_session(
            &service,
            &cfg,
            &plugins,
            &parsed,
            20,
            &mut session,
        )
        .expect("second query should succeed");

        assert_eq!(first, second);
        assert_eq!(session.indexed_latency_ms.len(), sample_count_after_first);
        assert!(!session.final_query_cache.is_empty());

        std::fs::remove_file(path).expect("temp file should be removed");
    }

    #[test]
    fn adaptive_seed_limit_reduces_on_high_latency_window() {
        let mut session = OverlaySearchSession::default();
        session
            .indexed_latency_ms
            .extend(std::iter::repeat(170_u128).take(12));

        let base = 320;
        let tuned = adaptive_indexed_seed_limit(&session, 120, 1, base);
        assert!(tuned < base);
        assert!(tuned >= INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT / 2);
        assert!(tuned <= INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT);
    }

    #[test]
    fn parses_background_run_args() {
        let args = vec!["--background".to_string()];
        let options = parse_cli_args(&args).expect("args should parse");
        assert_eq!(
            options,
            RuntimeOptions {
                command: RuntimeCommand::Run,
                background: true,
            }
        );
    }

    #[test]
    fn parses_lifecycle_commands() {
        let args = vec!["--status".to_string()];
        let options = parse_cli_args(&args).expect("status should parse");
        assert_eq!(options.command, RuntimeCommand::Status);
        assert!(!options.background);

        let args = vec!["--status-json".to_string()];
        let options = parse_cli_args(&args).expect("status-json should parse");
        assert_eq!(options.command, RuntimeCommand::StatusJson);
        assert!(!options.background);
    }

    #[test]
    fn parses_diagnostics_bundle_command() {
        let args = vec!["--diagnostics-bundle".to_string()];
        let options = parse_cli_args(&args).expect("diagnostics command should parse");
        assert_eq!(options.command, RuntimeCommand::DiagnosticsBundle);
        assert!(!options.background);
    }

    #[test]
    fn parses_set_launch_at_startup_command() {
        let args = vec!["--set-launch-at-startup=true".to_string()];
        let options = parse_cli_args(&args).expect("startup command should parse");
        assert_eq!(options.command, RuntimeCommand::SetLaunchAtStartup(true));
        assert!(!options.background);

        let args = vec!["--set-launch-at-startup=false".to_string()];
        let options = parse_cli_args(&args).expect("startup command should parse");
        assert_eq!(options.command, RuntimeCommand::SetLaunchAtStartup(false));
        assert!(!options.background);
    }

    #[test]
    fn rejects_invalid_set_launch_at_startup_value() {
        let args = vec!["--set-launch-at-startup=maybe".to_string()];
        let error = parse_cli_args(&args).expect_err("invalid value should fail");
        assert!(error.contains("invalid value for --set-launch-at-startup"));
    }

    #[test]
    fn rejects_background_with_non_run_commands() {
        let args = vec!["--quit".to_string(), "--background".to_string()];
        let error = parse_cli_args(&args).expect_err("invalid combination should fail");
        assert!(error.contains("background mode"));
    }

    #[test]
    fn command_mode_returns_action_results() {
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let parsed = ParsedQuery::parse(">diag", true);
        let results = search_overlay_results(&service, &cfg, &plugins, &parsed, 10)
            .expect("search should succeed");
        assert!(results
            .iter()
            .any(|item| item.id == ACTION_DIAGNOSTICS_BUNDLE_ID));
    }

    #[test]
    fn command_mode_includes_web_search_action() {
        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let parsed = ParsedQuery::parse(">nex roadmap", true);
        let results = search_overlay_results(&service, &cfg, &plugins, &parsed, 10)
            .expect("search should succeed");
        assert!(results
            .iter()
            .any(|item| item.id.starts_with(ACTION_WEB_SEARCH_PREFIX)));
    }

    #[test]
    fn short_single_letter_query_in_all_mode_biases_to_apps() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let app_path = std::env::temp_dir().join(format!("nex-short-query-app-{unique}.tmp"));
        let file_path =
            std::env::temp_dir().join(format!("nex-short-query-file-{unique}.tmp"));
        std::fs::write(&app_path, b"ok").expect("app temp file should be created");
        std::fs::write(&file_path, b"ok").expect("file temp file should be created");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "app-1",
                "app",
                "Vivaldi Browser",
                app_path.to_string_lossy().as_ref(),
            ))
            .expect("app should upsert");
        service
            .upsert_item(&SearchItem::new(
                "file-1",
                "file",
                "Vacation Notes",
                file_path.to_string_lossy().as_ref(),
            ))
            .expect("file should upsert");

        let parsed = ParsedQuery::parse("v", true);
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let results = search_overlay_results(&service, &cfg, &plugins, &parsed, 20)
            .expect("search should succeed");
        assert!(results.iter().any(|item| item.id == "app-1"));
        assert!(!results.iter().any(|item| item.id == "file-1"));

        std::fs::remove_file(app_path).expect("app temp file should be removed");
        std::fs::remove_file(file_path).expect("file temp file should be removed");
    }

    #[test]
    fn short_two_letter_query_in_all_mode_biases_to_apps() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let app_path = std::env::temp_dir().join(format!("nex-short-two-app-{unique}.tmp"));
        let file_path = std::env::temp_dir().join(format!("nex-short-two-file-{unique}.tmp"));
        std::fs::write(&app_path, b"ok").expect("app temp file should be created");
        std::fs::write(&file_path, b"ok").expect("file temp file should be created");

        let service = CoreService::with_connection(Config::default(), open_memory().unwrap())
            .expect("service should initialize");
        service
            .upsert_item(&SearchItem::new(
                "app-1",
                "app",
                "Valorant",
                app_path.to_string_lossy().as_ref(),
            ))
            .expect("app should upsert");
        service
            .upsert_item(&SearchItem::new(
                "file-1",
                "file",
                "Valuation Notes",
                file_path.to_string_lossy().as_ref(),
            ))
            .expect("file should upsert");

        let parsed = ParsedQuery::parse("va", true);
        let cfg = Config::default();
        let plugins = PluginRegistry::default();
        let results = search_overlay_results(&service, &cfg, &plugins, &parsed, 20)
            .expect("search should succeed");
        assert!(results.iter().any(|item| item.id == "app-1"));
        assert!(!results.iter().any(|item| item.id == "file-1"));

        std::fs::remove_file(app_path).expect("app temp file should be removed");
        std::fs::remove_file(file_path).expect("file temp file should be removed");
    }

    #[test]
    fn dedupes_duplicate_app_titles_for_overlay() {
        let mut results = vec![
            SearchItem::new("a1", "app", "Steam", "C:\\One\\Steam.lnk"),
            SearchItem::new("a2", "app", "Steam", "C:\\Two\\Steam.lnk"),
            SearchItem::new("a3", "app", "Calculator", "C:\\Calc.lnk"),
        ];
        dedupe_overlay_results(&mut results);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Steam");
        assert_eq!(results[1].title, "Calculator");
    }

    #[test]
    fn dedupes_non_app_entries_by_normalized_path() {
        let mut results = vec![
            SearchItem::new("f1", "file", "Doc A", "C:/Users/Admin/Docs/test.txt"),
            SearchItem::new("f2", "file", "Doc B", "C:\\Users\\Admin\\Docs\\test.txt"),
            SearchItem::new("f3", "file", "Doc C", "C:\\Users\\Admin\\Docs\\other.txt"),
        ];
        dedupe_overlay_results(&mut results);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "f1");
        assert_eq!(results[1].id, "f3");
    }

    #[test]
    fn dedupes_lnk_file_when_matching_app_title_exists() {
        let mut results = vec![
            SearchItem::new("a1", "app", "Framer", "C:\\ProgramData\\Framer.lnk"),
            SearchItem::new(
                "f1",
                "file",
                "Framer.lnk",
                "C:\\Users\\Admin\\Desktop\\Framer.lnk",
            ),
            SearchItem::new(
                "f2",
                "file",
                "Framer Notes.lnk",
                "C:\\Users\\Admin\\Desktop\\Framer Notes.lnk",
            ),
        ];

        dedupe_overlay_results(&mut results);
        let ids: Vec<&str> = results.iter().map(|item| item.id.as_str()).collect();

        assert_eq!(ids, vec!["a1", "f2"]);
    }

    #[test]
    fn parses_status_diagnostics_snapshot_from_log_content() {
        let content = "\
[0] [INFO] [nex] startup_phase phase=overlay_ready elapsed_ms=41
[0] [INFO] [nex] startup_phase phase=hotkey_ready elapsed_ms=56 hotkey=Ctrl+Space
[0] [INFO] [nex] startup_phase phase=indexing_started elapsed_ms=7 initial_cache_empty=true cached_items=0
[0] [INFO] [nex] startup_phase phase=indexing_completed elapsed_ms=2815 worker_elapsed_ms=2809 indexed_items=310 discovered=320 upserted=16 removed=4
[0] [INFO] [nex] startup_phase phase=cache_applied elapsed_ms=2820 cached_items=310 initial_cache_empty=true
[1] [INFO] [nex] startup indexed_items=310 discovered=320 upserted=16 removed=4
[2] [INFO] [nex] index_provider name=start-menu-apps discovered=120 upserted=4 removed=1 elapsed_ms=42
[3] [INFO] [nex] cache_compaction input_total=812 retained=596 dropped=216 retained_apps=20 retained_file_folders=576 retained_other=0 effective_file_seed_cap=576 broad_root_mode=true active_memory_target_mb=72
[4] [INFO] [nex] overlay_icon_cache reason=cache_clear hits=12 misses=8 load_failures=1 evictions=0 cleared_entries=9 live_entries=0 max_entries=90
";

        let snapshot = parse_status_diagnostics_snapshot(content).expect("snapshot should parse");
        assert!(snapshot
            .overlay_ready_line
            .as_deref()
            .unwrap_or_default()
            .contains("phase=overlay_ready"));
        assert!(snapshot
            .hotkey_ready_line
            .as_deref()
            .unwrap_or_default()
            .contains("phase=hotkey_ready"));
        assert!(snapshot
            .indexing_started_line
            .as_deref()
            .unwrap_or_default()
            .contains("phase=indexing_started"));
        assert!(snapshot
            .indexing_completed_line
            .as_deref()
            .unwrap_or_default()
            .contains("phase=indexing_completed"));
        assert!(snapshot
            .cache_applied_line
            .as_deref()
            .unwrap_or_default()
            .contains("phase=cache_applied"));
        assert!(snapshot
            .startup_index_line
            .as_deref()
            .unwrap_or_default()
            .contains("startup indexed_items=310"));
        assert!(snapshot
            .last_provider_line
            .as_deref()
            .unwrap_or_default()
            .contains("index_provider name=start-menu-apps"));
        assert!(snapshot
            .last_cache_compaction_line
            .as_deref()
            .unwrap_or_default()
            .contains("cache_compaction input_total=812"));
        assert!(snapshot
            .last_icon_cache_line
            .as_deref()
            .unwrap_or_default()
            .contains("overlay_icon_cache reason=cache_clear"));
    }

    #[test]
    fn status_diagnostics_json_includes_startup_lifecycle_tokens() {
        let content = "\
[1773000001] [INFO] [nex] startup_phase phase=overlay_ready elapsed_ms=33
[1773000002] [INFO] [nex] startup_phase phase=hotkey_ready elapsed_ms=48 hotkey=Ctrl+Space
[1773000003] [INFO] [nex] startup_phase phase=indexing_started elapsed_ms=6 initial_cache_empty=true cached_items=0
[1773000028] [INFO] [nex] startup_phase phase=indexing_completed elapsed_ms=2600 worker_elapsed_ms=2593 indexed_items=310 discovered=320 upserted=16 removed=4
[1773000029] [INFO] [nex] startup_phase phase=cache_applied elapsed_ms=2605 cached_items=310 initial_cache_empty=true
[1773000030] [INFO] [nex] cache_compaction input_total=812 retained=596 dropped=216 retained_apps=20 retained_file_folders=576 retained_other=0 effective_file_seed_cap=576 broad_root_mode=true active_memory_target_mb=72
[1773000031] [INFO] [nex] overlay_icon_cache reason=cache_clear hits=12 misses=8 load_failures=1 evictions=0 cleared_entries=9 live_entries=0 max_entries=90
";
        let snapshot = parse_status_diagnostics_snapshot(content).expect("snapshot should parse");
        let json = build_status_diagnostics_json(&snapshot);

        assert_eq!(
            json["startup_lifecycle"]["overlay_ready"]["tokens"]["elapsed_ms"],
            serde_json::json!(33)
        );
        assert_eq!(
            json["startup_lifecycle"]["hotkey_ready"]["tokens"]["hotkey"],
            serde_json::json!("Ctrl+Space")
        );
        assert_eq!(
            json["startup_lifecycle"]["indexing_started"]["tokens"]["initial_cache_empty"],
            serde_json::json!(true)
        );
        assert_eq!(
            json["startup_lifecycle"]["indexing_completed"]["tokens"]["worker_elapsed_ms"],
            serde_json::json!(2593)
        );
        assert_eq!(
            json["startup_lifecycle"]["cache_applied"]["tokens"]["cached_items"],
            serde_json::json!(310)
        );
        assert_eq!(
            json["startup_lifecycle"]["cache_applied"]["epoch_secs"],
            serde_json::json!(1773000029_u64)
        );
        assert_eq!(
            json["cache_compaction"]["effective_file_seed_cap"],
            serde_json::json!(576)
        );
        assert_eq!(
            json["cache_compaction"]["broad_root_mode"],
            serde_json::json!(true)
        );
        assert_eq!(
            json["icon_cache"]["max_entries"],
            serde_json::json!(90)
        );
    }

    #[test]
    fn returns_none_for_status_snapshot_without_diagnostics_tokens() {
        let content = "[1] [INFO] [nex] status: running\n";
        assert!(parse_status_diagnostics_snapshot(content).is_none());
    }

    #[test]
    fn summarizes_query_profiles_from_log_content() {
        let content = "\
[1] [INFO] [nex] query_profile q=\"v\" mode=all candidate_limit=60 indexed_seed_limit=240 short_app_bias=true indexed_cache_hit=false indexed_count=20 indexed_ms=20 provider_count=0 provider_ms=0 action_count=0 action_ms=0 built_in_actions=0 plugin_actions=0 clipboard_count=0 clipboard_ms=0 rank_ms=0 total_ms=21
[2] [INFO] [nex] query_profile q=\"va\" mode=all candidate_limit=80 indexed_seed_limit=160 short_app_bias=true indexed_cache_hit=false indexed_count=20 indexed_ms=26 provider_count=0 provider_ms=0 action_count=0 action_ms=0 built_in_actions=0 plugin_actions=0 clipboard_count=0 clipboard_ms=0 rank_ms=0 total_ms=27
[3] [INFO] [nex] query_profile q=\"vala\" mode=all candidate_limit=120 indexed_seed_limit=240 short_app_bias=false indexed_cache_hit=false indexed_count=20 indexed_ms=54 provider_count=0 provider_ms=0 action_count=0 action_ms=0 built_in_actions=0 plugin_actions=0 clipboard_count=0 clipboard_ms=0 rank_ms=0 total_ms=55
";
        let summary = summarize_query_profiles(content).expect("summary should parse");
        assert_eq!(summary.samples, 3);
        assert_eq!(summary.p95_total_ms, 55);
        assert_eq!(summary.short_query_samples, 2);
        assert_eq!(summary.short_query_app_bias_rate_pct, 100);
        assert_eq!(summary.short_query_p95_total_ms, 27);
    }

    #[test]
    fn skips_non_searchable_symbol_only_query() {
        let parsed = ParsedQuery::parse("-", true);
        let normalized = crate::model::normalize_for_search(parsed.free_text.trim());
        assert!(should_skip_non_searchable_query(&parsed, &normalized));

        let parsed_command = ParsedQuery::parse(">-", true);
        let normalized_command =
            crate::model::normalize_for_search(parsed_command.free_text.trim());
        assert!(!should_skip_non_searchable_query(
            &parsed_command,
            &normalized_command
        ));
    }

    #[test]
    fn parses_tasklist_pid_lines_from_list_output() {
        let content = "\
Image Name:   nex.exe
PID:          1124
Session Name: Console

Image Name:   nex.exe
PID:          2208
Session Name: Console
";
        let pids = parse_tasklist_pid_lines(content);
        assert_eq!(pids, vec![1124, 2208]);
    }
}
