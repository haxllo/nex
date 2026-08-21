use crate::model::{self, SearchItem};
use crate::runtime::log_warn;
use crate::uninstall_registry;
#[cfg(target_os = "windows")]
use crate::overlay::{NativeOverlayShell, OverlayRow, OverlayRowRole};

#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_NO_RESULTS: &str = "No results";
/// Result limit used when the user activates "Show all apps".
pub(crate) const SHOW_ALL_APPS_RESULT_LIMIT: usize = 100;
#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_NO_COMMAND_RESULTS: &str = "No command matches";
#[cfg(target_os = "windows")]
pub(crate) const STATUS_ROW_TYPE_TO_SEARCH: &str = "Start typing to search";

pub(crate) const ACTION_UNINSTALL_CONFIRM_ID: &str = "action:uninstall:confirm";
pub(crate) const ACTION_UNINSTALL_CANCEL_ID: &str = "action:uninstall:cancel";
pub(crate) const ACTION_POWER_CONFIRM_ID: &str = "action:power:confirm";
pub(crate) const ACTION_POWER_CANCEL_ID: &str = "action:power:cancel";

/// What kind of action the user is being asked to confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmationKind {
    Uninstall,
    Shutdown,
    Restart,
    SignOut,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
pub(crate) struct PendingConfirmation {
    pub(crate) kind: ConfirmationKind,
    pub(crate) uninstall_action: Option<SearchItem>,
    pub(crate) previous_results: Vec<SearchItem>,
    pub(crate) previous_selected_index: usize,
    pub(crate) previous_command_mode: bool,
}

