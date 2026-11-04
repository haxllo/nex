//! Local IPC between `NexUIProcess` (search/UI) and `NexIndexerProcess`
//! (discovery, SQLite, Tantivy).
//!
//! Transport is a JSON-line protocol over a loopback TCP socket. The UI only
//! ever transports an item snapshot; it never reads the Tantivy index and never
//! runs discovery, so the two processes cannot race on index files.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let service = Arc::clone(&service);
                let indexing = Arc::clone(&indexing);
                std::thread::spawn(move || handle_connection(stream, service, indexing));
            }
            Err(error) => crate::logging::warn(&format!("[nex-ipc] accept failed: {error}")),
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    service: Arc<RwLock<CoreService>>,
    indexing: Arc<AtomicBool>,
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
        dispatch(&request, &service, &indexing)
    };
    let _ = write_response(&mut stream, &response);
}

fn dispatch(
    request: &IpcRequest,
    service: &Arc<RwLock<CoreService>>,
    indexing: &AtomicBool,
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
            IpcResponse::ok(json!({
                "generation": generation,
                "changed": true,
                "indexing": false,
                "items": items_to_values(&items),
            }))
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
}
