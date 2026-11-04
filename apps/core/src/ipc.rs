//! Local IPC between `NexUIProcess` (search/UI) and `NexIndexerProcess`
//! (discovery, SQLite, Tantivy).
//!
//! Transport is a JSON-line protocol over a loopback TCP socket. The UI only
//! ever transports an item snapshot; it never reads the Tantivy index and never
//! runs discovery, so the two processes cannot race on index files.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::SearchMode;
use crate::core_service::CoreService;
use crate::model::SearchItem;

pub const PROTOCOL_VERSION: u32 = 1;
pub const INDEXER_ADDR: &str = "127.0.0.1:45821";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    /// Protocol version the caller speaks. The server rejects mismatches
    /// instead of guessing field semantics.
    #[serde(default)]
    pub version: u32,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl IpcRequest {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    #[serde(default)]
    pub version: u32,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IpcResponse {
    fn ok(result: Value) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }
}

pub fn item_to_value(item: &SearchItem) -> Value {
    json!({
        "id": item.id,
        "kind": item.kind,
        "title": item.title,
        "path": item.path,
        "subtitle": item.subtitle,
        "use_count": item.use_count,
        "last_accessed": item.last_accessed_epoch_secs,
        "launch_count": item.launch_count,
        "last_launched": item.last_launched_at,
    })
}

pub fn item_from_value(value: &Value) -> Option<SearchItem> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let id = text("id");
    if id.is_empty() {
        return None;
    }
    Some(SearchItem::from_owned_with_usage(
        id,
        text("kind"),
        text("title"),
        text("path"),
        text("subtitle"),
        value
            .get("use_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        value.get("last_accessed").and_then(Value::as_i64).unwrap_or(0),
        value
            .get("launch_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        value.get("last_launched").and_then(Value::as_i64).unwrap_or(0),
    ))
}

pub fn items_to_values(items: &[SearchItem]) -> Vec<Value> {
    items.iter().map(item_to_value).collect()
}

pub fn items_from_values(values: &[Value]) -> Vec<SearchItem> {
    values.iter().filter_map(item_from_value).collect()
}

/// Bind the indexer listener. Binding before the first rebuild means a second
/// instance exits without running a duplicate (expensive) discovery pass.
pub fn bind_indexer_listener(addr: &str) -> Option<TcpListener> {
    match TcpListener::bind(addr) {
        Ok(listener) => Some(listener),
        Err(error) => {
            crate::logging::info(&format!("[nex-ipc] bind failed on {addr}: {error}"));
            None
        }
    }
}

/// Serve requests until the process exits.
pub fn serve_indexer(
    listener: TcpListener,
    service: Arc<RwLock<CoreService>>,
    indexing: Arc<AtomicBool>,
) {
    crate::logging::info(&format!("[nex-ipc] listening on {INDEXER_ADDR}"));
    // Last item set handed to a poll, so a later sync can send a delta.
    let published: Arc<Mutex<Option<(u64, Vec<SearchItem>)>>> = Arc::new(Mutex::new(None));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let service = Arc::clone(&service);
                let indexing = Arc::clone(&indexing);
                let published = Arc::clone(&published);
                std::thread::spawn(move || handle_connection(stream, service, indexing, published));
            }
            Err(error) => crate::logging::warn(&format!("[nex-ipc] accept failed: {error}")),
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    service: Arc<RwLock<CoreService>>,
    indexing: Arc<AtomicBool>,
    published: Arc<Mutex<Option<(u64, Vec<SearchItem>)>>>,
) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let request: IpcRequest = match serde_json::from_str(line.trim()) {
        Ok(request) => request,
        Err(error) => {
            let response = IpcResponse::error(format!("bad request: {error}"));
            let _ = write_response(&mut stream, &response);
            return;
        }
    };

    let response = if request.version != PROTOCOL_VERSION {
        IpcResponse::error(format!(
            "protocol version mismatch: client={} server={PROTOCOL_VERSION}",
            request.version
        ))
    } else {
        dispatch(&request, &service, &indexing, &published)
    };
    let _ = write_response(&mut stream, &response);
}

