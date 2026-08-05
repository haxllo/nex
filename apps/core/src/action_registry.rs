use crate::config::{Config, WebSearchProvider};
use crate::model::{normalize_for_search, SearchItem};
use crate::uninstall_registry::{has_uninstall_intent, search_uninstall_actions};

pub const ACTION_OPEN_LOGS_ID: &str = "__nex_action_open_logs__";
pub const ACTION_REBUILD_INDEX_ID: &str = "__nex_action_rebuild_index__";
pub const ACTION_CLEAR_CLIPBOARD_ID: &str = "__nex_action_clear_clipboard__";
pub const ACTION_OPEN_CONFIG_ID: &str = "__nex_action_open_config__";
pub const ACTION_DIAGNOSTICS_BUNDLE_ID: &str = "__nex_action_diagnostics_bundle__";
pub const ACTION_TRIM_MEMORY_ID: &str = "__nex_action_trim_memory__";
pub const ACTION_CHECK_UPDATES_ID: &str = "__nex_action_check_updates__";
pub const ACTION_WEB_SEARCH_PREFIX: &str = "__nex_action_web_search__:";
pub const ACTION_CREATE_FOLDER_PREFIX: &str = "__nex_action_create_folder__:";
pub const ACTION_CREATE_FILE_PREFIX: &str = "__nex_action_create_file__:";
pub const ACTION_OPEN_URL_PREFIX: &str = "__nex_action_open_url__:";
pub const ACTION_LOCK_ID: &str = "__nex_action_lock__";
pub const ACTION_SLEEP_ID: &str = "__nex_action_sleep__";
pub const ACTION_SHUTDOWN_ID: &str = "__nex_action_shutdown__";
pub const ACTION_RESTART_ID: &str = "__nex_action_restart__";
pub const ACTION_SIGN_OUT_ID: &str = "__nex_action_sign_out__";

#[derive(Debug, Clone, Copy)]
pub struct BuiltInAction {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub keywords: &'static [&'static str],
}

pub fn built_in_actions() -> &'static [BuiltInAction] {
    &[
        BuiltInAction {
            id: ACTION_OPEN_LOGS_ID,
            title: "Open Nex Logs Folder",
            subtitle: "Open logs directory in File Explorer",
            keywords: &["logs", "log", "debug"],
        },
        BuiltInAction {
            id: ACTION_REBUILD_INDEX_ID,
            title: "Rebuild Search Index",
            subtitle: "Force a full refresh of indexed items",
            keywords: &["rebuild", "index", "refresh"],
        },
        BuiltInAction {
            id: ACTION_CLEAR_CLIPBOARD_ID,
            title: "Clear Clipboard History",
            subtitle: "Delete local clipboard history entries",
            keywords: &["clipboard", "clear", "history"],
        },
        BuiltInAction {
            id: ACTION_OPEN_CONFIG_ID,
            title: "Open Nex Config",
            subtitle: "Open config.toml",
            keywords: &["config", "settings", "preferences"],
        },
        BuiltInAction {
            id: ACTION_DIAGNOSTICS_BUNDLE_ID,
            title: "Create Diagnostics Bundle",
            subtitle: "Export logs and sanitized config for support",
            keywords: &["diagnostics", "support", "bundle", "debug"],
        },
        BuiltInAction {
            id: ACTION_CHECK_UPDATES_ID,
            title: "Check for Updates",
            subtitle: "Run the stable Windows updater",
            keywords: &["update", "upgrade", "stable", "install latest"],
        },
        BuiltInAction {
            id: ACTION_TRIM_MEMORY_ID,
            title: "Trim Memory Now",
            subtitle: "Clear overlay icon/query caches and log memory snapshot",
            keywords: &["memory", "trim", "cache", "compact"],
        },
        BuiltInAction {
            id: ACTION_LOCK_ID,
            title: "Lock",
            subtitle: "Lock your workstation",
            keywords: &["lock", "workstation", "secure"],
        },
        BuiltInAction {
            id: ACTION_SLEEP_ID,
            title: "Sleep",
            subtitle: "Put your computer to sleep",
            keywords: &["sleep", "suspend", "hibernate"],
        },
        BuiltInAction {
            id: ACTION_SHUTDOWN_ID,
            title: "Shutdown",
            subtitle: "Power off your computer",
            keywords: &["shutdown", "power", "off"],
        },
        BuiltInAction {
            id: ACTION_RESTART_ID,
            title: "Restart",
            subtitle: "Restart your computer",
            keywords: &["restart", "reboot", "reset"],
        },
        BuiltInAction {
            id: ACTION_SIGN_OUT_ID,
            title: "Sign Out",
            subtitle: "Sign out of your account",
            keywords: &["signout", "logout", "log out", "sign out"],
        },
    ]
}