/// Kind rendering order: apps first, then folders, files, actions, clipboard, other.
/// This SUPERSEDES the previous tier-beats-kind bucket structure.
fn kind_group_order(kind: &str) -> u8 {
    if kind.eq_ignore_ascii_case("app") {
        0
    } else if kind.eq_ignore_ascii_case("folder") {
        1
    } else if kind.eq_ignore_ascii_case("file") {
        2
    } else if kind.eq_ignore_ascii_case("action") {
        3
    } else if kind.eq_ignore_ascii_case("clipboard") {
        4
    } else {
        5
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn overlay_rows(results: &[SearchItem], command_mode: bool) -> Vec<OverlayRow> {
    overlay_rows_ext(results, command_mode, false)
}

/// Build overlay rows, optionally appending a synthetic "Show all apps"
/// entry after the last app row (before the Folders header). Activating
/// it re-runs the query apps-only with a larger limit.
pub(crate) fn overlay_rows_ext(
    results: &[SearchItem],
    command_mode: bool,
    show_all_apps_entry: bool,
) -> Vec<OverlayRow> {
    if results.is_empty() {
        return Vec::new();
    }

    if command_mode {
        // A4: actions render in score-descending deterministic order.
        // Results arrive pre-sorted by the scoring pipeline; enumerate+
        // map preserves that order exactly. No regroup or kind ordering
        // is applied in command mode — actions stay in score order.
        return results
            .iter()
            .enumerate()
            .map(|(index, item)| result_row(item, index, OverlayRowRole::Item, command_mode))
            .collect();
    }

    // Select TopHit index first (A2/A3), independent of kind-group rebuild.
    let top_hit_index = select_top_hit_index(results);

    // Only mark top hit as "consumed" when it IS an app — its TopHit row is
    // emitted below.  Non-app top hits (files, folders, action rows) stay in
    // their normal kind bucket so they render under their section header.
    let top_hit_is_app = results[top_hit_index].kind.eq_ignore_ascii_case("app");

    // Group indices by kind, then sort within each kind by tier
    // (0=Exact → 3=Fuzzy), preserving original index for stability.
    let mut kind_buckets: [Vec<usize>; 6] = Default::default();

    for (index, item) in results.iter().enumerate() {
        if top_hit_is_app && index == top_hit_index {
            continue;
        }
        let gi = kind_group_order(&item.kind) as usize;
        kind_buckets[gi].push(index);
    }

    // Sort each kind bucket by tier (ascending = better tier first), then
    // original index for stability (lower index = earlier in score order).
    for bucket in &mut kind_buckets {
        bucket.sort_by(|&a, &b| {
            let tier_a = results[a].match_tier.unwrap_or(3);
            let tier_b = results[b].match_tier.unwrap_or(3);
            tier_a.cmp(&tier_b).then(a.cmp(&b))
        });
    }

    let mut rows = Vec::new();

    // Emit TopHit row only when the top hit is an app — the standalone
    // TopHit slot is the app presentation (grid cell, no section header).
    // When no apps are present, folders/files belong under their own
    // section headers (Folders/Files) instead of floating alone above them.
    if results[top_hit_index].kind.eq_ignore_ascii_case("app") {
        rows.push(result_row(
            &results[top_hit_index],
            top_hit_index,
            OverlayRowRole::TopHit,
            command_mode,
        ));
    }

    // Emit remaining rows in kind-first order.
    // Apps: no section header (current visual contract — first app was TopHit,
    // remaining apps render as Item rows without a header).
    for &index in &kind_buckets[0] {
        rows.push(result_row(
            &results[index],
            index,
            OverlayRowRole::Item,
            command_mode,
        ));
    }
    // Synthetic "Show all apps" entry — last row of the app group. The
    // runtime intercepts its activation and re-issues the query with an
    // apps-only kind filter and a larger limit.
    if show_all_apps_entry && !kind_buckets[0].is_empty() {
        rows.push(OverlayRow {
            role: OverlayRowRole::ShowAllApps,
            result_index: None,
            kind: "action".to_string(),
            title: "Show all apps".to_string(),
            path: String::new(),
            icon_path: String::new(),
        });
    }
    append_group_rows(&mut rows, "Folders", &kind_buckets[1], results, command_mode);
    append_group_rows(&mut rows, "Files", &kind_buckets[2], results, command_mode);
    append_group_rows(&mut rows, "Actions", &kind_buckets[3], results, command_mode);
    append_group_rows(&mut rows, "Clipboard", &kind_buckets[4], results, command_mode);
    append_group_rows(&mut rows, "Other", &kind_buckets[5], results, command_mode);

    rows
}

/// Select the TopHit index for the overlay row 0 position.
///
/// A2: When any app exists in results, TopHit = best-scored app.
/// A3: When no apps exist, TopHit = best-scored folder or file (highest tier,
///     tie → original order preserved, i.e. lower index wins).
/// "Best" = lowest match_tier value (0=Exact best, 3=Fuzzy worst).
///     Tie → original earlier index (results arrive score-sorted, so lower
///     index = higher score).
fn select_top_hit_index(results: &[SearchItem]) -> usize {
    // Prefer best app when any app exists (A2).
    let has_app = results.iter().any(|item| item.kind.eq_ignore_ascii_case("app"));
    if has_app {
        return results
            .iter()
            .enumerate()
            .filter(|(_, item)| item.kind.eq_ignore_ascii_case("app"))
            .min_by_key(|(idx, item)| (item.match_tier.unwrap_or(3), *idx))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
    }

    // No apps: fallback to best folder or file (A3).
    // Folders and files at equal tier retain original relative order (stable).
    results
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.kind.eq_ignore_ascii_case("folder") || item.kind.eq_ignore_ascii_case("file")
        })
        .min_by_key(|(idx, item)| (item.match_tier.unwrap_or(3), *idx))
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
pub(crate) fn append_group_rows(
    rows: &mut Vec<OverlayRow>,
    label: &str,
    indices: &[usize],
    results: &[SearchItem],
    command_mode: bool,
) {
    if indices.is_empty() {
        return;
    }
    rows.push(header_row(label));
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
        result_index: Some(result_index),
        kind: item.kind.clone(),
        title: item.title.clone(),
        path: overlay_subtitle(item, command_mode),
        icon_path: item.path.clone(),
    }
}