fn dispatch(
    request: &IpcRequest,
    service: &Arc<RwLock<CoreService>>,
    indexing: &AtomicBool,
    published: &Mutex<Option<(u64, Vec<SearchItem>)>>,
) -> IpcResponse {
    match request.method.as_str() {
        "ping" => IpcResponse::ok(json!({ "version": PROTOCOL_VERSION })),
        "shutdown" => {
            // The UI is going away; exit promptly so no orphaned indexer
            // holds the SQLite/Tantivy files open. Reply first, then exit
            // from a helper thread so the response flushes before we die.
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                std::process::exit(0);
            });
            IpcResponse::ok(json!({ "stopped": true }))
        }
        "status" => {
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            IpcResponse::ok(json!({
                "version": PROTOCOL_VERSION,
                "generation": service.search_generation(),
                "cached_items": service.cached_items_len(),
                "indexing": indexing.load(Ordering::Acquire),
            }))
        }
        "snapshot" => {
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            let items = match service.all_items_snapshot() {
                Ok(items) => items,
                Err(error) => return IpcResponse::error(format!("snapshot failed: {error}")),
            };
            IpcResponse::ok(json!({
                "generation": service.search_generation(),
                "items": items_to_values(&items),
            }))
        }
        // One round trip for the UI's poll: reports whether anything changed
        // and ships the item set only when it did. Mirrors the old
        // status+snapshot pair so the caller does not open two connections.
        "sync" => {
            let known = request
                .params
                .get("generation")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            let generation = service.search_generation();
            let indexing = indexing.load(Ordering::Acquire);
            // While a rebuild runs the cache is mid-refresh: echo the caller's
            // generation so it does not advance past an item set it never saw.
            if generation == known || indexing {
                return IpcResponse::ok(json!({
                    "generation": if indexing { known } else { generation },
                    "changed": false,
                    "indexing": indexing,
                }));
            }
            let items = match service.all_items_snapshot() {
                Ok(items) => items,
                Err(error) => return IpcResponse::error(format!("snapshot failed: {error}")),
            };
            drop(service); // diffing/serializing does not need the service lock

            let mut published = published.lock().unwrap_or_else(|e| e.into_inner());
            let response = match published.as_ref() {
                // Delta only when the caller's set is exactly the one we
                // published last; a missed generation needs a full snapshot.
                Some((published_generation, previous)) if *published_generation == known => {
                    match build_item_delta(previous, &items) {
                        Some((upserts, deleted)) => json!({
                            "generation": generation,
                            "changed": true,
                            "indexing": false,
                            "delta": {
                                "upserts": items_to_values(&upserts),
                                "deleted": deleted,
                            },
                        }),
                        None => json!({
                            "generation": generation,
                            "changed": true,
                            "indexing": false,
                            "items": items_to_values(&items),
                        }),
                    }
                }
                _ => json!({
                    "generation": generation,
                    "changed": true,
                    "indexing": false,
                    "items": items_to_values(&items),
                }),
            };
            *published = Some((generation, items));
            IpcResponse::ok(response)
        }
        // Writes the UI must not perform itself: the indexer owns the store.
        "record_launch" => {
            let id = request.params.get("id").and_then(Value::as_str).unwrap_or("");
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            match service.record_successful_launch_by_id(id) {
                Ok(()) => IpcResponse::ok(json!({})),
                Err(error) => IpcResponse::error(error.to_string()),
            }
        }
        "record_selection" => {
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("");
            let id = request.params.get("id").and_then(Value::as_str).unwrap_or("");
            let mode = match request
                .params
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("all")
            {
                "apps" => SearchMode::Apps,
                "files" => SearchMode::Files,
                "folders" => SearchMode::Folders,
                "actions" => SearchMode::Actions,
                "clipboard" => SearchMode::Clipboard,
                _ => SearchMode::All,
            };
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            match service.record_query_selection_hint(query, mode, id) {
                Ok(()) => IpcResponse::ok(json!({})),
                Err(error) => IpcResponse::error(error.to_string()),
            }
        }
        "recent_documents" => {
            const REFRESH_SECS: i64 = 600;
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            if service.recent_documents_scan_due(REFRESH_SECS) {
                match service.apply_recent_documents() {
                    Ok(applied) if applied > 0 => crate::logging::info(&format!(
                        "[nex-indexer] recent_documents applied={applied}"
                    )),
                    Ok(_) => {}
                    Err(error) => crate::logging::warn(&format!(
                        "[nex-indexer] recent_documents failed: {error}"
                    )),
                }
            }
            IpcResponse::ok(json!({}))
        }
        "delete_item" => {
            let id = request.params.get("id").and_then(Value::as_str).unwrap_or("");
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            match service.delete_item_by_id(id) {
                Ok(()) => IpcResponse::ok(json!({})),
                Err(error) => IpcResponse::error(error.to_string()),
            }
        }
        "search" => {
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("");
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20) as usize;
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            match service.search(query, limit) {
                Ok(items) => IpcResponse::ok(json!({ "items": items_to_values(&items) })),
                Err(error) => IpcResponse::error(error.to_string()),
            }
        }
        other => IpcResponse::error(format!("unknown method: {other}")),
    }
}