pub fn search_actions(query: &str, limit: usize) -> Vec<SearchItem> {
    search_actions_with_mode(query, limit, false, &Config::default())
}

pub fn search_actions_with_mode(
    query: &str,
    limit: usize,
    command_mode: bool,
    cfg: &Config,
) -> Vec<SearchItem> {
    if limit == 0 {
        return Vec::new();
    }
    let trimmed_query = query.trim();
    let normalized = normalize_for_search(trimmed_query);
    let mut out = Vec::new();
    let uninstall_intent = cfg.uninstall_actions_enabled && has_uninstall_intent(trimmed_query);

    if command_mode {
        if !uninstall_intent {
            if let Some(web_action) = dynamic_provider_web_search_action(trimmed_query, cfg) {
                out.push(web_action);
                if out.len() >= limit {
                    return out;
                }
            }
            if let Some(open_url_action) = dynamic_provider_open_url_action(trimmed_query, cfg) {
                out.push(open_url_action);
                if out.len() >= limit {
                    return out;
                }
            }
        }

        let remaining = limit.saturating_sub(out.len());
        if remaining > 0 && cfg.uninstall_actions_enabled {
            let uninstall_actions = search_uninstall_actions(trimmed_query, remaining);
            out.extend(uninstall_actions);
            if out.len() >= limit {
                return out;
            }
        }
    }

    for action in built_in_actions() {
        if !normalized.is_empty() {
            let title_match = normalize_for_search(action.title).contains(&normalized);
            let keyword_match = action
                .keywords
                .iter()
                .any(|kw| normalize_for_search(kw).contains(&normalized));
            if !title_match && !keyword_match {
                continue;
            }
        }
        out.push(SearchItem::new(
            action.id,
            "action",
            action.title,
            action.subtitle,
        ));
        if out.len() >= limit {
            break;
        }
    }

    out
}

pub fn provider_web_search_url(cfg: &Config, query: &str) -> Option<String> {
    let encoded = url_encode_component(query.trim());
    let url = match cfg.web_search_provider {
        WebSearchProvider::Duckduckgo => format!("https://duckduckgo.com/?q={encoded}"),
        WebSearchProvider::Google => format!("https://www.google.com/search?q={encoded}"),
        WebSearchProvider::Bing => format!("https://www.bing.com/search?q={encoded}"),
        WebSearchProvider::Brave => format!("https://search.brave.com/search?q={encoded}"),
        WebSearchProvider::Startpage => {
            format!("https://www.startpage.com/sp/search?query={encoded}")
        }
        WebSearchProvider::Ecosia => format!("https://www.ecosia.org/search?q={encoded}"),
        WebSearchProvider::Yahoo => format!("https://search.yahoo.com/search?p={encoded}"),
        WebSearchProvider::Custom => {
            let template = cfg.web_search_custom_template.trim();
            if template.is_empty() || !template.contains("{query}") {
                return None;
            }
            template.replace("{query}", &encoded)
        }
    };
    Some(url)
}

pub(crate) fn dynamic_provider_web_search_action(query: &str, cfg: &Config) -> Option<SearchItem> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = provider_web_search_url(cfg, trimmed)?;
    let id = format!("{ACTION_WEB_SEARCH_PREFIX}{trimmed}");
    Some(SearchItem::new(
        &id,
        "action",
        &format!("Search Web for \"{trimmed}\""),
        &url,
    ))
}

