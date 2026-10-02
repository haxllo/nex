//! Post-update "What's New" state.
//!
//! The silent updater restarts Nex in place, so the new version boots with
//! no memory of having been updated. This module persists the last version
//! the user has seen the notes for (`state.json` next to `config.toml`)
//! and reports a pending version exactly once per install:
//!
//! - fresh install (no app-data state at all) → records the version, no view
//! - pre-feature install (config/index exist, no `state.json`) → pending
//! - `seen != current` → the app was updated → pending
//! - `seen == current` → already viewed/dismissed → nothing
//!
//! Notes content comes from the GitHub release page for the new version
//! (same source as the update check), trimmed to the user-facing
//! "What changed" section. Offline or unpublished notes degrade to an
//! empty string and the page renders a generic fallback + tips.

use std::path::{Path, PathBuf};

const STATE_FILE_NAME: &str = "state.json";
const SEEN_FIELD: &str = "whats_new_seen";
const RELEASES_REPO: &str = "haxllo/nex";
const MAX_NOTES_CHARS: usize = 6000;

fn state_path() -> PathBuf {
    crate::config::stable_app_data_dir().join(STATE_FILE_NAME)
}

/// Pre-feature install footprint: config or index from an older Nex that
/// never wrote `state.json`.
fn legacy_state_present(dir: &Path) -> bool {
    ["config.toml", "config.json", "index.sqlite3"]
        .iter()
        .any(|file_name| dir.join(file_name).exists())
}

fn read_seen(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(STATE_FILE_NAME)).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get(SEEN_FIELD)?
        .as_str()
        .map(str::to_string)
}

/// Version the user hasn't seen notes for, if any. Records fresh installs
/// silently so they never get a view for the version they started on.
pub fn check_pending() -> Option<String> {
    let dir = crate::config::stable_app_data_dir();
    pending_for_dir(&dir, env!("CARGO_PKG_VERSION"))
}

fn pending_for_dir(dir: &Path, current: &str) -> Option<String> {
    match read_seen(dir) {
        Some(seen) if seen == current => None,
        Some(_) => Some(current.to_string()),
        None if legacy_state_present(dir) => Some(current.to_string()),
        None => {
            mark_seen_in(dir, current);
            None
        }
    }
}

/// Record `version` as seen. Best-effort: a failed write just means the
/// view may show again next launch, never a crash or a blocked update.
pub fn mark_seen(version: &str) {
    mark_seen_in(&crate::config::stable_app_data_dir(), version);
}

fn mark_seen_in(dir: &Path, version: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let value = serde_json::json!({ SEEN_FIELD: version });
    let text = value.to_string();
    let path = dir.join(STATE_FILE_NAME);
    // Write-then-rename so a killed process never leaves half a file.
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Fetch the release notes for `version` and trim them to the user-facing
/// section. Returns the markdown (possibly empty when offline or when the
/// release has no parseable section — the page falls back to tips).
pub fn fetch_notes_markdown(version: &str) -> String {
    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases/tags/v{version}");
    let text = match ureq::get(&url)
        .set("User-Agent", "Nex-WhatsNew")
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(response) => match response.into_string() {
            Ok(text) => text,
            Err(_) => return String::new(),
        },
        Err(_) => return String::new(),
    };
    let body: serde_json::Value = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(_) => return String::new(),
    };
    let markdown = body
        .get("body")
        .and_then(|body| body.as_str())
        .unwrap_or_default();
    truncate_chars(&extract_highlights(markdown), MAX_NOTES_CHARS)
}

/// Keep the "What changed" section (headings + bullets), drop the commit
/// log and binary links — those are for the release page, not the overlay.
fn extract_highlights(body: &str) -> String {
    const START: &str = "## What changed";
    const ENDS: &[&str] = &["## Commit Log", "## Binary"];
    let Some(start) = body.find(START) else {
        return body.trim().to_string();
    };
    let rest = &body[start + START.len()..];
    let mut end = rest.len();
    for marker in ENDS {
        if let Some(pos) = rest.find(marker) {
            end = end.min(pos);
        }
    }
    rest[..end].trim().to_string()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_keeps_what_changed_drops_log_and_binaries() {
        let body = "## Title\n\nIntro.\n\n## What changed\n\n- **Fast**: now faster\n\n### Bug Fixes\n\n- **Stuck**: fixed\n\n## Commit Log\n\n- [`abc`](url) msg\n\n## Binary\n\n- [Download](url)";
        let out = extract_highlights(body);
        assert!(out.contains("- **Fast**: now faster"));
        assert!(out.contains("### Bug Fixes"));
        assert!(!out.contains("Commit Log"));
        assert!(!out.contains("Download"));
    }

    #[test]
    fn extract_falls_back_to_whole_body_without_section() {
        assert_eq!(extract_highlights("just text").trim(), "just text");
    }

    #[test]
    fn pending_seen_current_means_nothing_pending() {
        let dir = std::env::temp_dir().join(format!("nex-wn-seen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        mark_seen_in(&dir, "9.9.9");
        assert_eq!(pending_for_dir(&dir, "9.9.9"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_older_seen_means_update_pending() {
        let dir = std::env::temp_dir().join(format!("nex-wn-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        mark_seen_in(&dir, "9.9.8");
        assert_eq!(
            pending_for_dir(&dir, "9.9.9"),
            Some("9.9.9".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_empty_dir_records_fresh_install_silently() {
        let dir = std::env::temp_dir().join(format!("nex-wn-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(pending_for_dir(&dir, "9.9.9"), None);
        // Second launch now counts as seen.
        assert_eq!(pending_for_dir(&dir, "9.9.9"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_legacy_footprint_means_update_pending() {
        let dir = std::env::temp_dir().join(format!("nex-wn-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "hotkey = 'x'").unwrap();
        assert_eq!(
            pending_for_dir(&dir, "9.9.9"),
            Some("9.9.9".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