/// Compute small upserts/deletes between the previously published set and the
/// current one. Returns None when the change is large enough that a full
/// snapshot is cheaper than a delta.
fn build_item_delta(
    previous: &[SearchItem],
    current: &[SearchItem],
) -> Option<(Vec<SearchItem>, Vec<String>)> {
    const MAX_DELTA_ENTRIES: usize = 512;

    let previous_by_id: HashMap<&str, &SearchItem> =
        previous.iter().map(|item| (item.id.as_str(), item)).collect();
    let current_ids: HashSet<&str> = current.iter().map(|item| item.id.as_str()).collect();

    let mut upserts = Vec::new();
    for item in current {
        match previous_by_id.get(item.id.as_str()) {
            Some(old) if *old == item => {}
            _ => upserts.push(item.clone()),
        }
    }
    let deleted: Vec<String> = previous
        .iter()
        .filter(|item| !current_ids.contains(item.id.as_str()))
        .map(|item| item.id.clone())
        .collect();

    if upserts.len() > MAX_DELTA_ENTRIES || deleted.len() > MAX_DELTA_ENTRIES {
        return None;
    }
    // Replacing most of the set costs more than a full snapshot.
    if (upserts.len() + deleted.len()) * 4 > current.len() {
        return None;
    }
    Some((upserts, deleted))
}

fn write_response(stream: &mut TcpStream, response: &IpcResponse) -> std::io::Result<()> {
    let encoded = serde_json::to_string(response).unwrap_or_default();
    writeln!(stream, "{encoded}")?;
    stream.flush()
}

pub fn ipc_call(addr: &str, request: &IpcRequest) -> Result<IpcResponse, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    let encoded = serde_json::to_string(request).map_err(|e| e.to_string())?;
    writeln!(stream, "{encoded}").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}

pub fn indexer_is_alive(addr: &str) -> bool {
    matches!(
        ipc_call(addr, &IpcRequest::new("ping", Value::Null)),
        Ok(response) if response.ok
    )
}

/// Ask the indexer to exit voluntarily. Returns true when the indexer
/// acknowledged, or when nothing was listening in the first place.
pub fn shutdown_indexer(addr: &str) -> bool {
    match ipc_call(addr, &IpcRequest::new("shutdown", Value::Null)) {
        Ok(response) => response.ok,
        Err(_) => !indexer_is_alive(addr),
    }
}

/// Fire-and-forget command to the indexer.
///
/// The UI process must not write the store: launch history, query selections,
/// MRU scans, and prune deletions are forwarded here. Failures are dropped —
/// the next snapshot/delta reconciles any missed write.
pub fn send_indexer_command(method: &'static str, params: Value) {
    if std::env::var("NEX_UI_PROCESS").ok().as_deref() != Some("1") {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("nex-indexer-cmd".into())
        .spawn(move || {
            let _ = ipc_call(INDEXER_ADDR, &IpcRequest::new(method, params));
        });
}

/// Path to the indexer binary that ships beside the current executable.
pub fn indexer_exe_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("NexIndexerProcess.exe"))
}

