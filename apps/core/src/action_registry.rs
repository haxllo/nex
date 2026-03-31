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

fn dynamic_provider_web_search_action(query: &str, cfg: &Config) -> Option<SearchItem> {
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
        search_actions, search_actions_with_mode, ACTION_CHECK_UPDATES_ID,
        ACTION_WEB_SEARCH_PREFIX,
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