/// Resolve the base directory for create-file / create-folder actions.
/// Uses `cfg.default_create_dir` when non-empty, falls back to
/// `USERPROFILE\Desktop`, then `"."` as last resort.
fn folder_target_base(cfg: &Config) -> std::path::PathBuf {
    if !cfg.default_create_dir.as_os_str().is_empty() {
        cfg.default_create_dir.clone()
    } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
        std::path::PathBuf::from(user_profile).join("Desktop")
    } else {
        std::path::PathBuf::from(".")
    }
}

/// Windows device names can never be created as files/folders.
fn is_windows_reserved_name(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5"
            | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5"
            | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    )
}

/// Detect a trailing-`\` query (user typing a folder name they want to
/// create) and produce a create/open-folder action row.
///
/// Query must end with `\` (or `/`). Everything before it is the folder
/// name (may be nested: `a\b\c`). Target parent = config.default_create_dir,
/// falling back to USERPROFILE\Desktop when unset. If the resolved target
/// already exists on disk, produce an "Open folder" row instead of a create row.
pub(crate) fn dynamic_provider_create_folder_action(
    query: &str,
    cfg: &Config,
) -> Option<SearchItem> {
    let trimmed = query.trim();
    if !cfg.create_actions_enabled {
        return None;
    }
    if trimmed.len() <= 1 {
        return None;
    }
    if !trimmed.ends_with('\\') && !trimmed.ends_with('/') {
        return None;
    }
    let name = trimmed[..trimmed.len() - 1].trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if is_windows_reserved_name(name) {
        return None;
    }
    // Reject illegal Windows path characters in the folder name portion.
    for ch in name.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
            return None;
        }
    }
    let base = folder_target_base(cfg);
    let target = base.join(name);
    let exists = target.exists();

    let id = format!(
        "{ACTION_CREATE_FOLDER_PREFIX}{}",
        if exists { "open:" } else { "create:" }
    );
    let title = if exists {
        format!("Open folder '{name}'")
    } else {
        format!("Create folder '{name}'")
    };
    Some(SearchItem::new(
        &id,
        "action",
        &title,
        &target.to_string_lossy(),
    ))
}

/// Detect a query ending in a whitelisted file extension (e.g. `x.txt`)
/// and produce a create/open-file action row.
///
/// Query must end with `.ext` where ext is in cfg.create_file_extensions
/// (case-insensitive). Name = query before the dot-ext. Target parent =
/// config.default_create_dir, falling back to USERPROFILE\Desktop when
/// unset. If the resolved target already exists, produce an "Open file"
/// row instead of a create row.
pub(crate) fn dynamic_provider_create_file_action(
    query: &str,
    cfg: &crate::config::Config,
) -> Option<SearchItem> {
    let trimmed = query.trim();
    if !cfg.create_actions_enabled {
        return None;
    }
    let dot = trimmed.rfind('.')?;
    if dot == 0 || dot == trimmed.len() - 1 {
        return None;
    }
    // No trailing slashes allowed — folder creation owns those queries.
    if trimmed.ends_with('/') || trimmed.ends_with('\\') {
        return None;
    }
    let name = trimmed[..dot].trim();
    if name.is_empty() {
        return None;
    }
    if is_windows_reserved_name(name) {
        return None;
    }
    for ch in name.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/') {
            return None;
        }
    }
    let ext = trimmed[dot + 1..].to_ascii_lowercase();
    if !cfg
        .create_file_extensions
        .iter()
        .any(|e| e.eq_ignore_ascii_case(&ext))
    {
        return None;
    }

    let base = folder_target_base(cfg);
    let target = base.join(trimmed);
    let exists = target.exists();

    let id = format!(
        "{ACTION_CREATE_FILE_PREFIX}{}",
        if exists { "open:" } else { "create:" }
    );
    let title = if exists {
        format!("Open file '{trimmed}'")
    } else {
        format!("Create file '{trimmed}'")
    };
    Some(SearchItem::new(
        &id,
        "action",
        &title,
        &target.to_string_lossy(),
    ))
}