/// Start the indexer when it is not answering yet. Rate-limited so a crash loop
/// cannot spawn a new process on every keystroke.
pub fn ensure_indexer_running(addr: &str) -> bool {
    if indexer_is_alive(addr) {
        return true;
    }
    static LAST_SPAWN: Mutex<Option<Instant>> = Mutex::new(None);
    let now = Instant::now();
    {
        let mut last = LAST_SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| now.duration_since(at) < Duration::from_secs(5)) {
            return false;
        }
        *last = Some(now);
    }
    let Some(indexer) = indexer_exe_path() else {
        return false;
    };
    crate::logging::info("[nex-ipc] indexer not reachable, starting NexIndexerProcess");
    // The parent pid lets the indexer exit on its own when the UI process
    // disappears, even if the UI crashed instead of quitting cleanly.
    let _ = std::process::Command::new(indexer)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .spawn();

    // The process needs a moment to bind its listener; a bounded wait keeps
    // the first keystroke from falling back to a cold local cache.
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if indexer_is_alive(addr) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_keeps_version_and_params() {
        let request = IpcRequest::new("search", json!({ "query": "nex" }));
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: IpcRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.method, "search");
        assert_eq!(decoded.params["query"], "nex");
    }

    #[test]
    fn requests_without_version_are_rejected() {
        let decoded: IpcRequest = serde_json::from_str(r#"{"method":"ping"}"#).unwrap();
        assert_ne!(decoded.version, PROTOCOL_VERSION);
    }

    #[test]
    fn item_roundtrip_preserves_searchable_fields() {
        let item = SearchItem::from_owned_with_usage(
            "file:1".to_string(),
            "file".to_string(),
            "message.txt".to_string(),
            r"C:\Users\me\Downloads\message.txt".to_string(),
            "Downloads".to_string(),
            3,
            1_700_000_000,
            2,
            1_700_000_100,
        );
        let decoded = item_from_value(&item_to_value(&item)).expect("item roundtrip");
        assert_eq!(decoded.id, item.id);
        assert_eq!(decoded.path, item.path);
        assert_eq!(decoded.subtitle, item.subtitle);
        assert_eq!(decoded.use_count, item.use_count);
        assert_eq!(decoded.launch_count, item.launch_count);
        assert_eq!(decoded.last_launched_at, item.last_launched_at);
    }

    #[test]
    fn item_without_id_is_skipped() {
        assert!(item_from_value(&json!({ "title": "orphan" })).is_none());
    }

    #[test]
    fn shutdown_request_roundtrips() {
        let request = IpcRequest::new("shutdown", Value::Null);
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: IpcRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.method, "shutdown");
    }

    #[test]
    fn version_mismatch_request_fails_to_parse_as_current() {
        let decoded: IpcRequest =
            serde_json::from_str(r#"{"version":999,"method":"ping"}"#).unwrap();
        assert_ne!(decoded.version, PROTOCOL_VERSION);
    }

    #[test]
    fn delta_lists_upserts_and_deletes() {
        let a = SearchItem::new("a", "app", "Alpha", r"C:\a.exe");
        let b = SearchItem::new("b", "app", "Beta", r"C:\b.exe");
        let a2 = SearchItem::new("a", "app", "Alpha 2", r"C:\a.exe");
        let c = SearchItem::new("c", "app", "Gamma", r"C:\c.exe");

        let (upserts, deleted) =
            build_item_delta(&[a, b], &[a2, c]).expect("small change should be a delta");
        let ids: Vec<&str> = upserts.iter().map(|item| item.id.as_str()).collect();
        assert!(ids.contains(&"a"), "changed item is an upsert");
        assert!(ids.contains(&"c"), "new item is an upsert");
        assert!(!ids.contains(&"b"), "unchanged item is not resent");
        assert_eq!(deleted, vec!["b".to_string()]);
    }

    #[test]
    fn delta_falls_back_to_full_for_large_changes() {
        let previous: Vec<SearchItem> = (0..100)
            .map(|i| SearchItem::new(&format!("old{i}"), "app", "Old", "C:\\old.exe"))
            .collect();
        let current: Vec<SearchItem> = (0..100)
            .map(|i| SearchItem::new(&format!("new{i}"), "app", "New", "C:\\new.exe"))
            .collect();
        assert!(build_item_delta(&previous, &current).is_none());
    }
}
