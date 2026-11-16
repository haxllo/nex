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

/// The What's New document for this binary, baked at compile time by
/// `build.rs` from `docs/whats-new.md`.
///
/// This is a separate document from the GitHub release notes
/// (`docs/releases/v<version>-notes.md`). The release page carries the commit
/// log and the downloads; this one is written for the person using Nex and is
/// edited by hand before each release. `build.rs` strips the format
/// instructions the document carries in HTML comments.
///
/// This used to be fetched from the GitHub release page at runtime, which
/// meant the view rendered a body generated from commit messages — a changelog
/// dump that got worse every release and could not be curated.
const BAKED_NOTES: &str = include_str!(concat!(env!("OUT_DIR"), "/whats_new.md"));

/// The What's New text to show for this build.
///
/// The document explains its own format in HTML comments so whoever edits it
/// knows the rules; the in-app renderer understands headings, paragraphs and
/// bullets but not HTML, so those comments are removed here rather than
/// appearing as literal text.
pub fn notes_markdown() -> String {
    strip_html_comments(BAKED_NOTES)
}

/// Remove `<!-- ... -->` blocks, including multi-line ones.
///
/// An unterminated comment is dropped to the end of the document rather than
/// left in: the alternative is leaking editing instructions to a user.
fn strip_html_comments(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let continued_comment = inside;
        let mut kept = String::new();
        let mut rest = line;
        loop {
            if inside {
                match rest.find("-->") {
                    Some(end) => {
                        rest = &rest[end + 3..];
                        inside = false;
                    }
                    None => {
                        // The comment continues onto the next line; nothing
                        // of this one survives. Break rather than assign an
                        // empty `rest`, which would loop forever with no
                        // `-->` left to find.
                        break;
                    }
                }
            } else {
                match rest.find("<!--") {
                    Some(start) => {
                        kept.push_str(&rest[..start]);
                        rest = &rest[start + 4..];
                        inside = true;
                    }
                    None => {
                        kept.push_str(rest);
                        break;
                    }
                }
            }
        }
        let had_comment = continued_comment || line.contains("<!--");
        // A line that held nothing but comment disappears completely. Keeping
        // it as a blank line would split a bullet list the comment sat inside
        // into two lists, which changes how the view renders the document.
        if had_comment && kept.trim().is_empty() {
            continue;
        }
        out.push(kept.trim_end().to_string());
    }

    // Collapse the blank runs left where comment blocks used to be.
    let mut cleaned: Vec<&str> = Vec::new();
    let mut blank_run = 0;
    for line in &out {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        cleaned.push(line);
    }
    cleaned.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release must not ship an empty What's New view. `build.rs` warns
    /// rather than failing when the document is missing, which keeps ordinary
    /// builds working but would let a release publish the tips fallback; this
    /// makes that case visible in the test run instead.
    #[test]
    fn baked_notes_are_present_and_are_prose() {
        let notes = notes_markdown();
        assert!(
            !notes.trim().is_empty(),
            "no What's New baked for {}: write docs/whats-new.md",
            env!("CARGO_PKG_VERSION"),
        );
        // Instructions for whoever edits the document live in HTML comments.
        // The in-app renderer does not understand HTML, so anything left would
        // be shown to the user as literal text.
        assert!(
            !notes.contains("<!--") && !notes.contains("-->"),
            "a format comment survived into the baked document"
        );
        // The release page owns the commit log; the app must not show it.
        assert!(
            !notes.contains("## Commit Log"),
            "the baked document carries a commit log; it belongs in docs/releases"
        );
    }

    #[test]
    fn comments_are_removed_from_the_shown_text() {
        assert_eq!(strip_html_comments("before\n<!-- note -->\nafter"), "before\nafter");
    }

    #[test]
    fn a_multi_line_comment_is_removed() {
        let text = "keep\n<!--\n  several\n  lines\n-->\nalso keep";
        assert_eq!(strip_html_comments(text), "keep\nalso keep");
    }

    /// An unterminated comment must not reach the user.
    #[test]
    fn an_unclosed_comment_is_dropped_to_the_end() {
        assert_eq!(strip_html_comments("shown\n<!-- never closed\nsecret"), "shown");
    }

    /// A comment between two bullets must not become a blank line: a blank line
    /// ends the list in the view's renderer, turning one list into two.
    #[test]
    fn a_comment_inside_a_list_does_not_split_it() {
        let text = "- first\n<!-- why -->\n- second";
        assert_eq!(strip_html_comments(text), "- first\n- second");
    }

    /// Comments between blocks leave exactly one blank line, not a run.
    #[test]
    fn a_comment_between_blocks_leaves_one_blank_line() {
        let text = "## Heading\n\n<!--\n  note\n-->\n\nText.";
        assert_eq!(strip_html_comments(text), "## Heading\n\nText.");
    }

    #[test]
    fn an_inline_comment_keeps_the_text_around_it() {
        assert_eq!(strip_html_comments("left <!-- x --> right"), "left  right");
    }

    /// A `-->` written inside a comment ends it early, and everything after
    /// it is shown. This bit the document itself: the instructions described
    /// the release marker by quoting its comment syntax, which closed the
    /// block and leaked instruction text into the view.
    #[test]
    fn text_after_a_stray_terminator_is_still_only_a_warning_not_silence() {
        let text = "<!--\n  see <!-- release: 1 --> for the marker\n  more rules\n-->\n## Shown";
        let shown = strip_html_comments(text);
        // The early close leaves the remaining rules visible, which is exactly
        // why the document must not quote comment syntax at itself.
        assert!(shown.contains("more rules"), "{shown}");
        assert!(shown.contains("## Shown"));
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
