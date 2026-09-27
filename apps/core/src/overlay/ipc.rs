//! Strictly-typed WebView IPC envelope.
//!
//! Both overlay windows (main launcher + settings) speak JSON over the
//! wry IPC bridge. This module defines the explicit message enums and
//! payload schemas both handlers deserialize into. Deserialization is
//! strict: malformed JSON, unknown message types, and unknown fields are
//! all rejected with a logged error instead of being silently coerced.
//!
//! Size caps live on the raw string (`MAX_IPC_BYTES`) and on the
//! variable-length string fields (`MAX_QUERY_CHARS`, `MAX_TITLE_CHARS`,
//! `MAX_PATH_CHARS`, `MAX_URL_CHARS`). The caps are intentionally
//! small multiples of the largest legitimate UI value: a query is a
//! single input line, titles/paths/URLs originate from rendered rows.

#![cfg(target_os = "windows")]

use serde::Deserialize;

/// Hard cap on the raw IPC body. The largest legitimate message is a
/// settings save (~1 KB of cfg JSON); 64 KiB leaves wide headroom while
/// bounding allocator work per message.
pub(crate) const MAX_IPC_BYTES: usize = 64 * 1024;
/// Longest accepted search query (characters). The input box holds one line.
pub(crate) const MAX_QUERY_CHARS: usize = 1024;
/// Longest accepted title for pin/unpin/context actions (characters).
pub(crate) const MAX_TITLE_CHARS: usize = 1024;
/// Longest accepted filesystem path for quick-launch/context actions (characters).
pub(crate) const MAX_PATH_CHARS: usize = 32_768;
/// Longest accepted bookmark URL (characters).
pub(crate) const MAX_URL_CHARS: usize = 8192;

/// Why an IPC message was rejected. Returned to both call sites so they
/// can log a one-line reason; the page itself gets no error channel
/// (the IPC bridge is fire-and-forget by design).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IpcReject {
    /// Body exceeds `MAX_IPC_BYTES`.
    Oversize { bytes: usize },
    /// Body is not valid JSON.
    Malformed(String),
    /// `"t"` tag names no known variant.
    UnknownType(String),
    /// A known variant whose payload fails schema validation
    /// (unknown field, wrong type, or over-long string).
    BadPayload(String),
}

impl std::fmt::Display for IpcReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversize { bytes } => write!(f, "oversize ipc body ({bytes} bytes)"),
            Self::Malformed(e) => write!(f, "malformed ipc json: {e}"),
            Self::UnknownType(t) => write!(f, "unknown ipc message type: {t:?}"),
            Self::BadPayload(e) => write!(f, "bad ipc payload: {e}"),
        }
    }
}

fn check_len(value: &str, max_chars: usize, field: &str) -> Result<(), String> {
    if value.chars().count() > max_chars {
        return Err(format!("{field} exceeds {max_chars} chars"));
    }
    Ok(())
}

