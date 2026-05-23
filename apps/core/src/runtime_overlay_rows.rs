use crate::model::{self, SearchItem};
use crate::runtime::log_warn;
use crate::uninstall_registry;
#[cfg(target_os = "windows")]
use crate::windows_overlay::{NativeOverlayShell, OverlayRow, OverlayRowRole};

#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_NO_RESULTS: &str = "No results";
#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_NO_COMMAND_RESULTS: &str = "No command matches";
#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_TYPE_TO_SEARCH: &str = "Start typing to search";
#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_INDEXING: &str = "Indexing in background...";
#[cfg(target_os = "windows")]
pub(crate) const STATUS_TEXT_INDEX_READY: &str = "Index ready";
pub(crate) const ACTION_UNINSTALL_CONFIRM_ID: &str = "action:uninstall:confirm";
pub(crate) const ACTION_UNINSTALL_CANCEL_ID: &str = "action:uninstall:cancel";

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
pub(crate) struct PendingUninstallConfirmation {
    pub(crate) uninstall_action: SearchItem,
    pub(crate) previous_results: Vec<SearchItem>,
    pub(crate) previous_selected_index: usize,
    pub(crate) previous_command_mode: bool,
}

#[cfg(target_os = "windows")]
pub(crate) fn overlay_rows(results: &[SearchItem], command_mode: bool) -> Vec<OverlayRow> {
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
pub(crate) fn append_group_rows(
    rows: &mut Vec<OverlayRow>,
    indices: &[usize],
    results: &[SearchItem],
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
pub(crate) fn result_row(
    item: &SearchItem,
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
pub(crate) fn dedupe_overlay_results(results: &mut Vec<SearchItem>) {
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
pub(crate) fn should_hide_known_start_menu_doc_sample_entry(item: &SearchItem) -> bool {
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
pub(crate) fn has_non_app_document_extension(value: &str) -> bool {
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
pub(crate) fn normalize_title_key(title: &str) -> String {
    model::normalize_for_search(title.trim())
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn shortcut_base_title_key(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.len() >= 4 && trimmed[trimmed.len() - 4..].eq_ignore_ascii_case(".lnk") {
        normalize_title_key(&trimmed[..trimmed.len() - 4])
    } else {
        normalize_title_key(trimmed)
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_windows_shortcut_path(path: &str) -> bool {
    let trimmed = path.trim();
    trimmed.len() >= 4 && trimmed[trimmed.len() - 4..].eq_ignore_ascii_case(".lnk")
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn normalize_path_key(path: &str) -> String {
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
pub(crate) fn track_uninstall_title_suppression(
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
pub(crate) fn reconcile_suppressed_uninstall_titles(suppressed_uninstall_titles: &mut Vec<String>) {
    if suppressed_uninstall_titles.is_empty() {
        return;
    }

    suppressed_uninstall_titles.retain(
        |title| match uninstall_registry::is_display_name_registered(title.as_str()) {
            Ok(still_registered) => still_registered,
            Err(error) => {
                log_warn(&format!(
                    "[nex] uninstall suppression registry check failed for '{}': {}",
                    title, error
                ));
                true
            }
        },
    );
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn filter_suppressed_uninstall_results(
    results: &mut Vec<SearchItem>,
    suppressed_uninstall_titles: &[String],
) {
    if results.is_empty() || suppressed_uninstall_titles.is_empty() {
        return;
    }

    let suppressed_keys: Vec<String> = suppressed_uninstall_titles
        .iter()
        .map(|title| model::normalize_for_search(title.as_str()))
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
                .starts_with(uninstall_registry::ACTION_UNINSTALL_PREFIX)
        {
            uninstall_target_title_from_action_title(item.title.as_str())
                .map(|title| model::normalize_for_search(title.as_str()))
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
pub(crate) fn uninstall_target_title_from_action_title(action_title: &str) -> Option<String> {
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
pub(crate) fn uninstall_title_matches(app_title_key: &str, suppressed_key: &str) -> bool {
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
pub(crate) fn overlay_subtitle(item: &SearchItem, command_mode: bool) -> String {
    if command_mode
        && item.kind.eq_ignore_ascii_case("action")
        && !item
            .id
            .starts_with(uninstall_registry::ACTION_UNINSTALL_PREFIX)
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
pub(crate) fn abbreviate_path(path: &str) -> String {
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
pub(crate) fn set_idle_overlay_state(overlay: &NativeOverlayShell) {
    overlay.clear_placeholder_hint();
    overlay.set_results(&[], 0);
    overlay.set_status_text("");
}

#[cfg(target_os = "windows")]
pub(crate) fn set_status_row_overlay_state(overlay: &NativeOverlayShell, message: &str) {
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

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn next_selection_index(current: usize, len: usize, direction: i32) -> usize {
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