#[cfg(target_os = "windows")]
fn header_row(label: &str) -> OverlayRow {
    OverlayRow {
        role: OverlayRowRole::Header,
        // `None` signals "no backing result index"; header rows are
        // not selectable and do not contribute to the
        // result->row mapping.
        result_index: None,
        kind: String::new(),
        title: label.to_string(),
        path: String::new(),
        icon_path: String::new(),
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
    // Always hide shell: URIs — they're internal implementation paths.
    let path = item.path.trim();
    let is_shell = path.starts_with("shell:");
    if item.kind.eq_ignore_ascii_case("app") {
        let s = item.subtitle.trim();
        if s.is_empty() || s.contains('\\') || s.contains('/') || s.contains(':') {
            return String::new();
        }
        return s.to_string();
    }
    if item.kind.eq_ignore_ascii_case("action") {
        if path.is_empty() {
            return "Nex action".to_string();
        }
        return path.to_string();
    }
    if is_shell {
        return String::new();
    }
    // ms-settings: URIs and Control Panel .cpl dialogs are launch
    // targets, not display paths.
    if path.starts_with("ms-settings:") || path.ends_with(".cpl") {
        return String::new();
    }
    abbreviate_path(path)
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

/// Sanitize subtitle for Quick Launch items. Follows the same
/// semantics as `overlay_subtitle()` for `kind == "app"`: drop
/// empty/whitespace-only subtitles and those containing path
/// separators or drive-letter colons.
fn quick_launch_subtitle(subtitle: &str) -> String {
    let s = subtitle.trim();
    if s.is_empty() || s.contains('\\') || s.contains('/') || s.contains(':') {
        String::new()
    } else {
        s.to_string()
    }
}

/// Build Quick Launch rows for the idle state (empty query).
/// Returns rows with QuickLaunch role, ready to be pushed to the overlay.
#[cfg(target_os = "windows")]
pub(crate) fn build_quick_launch_rows(
    quick_launch_items: &[crate::overlay::model::QuickLaunchItem],
) -> Vec<OverlayRow> {
    quick_launch_items
        .iter()
        .enumerate()
        .map(|(index, item)| OverlayRow {
            role: OverlayRowRole::QuickLaunch,
            result_index: Some(index),
            kind: "app".to_string(),
            title: item.title.clone(),
            path: quick_launch_subtitle(&item.subtitle),
            icon_path: item.icon_path.clone(),
        })
        .collect()
}

/// Set the overlay to show Quick Launch items in idle state.
#[cfg(target_os = "windows")]
pub(crate) fn set_quick_launch_overlay_state(
    overlay: &NativeOverlayShell,
    quick_launch_items: &[crate::overlay::model::QuickLaunchItem],
) {
    overlay.clear_placeholder_hint();
    let rows = build_quick_launch_rows(quick_launch_items);
    overlay.set_results(&rows, 0);
    overlay.set_status_text("");
}

#[cfg(target_os = "windows")]
pub(crate) fn set_status_row_overlay_state(overlay: &NativeOverlayShell, message: &str) {
    overlay.clear_placeholder_hint();
    let rows = [OverlayRow {
        role: OverlayRowRole::Status,
        result_index: None,
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

#[cfg(test)]
#[cfg(target_os = "windows")]
mod tests {
    use super::*;
    use crate::overlay::OverlayRowRole;

    fn app(id: &str, title: &str, tier: u8) -> SearchItem {
        SearchItem::new(id, "app", title, "").with_match_tier(tier)
    }
    fn folder(id: &str, title: &str, tier: u8) -> SearchItem {
        SearchItem::new(id, "folder", title, "").with_match_tier(tier)
    }
    fn file(id: &str, title: &str, tier: u8) -> SearchItem {
        SearchItem::new(id, "file", title, "").with_match_tier(tier)
    }
    fn action(id: &str, title: &str, tier: u8) -> SearchItem {
        SearchItem::new(id, "action", title, "").with_match_tier(tier)
    }
    fn clipboard(id: &str, title: &str, tier: u8) -> SearchItem {
        SearchItem::new(id, "clipboard", title, "").with_match_tier(tier)
    }

    fn role_of(row: &OverlayRow) -> &OverlayRowRole {
        &row.role
    }

    /// (a) Fuzzy app (tier 3) renders BEFORE exact folder (tier 0).
    #[test]
    fn fuzzy_app_before_exact_folder() {
        let results = vec![folder("f1", "folder", 0), app("a1", "app", 3)];
        let rows = overlay_rows(&results, false);

        // TopHit (app) + Folders header + folder item = 3 rows.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].title, "app");
        assert_eq!(*role_of(&rows[0]), OverlayRowRole::TopHit);
        assert_eq!(rows[1].title, "Folders"); // header
        assert_eq!(*role_of(&rows[1]), OverlayRowRole::Header);
        assert_eq!(rows[2].title, "folder");
        assert_eq!(*role_of(&rows[2]), OverlayRowRole::Item);
    }

    /// (b) No apps → top hit is best file/folder (A3).
    ///     Folder (tier=3) at index 0 must NOT become TopHit — the exact
    ///     file (tier=0) at index 1 must be selected instead.
    #[test]
    fn no_apps_top_hit_is_file_or_folder() {
        let results = vec![folder("f2", "fuzzy folder", 3), file("f1", "exact file", 0)];
        let rows = overlay_rows(&results, false);

        // TopHit (exact file) + Folders header + folder = 3 rows.
        assert_eq!(rows.len(), 3);
        assert_eq!(*role_of(&rows[0]), OverlayRowRole::TopHit);
        assert_eq!(rows[0].title, "exact file");
    }

    /// "Show all apps" entry: appended after the last app row, before the
    /// Folders header; suppressed when the flag is off; never in command mode.
    #[test]
    fn show_all_apps_entry_position_and_gating() {
        let results = vec![
            app("a1", "Alpha", 0),
            app("a2", "Alfa", 1),
            folder("f1", "folder", 0),
            file("fi1", "file", 0),
        ];
        let rows = overlay_rows_ext(&results, false, true);

        // TopHit(app) + remaining app + ShowAllApps + Folders hdr + folder
        // + Files hdr + file.
        assert_eq!(rows.len(), 7);
        assert_eq!(*role_of(&rows[0]), OverlayRowRole::TopHit);
        assert_eq!(*role_of(&rows[1]), OverlayRowRole::Item);
        assert_eq!(*role_of(&rows[2]), OverlayRowRole::ShowAllApps);
        assert_eq!(rows[2].title, "Show all apps");
        assert_eq!(rows[2].result_index, None);
        assert_eq!(*role_of(&rows[3]), OverlayRowRole::Header);
        assert_eq!(rows[3].title, "Folders");

        let rows = overlay_rows_ext(&results, false, false);
        assert!(rows.iter().all(|r| r.role != OverlayRowRole::ShowAllApps));

        let rows = overlay_rows_ext(&results, true, true);
        assert!(rows.iter().all(|r| r.role != OverlayRowRole::ShowAllApps));

        // No apps → no entry even with the flag on.
        let no_apps = vec![folder("f1", "folder", 0)];
        let rows = overlay_rows_ext(&no_apps, false, true);
        assert!(rows.iter().all(|r| r.role != OverlayRowRole::ShowAllApps));
    }

    /// (c) Kind order: apps > folders > files > actions > clipboard, same tier.
    #[test]
    fn kind_order_within_same_tier() {
        let results = vec![
            clipboard("c1", "clip", 2),
            action("a1", "act", 2),
            file("f1", "file", 2),
            folder("fo1", "folder", 2),
            app("ap1", "app", 2),
        ];
        let rows = overlay_rows(&results, false);

        // Row 0: TopHit = app
        assert_eq!(rows[0].title, "app");
        assert_eq!(*role_of(&rows[0]), OverlayRowRole::TopHit);

        // Remaining apps (none), then folders header + folder, files header + file,
        // actions header + action, clipboard header + clipboard.
        let titles: Vec<&str> = rows[1..].iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Folders", "folder", "Files", "file", "Actions", "act", "Clipboard", "clip"]
        );
    }

    /// (d) Command mode preserves original score order, no regroup.
    #[test]
    fn command_mode_preserves_order() {
        let results = vec![
            action("a1", "action1", 0),
            action("a2", "action2", 3),
            file("f1", "file1", 1),
        ];
        let rows = overlay_rows(&results, true);

        // All rows are Item, in original order, no headers.
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_eq!(*role_of(row), OverlayRowRole::Item);
        }
        assert_eq!(rows[0].title, "action1");
        assert_eq!(rows[1].title, "action2");
        assert_eq!(rows[2].title, "file1");
    }
}
