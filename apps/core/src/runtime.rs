use crate::config::{self, ConfigError};
use crate::core_service::{CoreService, ServiceError};
use crate::hotkey_runtime::HotkeyRuntimeError;
use crate::runtime_commands::{
    command_ensure_config, command_quit, command_restart, command_set_launch_at_startup,
    command_status, command_sync_startup,
};
#[cfg(target_os = "windows")]
use crate::runtime_loop::run_windows_runtime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

// Re-exports from runtime_diagnostics module (extracted for modularity)
#[cfg(test)]
pub(crate) use crate::runtime_actions::{launch_overlay_selection, uninstall_confirmation_results};
#[cfg(test)]
pub(crate) use crate::runtime_diagnostics::{
    build_status_diagnostics_json, parse_status_diagnostics_snapshot, summarize_query_profiles,
};
pub(crate) use crate::runtime_diagnostics::{
    command_diagnostics_bundle, command_probe_everything, command_status_json, env_var_with_legacy,
    load_query_profile_status_report, load_status_diagnostics_snapshot, write_diagnostics_bundle,
};
#[cfg(test)]
pub(crate) use crate::runtime_overlay_rows::{
    dedupe_overlay_results, filter_suppressed_uninstall_results, next_selection_index,
    should_hide_known_start_menu_doc_sample_entry, track_uninstall_title_suppression,
    uninstall_target_title_from_action_title, ACTION_UNINSTALL_CANCEL_ID,
    ACTION_UNINSTALL_CONFIRM_ID,
};
#[cfg(test)]
pub(crate) use crate::runtime_search_session::{
    adaptive_indexed_seed_limit, can_use_indexed_prefix_cache, candidate_limit_for_query,
    maybe_expand_uninstall_quick_shortcut, result_limit_for_query, search_overlay_results,
    search_overlay_results_with_session, should_skip_non_searchable_query, IndexedPrefixCache,
    OverlaySearchSession, INDEXED_PREFIX_CACHE_MAX_SEED_LIMIT, INDEXED_PREFIX_CACHE_MIN_SEED_LIMIT,
};

#[cfg(test)]
pub(crate) use crate::runtime_process::{
    hotkey_registration_recovery_message, hotkey_registration_status_text, parse_tasklist_pid_lines,
};
#[cfg(target_os = "windows")]
use crate::runtime_process::{runtime_mode, spawn_background_process};

#[cfg(test)]
pub(crate) use crate::runtime_hotkey::{
    should_block_hotkey_for_foreground_window, ForegroundWindowSnapshot,
};

#[cfg(test)]
pub(crate) use crate::runtime_index::queued_discovery_reindex_is_due;

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(crate) const UNINSTALL_QUERY_RESULT_LIMIT: usize = 160;

