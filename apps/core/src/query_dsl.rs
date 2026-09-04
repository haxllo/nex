use crate::config::SearchMode;
use crate::model::normalize_for_search;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFilterWindow {
    Today,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub raw: String,
    pub free_text: String,
    pub mode_override: Option<SearchMode>,
    pub kind_filter: Option<String>,
    pub extension_filter: Option<String>,
    pub include_groups: Vec<Vec<String>>,
    pub exclude_terms: Vec<String>,
    pub modified_within: Option<TimeFilterWindow>,
    pub created_within: Option<TimeFilterWindow>,
    pub command_mode: bool,
}

impl ParsedQuery {
    pub fn parse(query: &str, dsl_enabled: bool) -> Self {
        let raw = query.trim().to_string();
        if raw.is_empty() {
            return Self {
                raw,
                free_text: String::new(),
                mode_override: None,
                kind_filter: None,
                extension_filter: None,
                include_groups: Vec::new(),
                exclude_terms: Vec::new(),
                modified_within: None,
                created_within: None,
                command_mode: false,
            };
        }

        if !dsl_enabled {
            return Self {
                free_text: raw.clone(),
                raw,
                mode_override: None,
                kind_filter: None,
                extension_filter: None,
                include_groups: Vec::new(),
                exclude_terms: Vec::new(),
                modified_within: None,
                created_within: None,
                command_mode: false,
            };
        }

        let mut command_mode = false;
        let mut working = raw.clone();
        if let Some(rest) = working.strip_prefix('>') {
            command_mode = true;
            working = rest.trim_start().to_string();
        } else if let Some(rest) = working.strip_prefix('@') {
            // `@` is the command-mode prefix, but a leading mode token
            // (`@apps`, `@files`, `@clipboard`, …) keeps its existing
            // meaning — only non-mode `@…` queries enter command mode.
            let first = rest.split_whitespace().next().unwrap_or("");
            if SearchMode::parse(first).is_none() {
                command_mode = true;
                working = rest.trim_start().to_string();
            }
        }

        let tokens = tokenize(&working);
        let mut mode_override = if command_mode {
            Some(SearchMode::Actions)
        } else {
            None
        };
        let mut kind_filter: Option<String> = None;
        let mut extension_filter: Option<String> = None;
        let mut include_groups: Vec<Vec<String>> = vec![Vec::new()];
        let mut exclude_terms = Vec::new();
        let mut free_terms = Vec::new();
        let mut expect_not = false;
        let mut modified_within = None;
        let mut created_within = None;

        for token in tokens {
            let token_trimmed = token.trim();
            if token_trimmed.is_empty() {
                continue;
            }

            let upper = token_trimmed.to_ascii_uppercase();
            if upper == "AND" {
                continue;
            }
            if upper == "OR" {
                include_groups.push(Vec::new());
                expect_not = false;
                continue;
            }
            if upper == "NOT" {
                expect_not = true;
                continue;
            }

            if let Some(mode) = parse_mode_token(token_trimmed) {
                mode_override = Some(mode);
                expect_not = false;
                continue;
            }

            if let Some(value) = parse_prefixed(token_trimmed, "mode:") {
                if let Some(mode) = SearchMode::parse(value) {
                    mode_override = Some(mode);
                }
                expect_not = false;
                continue;
            }
            if let Some(value) = parse_prefixed(token_trimmed, "kind:") {
                let normalized = value.trim().to_ascii_lowercase();
                if !normalized.is_empty() {
                    kind_filter = Some(normalized);
                }
                expect_not = false;
                continue;
            }
            if let Some(value) = parse_prefixed(token_trimmed, "ext:")
                .or_else(|| parse_prefixed(token_trimmed, "extension:"))
            {
                let normalized = normalize_extension_filter(value);
                if !normalized.is_empty() {
                    extension_filter = Some(normalized);
                }
                expect_not = false;
                continue;
            }
            if let Some(value) = parse_prefixed(token_trimmed, "modified:") {
                modified_within = parse_time_filter(value);
                expect_not = false;
                continue;
            }
            if let Some(value) = parse_prefixed(token_trimmed, "created:") {
                created_within = parse_time_filter(value);
                expect_not = false;
                continue;
            }

            let is_negative_literal = token_trimmed.starts_with('-') && token_trimmed.len() > 1;
            let target = if is_negative_literal {
                &token_trimmed[1..]
            } else {
                token_trimmed
            };
            let normalized = normalize_for_search(target);
            if normalized.is_empty() {
                expect_not = false;
                continue;
            }

            if expect_not || is_negative_literal {
                exclude_terms.push(normalized.clone());
            } else {
                if include_groups.is_empty() {
                    include_groups.push(Vec::new());
                }
                if let Some(group) = include_groups.last_mut() {
                    group.push(normalized.clone());
                }
                free_terms.push(target.to_string());
            }
            expect_not = false;
        }

        include_groups.retain(|group| !group.is_empty());
        let free_text = free_terms.join(" ");

        Self {
            raw,
            free_text,
            mode_override,
            kind_filter,
            extension_filter,
            include_groups,
            exclude_terms,
            modified_within,
            created_within,
            command_mode,
        }
    }
}

