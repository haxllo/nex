#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::action_registry::{
    ACTION_CHECK_UPDATES_ID, ACTION_CLEAR_CLIPBOARD_ID, ACTION_DIAGNOSTICS_BUNDLE_ID,
    ACTION_OPEN_CONFIG_ID, ACTION_OPEN_LOGS_ID, ACTION_REBUILD_INDEX_ID, ACTION_TRIM_MEMORY_ID,
    ACTION_WEB_SEARCH_PREFIX,
};
use crate::clipboard_history;
use crate::config::Config;
use crate::core_service::{CoreService, LaunchTarget};
use crate::model::SearchItem;
use crate::plugin_sdk::{PluginActionKind, PluginRegistry};
use crate::query_dsl::ParsedQuery;
use crate::runtime::{log_info, log_warn};
use crate::runtime_overlay_rows::{
    uninstall_target_title_from_action_title, ACTION_UNINSTALL_CANCEL_ID,
    ACTION_UNINSTALL_CONFIRM_ID,
};
use crate::runtime_search_session::resolved_mode_for_query;

pub(crate) fn should_suppress_failed_uninstall(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("shell_code=2")
        || lower.contains(" code 2")
        || lower.contains("no longer available")
        || lower.contains("file not found")
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(crate) fn uninstall_confirmation_results(uninstall_action: &SearchItem) -> Vec<SearchItem> {
    let target = uninstall_target_title_from_action_title(uninstall_action.title.as_str())
        .unwrap_or_else(|| uninstall_action.title.trim().to_string());
    let confirm_title = if target.is_empty() {
        "Confirm uninstall".to_string()
    } else {
        format!("Confirm uninstall {}", target.trim())
    };

    vec![
        SearchItem::new(
            ACTION_UNINSTALL_CONFIRM_ID,
            "action",
            confirm_title.as_str(),
            "Open app uninstaller",
        ),
        SearchItem::new(
            ACTION_UNINSTALL_CANCEL_ID,
            "action",
            "Cancel",
            "Return to previous results",
        ),
    ]
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn launch_overlay_selection(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    results: &[SearchItem],
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
        .map_err(|error| format!("launch failed: {error}"))?;

    // Record the launch for Quick Launch usage tracking
    if selected.kind.eq_ignore_ascii_case("app") {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let db = service.db_ref();
        if let Err(error) = crate::index_store::record_launch(&db, &selected.id, now) {
            crate::runtime::log_warn(&format!("[nex] record_launch failed: {error}"));
        }
    }

    Ok(())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn execute_action_selection(
    service: &CoreService,
    cfg: &Config,
    plugins: &PluginRegistry,
    selected: &SearchItem,
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
                report.indexed_total,
                report.discovered_total,
                report.upserted_total,
                report.removed_total
            ));
            Ok(())
        }
        ACTION_CLEAR_CLIPBOARD_ID => clipboard_history::clear_history(cfg),
        ACTION_OPEN_CONFIG_ID => {
            crate::action_executor::launch_path(cfg.config_path.to_string_lossy().as_ref())
                .map_err(|error| format!("open config failed: {error}"))
        }
        ACTION_DIAGNOSTICS_BUNDLE_ID => {
            let output_dir = crate::runtime::write_diagnostics_bundle(cfg)
                .map_err(|error| format!("diagnostics bundle failed: {error}"))?;
            log_info(&format!(
                "[nex] diagnostics bundle written to {}",
                output_dir.display()
            ));
            Ok(())
        }
        ACTION_CHECK_UPDATES_ID => crate::runtime_process::launch_stable_updater()
            .map(|_| ())
            .map_err(|error| format!("check for updates failed: {error}")),
        ACTION_TRIM_MEMORY_ID => {
            log_info("[nex] trim memory action invoked");
            Ok(())
        }
        _ => execute_plugin_action(cfg, plugins, &selected.id),
    }
}

pub(crate) fn execute_plugin_action(
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
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn()
                .map_err(|e| format!("plugin command spawn failed: {e}"))?;
            Ok(())
        }
    }
}