/// Empty payload for unit-like messages (`ready`, `escape`, ...). Rejects
/// any extra top-level field so `{"t":"ready","zzz":1}` is a `BadPayload`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoPayload {}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryPayload {
    #[serde(default)]
    pub v: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexPayload {
    #[serde(default)]
    pub v: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextPayload {
    pub v: String,
}

/// Resize payload: `{t:"resize", v:{v:h, immediate:bool}}` (current) or
/// `{t:"resize", v:h}` (legacy number-only).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResizeObject {
    pub v: f64,
    #[serde(default)]
    pub immediate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub(crate) enum ResizeHeight {
    Object(ResizeObject),
    Bare(f64),
}

impl ResizeHeight {
    pub(crate) fn height_and_immediate(self) -> (f64, bool) {
        match self {
            Self::Object(o) => (o.v, o.immediate),
            Self::Bare(h) => (h, false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResizePayload {
    pub v: ResizeHeight,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BookmarkInner {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub remove: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BookmarkPayload {
    pub v: BookmarkInner,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextInner {
    pub action: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextPayload {
    pub v: ContextInner,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavePayload {
    pub cfg: serde_json::Value,
}

/// Main overlay IPC envelope. `#[serde(tag = "t")]` routes on the type
/// tag; unknown tags fail the parse (surfaced as `UnknownType`). Payloads
/// are newtype struct variants so `deny_unknown_fields` rejects extra
/// fields on every message, including unit-like ones via `NoPayload`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t")]
pub(crate) enum OverlayMessage {
    #[serde(rename = "ready")]
    Ready(NoPayload),
    #[serde(rename = "query")]
    Query(QueryPayload),
    #[serde(rename = "submit")]
    Submit(IndexPayload),
    #[serde(rename = "select")]
    Select(IndexPayload),
    #[serde(rename = "escape")]
    Escape(NoPayload),
    #[serde(rename = "resize")]
    Resize(ResizePayload),
    #[serde(rename = "painted")]
    Painted(NoPayload),
    #[serde(rename = "pin")]
    Pin(TextPayload),
    #[serde(rename = "unpin")]
    Unpin(TextPayload),
    #[serde(rename = "bookmark")]
    Bookmark(BookmarkPayload),
    #[serde(rename = "addToQuickLaunch")]
    AddToQuickLaunch(TextPayload),
    #[serde(rename = "powerAction")]
    PowerAction(TextPayload),
    #[serde(rename = "contextAction")]
    ContextAction(ContextPayload),
    #[serde(rename = "settings")]
    Settings(NoPayload),
    #[serde(rename = "checkUpdates")]
    CheckUpdates(NoPayload),
    #[serde(rename = "dragStart")]
    DragStart(NoPayload),
}

/// Settings-window IPC envelope. Deliberately separate from
/// [`OverlayMessage`] so a settings-only message can never be delivered
/// to the launcher handler and vice versa.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t")]
pub(crate) enum SettingsMessage {
    #[serde(rename = "save")]
    Save(SavePayload),
    #[serde(rename = "ready")]
    Ready(NoPayload),
    #[serde(rename = "recordHotkey")]
    RecordHotkey(NoPayload),
    #[serde(rename = "cancelRecord")]
    CancelRecord(NoPayload),
    #[serde(rename = "minimize")]
    Minimize(NoPayload),
    #[serde(rename = "close")]
    Close(NoPayload),
}

/// Extract the `"t"` tag for error reporting when full parsing fails.
fn tag_of(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("t").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// Strictly parse one main-overlay IPC body.
pub(crate) fn parse_overlay(body: &str) -> Result<OverlayMessage, IpcReject> {
    if body.len() > MAX_IPC_BYTES {
        return Err(IpcReject::Oversize { bytes: body.len() });
    }
    let tag = tag_of(body);
    const KNOWN: &[&str] = &[
        "ready",
        "query",
        "submit",
        "select",
        "escape",
        "resize",
        "painted",
        "pin",
        "unpin",
        "bookmark",
        "addToQuickLaunch",
        "powerAction",
        "contextAction",
        "settings",
        "checkUpdates",
        "dragStart",
    ];
    if !tag.is_empty() && !KNOWN.contains(&tag.as_str()) {
        return Err(IpcReject::UnknownType(tag));
    }
    let msg: OverlayMessage = serde_json::from_str(body).map_err(|e| {
        let text = e.to_string();
        if tag.is_empty() {
            IpcReject::Malformed(text)
        } else if text.contains("unknown variant") {
            IpcReject::UnknownType(tag)
        } else {
            IpcReject::BadPayload(text)
        }
    })?;
    match &msg {
        OverlayMessage::Query(p) => {
            check_len(&p.v, MAX_QUERY_CHARS, "query").map_err(IpcReject::BadPayload)?
        }
        OverlayMessage::Pin(p) | OverlayMessage::Unpin(p) => {
            check_len(&p.v, MAX_TITLE_CHARS, "pin target").map_err(IpcReject::BadPayload)?;
        }
        OverlayMessage::AddToQuickLaunch(p) => {
            check_len(&p.v, MAX_PATH_CHARS, "quick-launch path").map_err(IpcReject::BadPayload)?;
        }
        OverlayMessage::Bookmark(p) => {
            check_len(&p.v.title, MAX_TITLE_CHARS, "bookmark title")
                .map_err(IpcReject::BadPayload)?;
            check_len(&p.v.url, MAX_URL_CHARS, "bookmark url").map_err(IpcReject::BadPayload)?;
        }
        OverlayMessage::PowerAction(p) => {
            check_len(&p.v, 32, "power action").map_err(IpcReject::BadPayload)?;
        }
        OverlayMessage::ContextAction(p) => {
            check_len(&p.v.action, 64, "context action").map_err(IpcReject::BadPayload)?;
            check_len(&p.v.title, MAX_TITLE_CHARS, "context title")
                .map_err(IpcReject::BadPayload)?;
            check_len(&p.v.path, MAX_PATH_CHARS, "context path").map_err(IpcReject::BadPayload)?;
        }
        _ => {}
    }
    Ok(msg)
}

/// Strictly parse one settings-window IPC body.
pub(crate) fn parse_settings(body: &str) -> Result<SettingsMessage, IpcReject> {
    if body.len() > MAX_IPC_BYTES {
        return Err(IpcReject::Oversize { bytes: body.len() });
    }
    let tag = tag_of(body);
    const KNOWN: &[&str] = &[
        "save",
        "ready",
        "recordHotkey",
        "cancelRecord",
        "minimize",
        "close",
    ];
    if !tag.is_empty() && !KNOWN.contains(&tag.as_str()) {
        return Err(IpcReject::UnknownType(tag));
    }
    serde_json::from_str(body).map_err(|e| {
        let text = e.to_string();
        if tag.is_empty() {
            IpcReject::Malformed(text)
        } else if text.contains("unknown variant") {
            IpcReject::UnknownType(tag)
        } else {
            IpcReject::BadPayload(text)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            parse_overlay("{not json"),
            Err(IpcReject::Malformed(_))
        ));
        assert!(matches!(
            parse_settings("{not json"),
            Err(IpcReject::Malformed(_))
        ));
    }

    #[test]
    fn rejects_unknown_message_types() {
        assert_eq!(
            parse_overlay(r#"{"t":"bogus"}"#),
            Err(IpcReject::UnknownType("bogus".into()))
        );
        assert_eq!(
            parse_settings(r#"{"t":"query","v":"x"}"#),
            Err(IpcReject::UnknownType("query".into()))
        );
    }

    #[test]
    fn rejects_unknown_fields_everywhere() {
        assert!(matches!(
            parse_overlay(r#"{"t":"query","v":"x","zzz":1}"#),
            Err(IpcReject::BadPayload(_))
        ));
        assert!(matches!(
            parse_overlay(r#"{"t":"ready","zzz":1}"#),
            Err(IpcReject::BadPayload(_))
        ));
        assert!(matches!(
            parse_overlay(
                r#"{"t":"bookmark","v":{"title":"a","url":"https://x","remove":false,"zzz":1}}"#
            ),
            Err(IpcReject::BadPayload(_))
        ));
        assert!(matches!(
            parse_settings(r#"{"t":"close","zzz":1}"#),
            Err(IpcReject::BadPayload(_))
        ));
    }

    #[test]
    fn rejects_oversize_bodies() {
        let big = format!(r#"{{"t":"query","v":"{}"}}"#, "x".repeat(MAX_IPC_BYTES));
        assert!(matches!(
            parse_overlay(&big),
            Err(IpcReject::Oversize { .. })
        ));
    }

    #[test]
    fn rejects_overlong_query_strings() {
        let big = format!(
            r#"{{"t":"query","v":"{}"}}"#,
            "q".repeat(MAX_QUERY_CHARS + 1)
        );
        assert!(matches!(parse_overlay(&big), Err(IpcReject::BadPayload(_))));
    }

    #[test]
    fn accepts_bare_and_object_resize_shapes() {
        assert_eq!(
            parse_overlay(r#"{"t":"resize","v":120.5}"#),
            Ok(OverlayMessage::Resize(ResizePayload {
                v: ResizeHeight::Bare(120.5)
            }))
        );
        assert_eq!(
            parse_overlay(r#"{"t":"resize","v":{"v":120.5,"immediate":true}}"#),
            Ok(OverlayMessage::Resize(ResizePayload {
                v: ResizeHeight::Object(ResizeObject {
                    v: 120.5,
                    immediate: true
                })
            }))
        );
        assert!(matches!(
            parse_overlay(r#"{"t":"resize","v":{"v":120.5,"bogus":1}}"#),
            Err(IpcReject::BadPayload(_))
        ));
    }

    #[test]
    fn settings_save_requires_cfg_object() {
        assert!(matches!(
            parse_settings(r#"{"t":"save"}"#),
            Err(IpcReject::BadPayload(_))
        ));
    }
}