fn normalize_extension_filter(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('.')
        .chars()
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn parse_prefixed<'a>(token: &'a str, prefix: &str) -> Option<&'a str> {
    token
        .strip_prefix(prefix)
        .or_else(|| token.strip_prefix(&prefix.to_ascii_uppercase()))
}

fn parse_mode_token(token: &str) -> Option<SearchMode> {
    let token = token.trim();
    if !token.starts_with('@') {
        return None;
    }
    SearchMode::parse(token.trim_start_matches('@'))
}

fn parse_time_filter(value: &str) -> Option<TimeFilterWindow> {
    match value.trim().to_ascii_lowercase().as_str() {
        "today" => Some(TimeFilterWindow::Today),
        "week" | "last_week" => Some(TimeFilterWindow::Week),
        "month" | "last_month" => Some(TimeFilterWindow::Month),
        _ => None,
    }
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }

        if ch.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }

        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::{ParsedQuery, TimeFilterWindow};
    use crate::config::SearchMode;

    #[test]
    fn parses_mode_kind_and_filters() {
        let parsed = ParsedQuery::parse(
            r#"@apps kind:file ext:md report OR notes NOT draft -temp modified:week"#,
            true,
        );
        assert_eq!(parsed.mode_override, Some(SearchMode::Apps));
        assert_eq!(parsed.kind_filter.as_deref(), Some("file"));
        assert_eq!(parsed.extension_filter.as_deref(), Some("md"));
        assert_eq!(parsed.modified_within, Some(TimeFilterWindow::Week));
        assert_eq!(parsed.include_groups.len(), 2);
        assert!(parsed.exclude_terms.contains(&"draft".to_string()));
        assert!(parsed.exclude_terms.contains(&"temp".to_string()));
    }

    #[test]
    fn parses_command_mode_prefix() {
        let parsed = ParsedQuery::parse(">logs", true);
        assert!(parsed.command_mode);
        assert_eq!(parsed.mode_override, Some(SearchMode::Actions));
        assert_eq!(parsed.free_text, "logs");
    }

    #[test]
    fn at_symbol_enters_command_mode() {
        let parsed = ParsedQuery::parse("@clip", true);
        assert!(parsed.command_mode);
        assert_eq!(parsed.mode_override, Some(SearchMode::Actions));
        assert_eq!(parsed.free_text, "clip");
    }

    #[test]
    fn at_mode_token_still_wins_over_command_mode() {
        let parsed = ParsedQuery::parse("@apps", true);
        assert!(!parsed.command_mode);
        assert_eq!(parsed.mode_override, Some(SearchMode::Apps));

        let parsed = ParsedQuery::parse("@clipboard history", true);
        assert!(!parsed.command_mode);
        assert_eq!(parsed.mode_override, Some(SearchMode::Clipboard));
        assert_eq!(parsed.free_text, "history");
    }

    #[test]
    fn disables_dsl_when_disabled() {
        let parsed = ParsedQuery::parse("kind:file notes", false);
        assert_eq!(parsed.free_text, "kind:file notes");
        assert!(parsed.mode_override.is_none());
        assert!(parsed.include_groups.is_empty());
    }
}