static STDIO_LOGGING_ENABLED: AtomicBool = AtomicBool::new(true);

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
    ProbeEverything,
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
            "--probe-everything" => options.command = RuntimeCommand::ProbeEverything,
            "--help" | "-h" => {
                return Err(
                    "usage: nex [--background|--foreground] [--status|--status-json|--quit|--restart|--ensure-config|--sync-startup|--set-launch-at-startup=true|false|--diagnostics-bundle|--probe-everything]".to_string(),
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
        RuntimeCommand::ProbeEverything => return command_probe_everything(),
        RuntimeCommand::Run => {}
    }

    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    let startup_started_at = Instant::now();
    let runtime_config = config::load(None)?;
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
    {
        run_windows_runtime(startup_started_at, runtime_config, service)
    }
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
        log_info("[nex] non-windows runtime mode: no global hotkey loop");
        Ok(())
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

pub(crate) fn log_info(message: &str) {
    if should_log_to_stdio() {
        println!("{message}");
    }
    crate::logging::info(message);
}

pub(crate) fn log_warn(message: &str) {
    if should_log_to_stdio() {
        eprintln!("{message}");
    }
    crate::logging::warn(message);
}

#[cfg(test)]
mod tests {
    use super::{
        adaptive_indexed_seed_limit, build_status_diagnostics_json, can_use_indexed_prefix_cache,
        candidate_limit_for_query, dedupe_overlay_results, filter_suppressed_uninstall_results,
        hotkey_registration_recovery_message, hotkey_registration_status_text,
        launch_overlay_selection, maybe_expand_uninstall_quick_shortcut, next_selection_index,
        parse_cli_args, parse_status_diagnostics_snapshot, parse_tasklist_pid_lines,
        queued_discovery_reindex_is_due, result_limit_for_query, search_overlay_results,
        search_overlay_results_with_session, should_block_hotkey_for_foreground_window,
        should_hide_known_start_menu_doc_sample_entry, should_skip_non_searchable_query,
        summarize_query_profiles, track_uninstall_title_suppression,
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
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
                "app",
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
        let file_path = std::env::temp_dir().join(format!("nex-short-query-file-{unique}.tmp"));
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
[0] [WARN] [nex] hotkey_registration_issue hotkey=Ctrl+Space suggestions=Ctrl+Shift+Space|Alt+Space error=conflict
[0] [INFO] [nex] startup_phase phase=indexing_started elapsed_ms=7 initial_cache_empty=true cached_items=0
[0] [INFO] [nex] startup_phase phase=indexing_completed elapsed_ms=2815 worker_elapsed_ms=2809 indexed_items=310 discovered=320 upserted=16 removed=4
[0] [INFO] [nex] startup_phase phase=cache_applied elapsed_ms=2820 cached_items=310 initial_cache_empty=true
[1] [INFO] [nex] startup indexed_items=310 discovered=320 upserted=16 removed=4
[2] [INFO] [nex] index_provider name=start-menu-apps discovered=120 upserted=4 removed=1 elapsed_ms=42
[3] [INFO] [nex] provider_freshness name=filesystem skipped=false last_scan_age_secs=0 reconcile_interval_secs=1800 has_stamp=true
[4] [INFO] [nex] stale_prune scanned=512 removed=3 cached_items_remaining=738
[5] [INFO] [nex] cache_compaction input_total=812 retained=596 dropped=216 retained_apps=20 retained_file_folders=576 retained_other=0 effective_file_seed_cap=576 broad_root_mode=true active_memory_target_mb=72
[6] [INFO] [nex] overlay_icon_cache reason=cache_clear hits=12 misses=8 load_failures=1 evictions=0 cleared_entries=9 live_entries=0 max_entries=90
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
            .hotkey_registration_issue_line
            .as_deref()
            .unwrap_or_default()
            .contains("hotkey_registration_issue hotkey=Ctrl+Space"));
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
            .last_provider_freshness_line
            .as_deref()
            .unwrap_or_default()
            .contains("provider_freshness name=filesystem"));
        assert!(snapshot
            .last_stale_prune_line
            .as_deref()
            .unwrap_or_default()
            .contains("stale_prune scanned=512"));
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
[1773000002] [WARN] [nex] hotkey_registration_issue hotkey=Ctrl+Space suggestions=Ctrl+Shift+Space|Alt+Space error=conflict
[1773000003] [INFO] [nex] startup_phase phase=indexing_started elapsed_ms=6 initial_cache_empty=true cached_items=0
[1773000028] [INFO] [nex] startup_phase phase=indexing_completed elapsed_ms=2600 worker_elapsed_ms=2593 indexed_items=310 discovered=320 upserted=16 removed=4
[1773000029] [INFO] [nex] startup_phase phase=cache_applied elapsed_ms=2605 cached_items=310 initial_cache_empty=true
[1773000030] [INFO] [nex] provider_freshness name=filesystem skipped=false last_scan_age_secs=0 reconcile_interval_secs=1800 has_stamp=true
[1773000031] [INFO] [nex] stale_prune scanned=512 removed=3 cached_items_remaining=738
[1773000032] [INFO] [nex] cache_compaction input_total=812 retained=596 dropped=216 retained_apps=20 retained_file_folders=576 retained_other=0 effective_file_seed_cap=576 broad_root_mode=true active_memory_target_mb=72
[1773000033] [INFO] [nex] overlay_icon_cache reason=cache_clear hits=12 misses=8 load_failures=1 evictions=0 cleared_entries=9 live_entries=0 max_entries=90
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
            json["hotkey_issue"]["tokens"]["suggestions"],
            serde_json::json!("Ctrl+Shift+Space|Alt+Space")
        );
        assert_eq!(
            json["hotkey_issue"]["epoch_secs"],
            serde_json::json!(1773000002_u64)
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
            json["provider_freshness"]["reconcile_interval_secs"],
            serde_json::json!(1800)
        );
        assert_eq!(json["stale_prune"]["removed"], serde_json::json!(3));
        assert_eq!(
            json["cache_compaction"]["effective_file_seed_cap"],
            serde_json::json!(576)
        );
        assert_eq!(
            json["cache_compaction"]["broad_root_mode"],
            serde_json::json!(true)
        );
        assert_eq!(json["icon_cache"]["max_entries"], serde_json::json!(90));
    }

    #[test]
    fn queued_reindex_starts_only_after_due_time() {
        let now = Instant::now();
        assert!(!queued_discovery_reindex_is_due(
            true,
            true,
            Some(now + std::time::Duration::from_millis(5)),
            now
        ));
        assert!(queued_discovery_reindex_is_due(true, true, Some(now), now));
        assert!(!queued_discovery_reindex_is_due(
            true,
            false,
            Some(now),
            now
        ));
        assert!(!queued_discovery_reindex_is_due(
            false,
            true,
            Some(now),
            now
        ));
        assert!(!queued_discovery_reindex_is_due(true, true, None, now));
    }

    #[test]
    fn returns_none_for_status_snapshot_without_diagnostics_tokens() {
        let content = "[1] [INFO] [nex] status: running\n";
        assert!(parse_status_diagnostics_snapshot(content).is_none());
    }

    #[test]
    fn hotkey_registration_messages_include_recovery_guidance() {
        let message = hotkey_registration_recovery_message(
            "Ctrl+Space",
            std::path::Path::new("C:\\Users\\Admin\\AppData\\Roaming\\Nex\\config.toml"),
        );
        assert!(message.contains("Hotkey 'Ctrl+Space' is unavailable."));
        assert!(message.contains("Ctrl+Shift+Space"));
        assert!(message.contains("config.toml"));

        let status = hotkey_registration_status_text("Ctrl+Space");
        assert!(status.contains("Hotkey unavailable: Ctrl+Space."));
        assert!(status.contains("Ctrl+Shift+Space"));
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