/// Detect a bare domain or explicit URL (youtube.com, github.com/haxllo/nex,
/// https://localhost:8080) and produce an "Open <url>" action row.
///
/// Accepted when either:
///  - the query starts with http:// or https:// (any host, incl. localhost), or
///  - the query is a single whitespace-free token whose last dot-segment is a
///    known TLD from cfg.url_tlds (bare domain → auto-prefix https://).
/// Queries ending in a whitelisted create-file extension never match ("site.com
/// .html" → .html is a create-file extension; its TLD segment isn't in url_tlds
/// anyway).
pub(crate) fn dynamic_provider_open_url_action(
    query: &str,
    cfg: &crate::config::Config,
) -> Option<SearchItem> {
    if !cfg.open_url_in_default_browser {
        return None;
    }
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let url = if lower.starts_with("http://") || lower.starts_with("https://") {
        trimmed.to_string()
    } else if looks_like_bare_domain(&lower, &cfg.url_tlds) {
        format!("https://{trimmed}")
    } else {
        return None;
    };
    let id = format!("{ACTION_OPEN_URL_PREFIX}{trimmed}");
    Some(SearchItem::new(
        &id,
        "action",
        &format!("Open {trimmed}"),
        &url,
    ))
}

fn looks_like_bare_domain(lower_query: &str, tlds: &[String]) -> bool {
    // Single whitespace-free token.
    if lower_query.contains(char::is_whitespace) {
        return false;
    }
    // Explicit scheme already handled by caller — reject any other scheme.
    if lower_query.contains("://") {
        return false;
    }
    // Must not be an IP literal or a localhost:port form.
    if lower_query.starts_with("localhost") {
        return false;
    }
    let Some((name_part, tld)) = lower_query.rsplit_once('.') else {
        return false; // no dot at all → not a domain
    };
    if name_part.is_empty() {
        return false;
    }
    // TLD must be >= 2 chars, all lowercase ASCII.
    if tld.is_empty() || tld.len() < 2 || !tld.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    tlds.iter().any(|known| known.eq_ignore_ascii_case(tld))
}

fn url_encode_component(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        search_actions, search_actions_with_mode, ACTION_CHECK_UPDATES_ID, ACTION_WEB_SEARCH_PREFIX,
    };
    use crate::config::{Config, WebSearchProvider};

    #[test]
    fn filters_actions_by_query() {
        let actions = search_actions("diag", 10);
        assert!(actions
            .iter()
            .any(|action| action.id == "__nex_action_diagnostics_bundle__"));
    }

    #[test]
    fn command_mode_includes_web_search_action() {
        let cfg = Config::default();
        let actions = search_actions_with_mode("rust icons", 10, true, &cfg);
        assert!(actions
            .iter()
            .any(|action| action.id.starts_with(ACTION_WEB_SEARCH_PREFIX)));
    }

    #[test]
    fn non_command_mode_omits_web_search_action() {
        let cfg = Config::default();
        let actions = search_actions_with_mode("rust icons", 10, false, &cfg);
        assert!(!actions
            .iter()
            .any(|action| action.id.starts_with(ACTION_WEB_SEARCH_PREFIX)));
    }

    #[test]
    fn command_mode_respects_configured_provider() {
        let mut cfg = Config::default();
        cfg.web_search_provider = WebSearchProvider::Google;

        let actions = search_actions_with_mode("rust icons", 10, true, &cfg);
        let provider = actions
            .iter()
            .find(|action| action.id.starts_with(ACTION_WEB_SEARCH_PREFIX))
            .expect("provider web action should exist");
        assert!(provider.path.contains("google.com/search?q="));
    }

    #[test]
    fn uninstall_intent_hides_web_action() {
        let cfg = Config::default();
        let actions = search_actions_with_mode("u notepad", 20, true, &cfg);
        assert!(!actions
            .iter()
            .any(|action| action.id.starts_with(ACTION_WEB_SEARCH_PREFIX)));
    }

    #[test]
    fn built_in_actions_include_check_for_updates() {
        let cfg = Config::default();
        let actions = search_actions_with_mode("update", 10, true, &cfg);
        assert!(actions
            .iter()
            .any(|action| action.id == ACTION_CHECK_UPDATES_ID));
    }
}
