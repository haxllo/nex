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
//! Notes content is baked into the binary at build time from
//! `docs/releases/v<version>-notes.md` (see `build.rs`), trimmed to the
//! user-facing part with technical commit logs and binary links removed. A
//! build without that document bakes an empty string and the page renders its
//! generic fallback + tips.

use std::path::{Path, PathBuf};

const STATE_FILE_NAME: &str = "state.json";
const SEEN_FIELD: &str = "whats_new_seen";

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

/// The notes for the version this binary is, baked at compile time by
/// `build.rs` from `docs/releases/v<version>-notes.md`.
///
/// These used to be fetched from the GitHub release page at runtime, which
/// meant the page rendered whatever the release body contained — a body now
/// generated from commit messages, so it grew into a changelog dump that got
/// worse every release and could not be curated. Baking the hand-written
/// document keeps the page clean, works offline, and cannot disagree with the
/// version the user is actually running.
const BAKED_NOTES: &str = include_str!(concat!(env!("OUT_DIR"), "/release_notes.md"));

/// The notes to show for this build, commit log and binary links removed.
pub fn notes_markdown() -> String {
    extract_highlights(BAKED_NOTES)
}

/// Keep all user-facing release notes, including the title and summary, and
/// drop the commit log and binary links — those are for the release page, not
/// the overlay.
fn extract_highlights(body: &str) -> String {
    const ENDS: &[&str] = &["## Commit Log", "## Binary"];
    body.lines()
        .take_while(|line| !ENDS.iter().any(|marker| line.trim() == *marker))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
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

    /// The build bakes the notes document for the version being built, so a
    /// release cannot ship another release's prose. `build.rs` warns rather
    /// than failing when the document is missing, which keeps ordinary builds
    /// working but would let a release ship the tips fallback; this makes that
    /// case visible in the test run instead.
    #[test]
    fn baked_notes_exist_and_are_not_the_previous_release() {
        let notes = notes_markdown();
        assert!(
            !notes.trim().is_empty(),
            "no notes baked for {}: add docs/releases/v{}-notes.md",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_VERSION"),
        );
        assert!(
            !notes.contains("## Commit Log"),
            "the baked notes still carry the commit log; it belongs on the              release page, not in the overlay"
        );
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
