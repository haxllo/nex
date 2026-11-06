//! Local IPC between `NexUIProcess` (search/UI) and `NexIndexerProcess`
//! (discovery, SQLite, Tantivy).
//!
//! Transport is a JSON-line protocol over a loopback TCP socket. The UI only
//! ever transports an item snapshot; it never reads the Tantivy index and never
//! runs discovery, so the two processes cannot race on index files.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
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
        "pre_score": item.pre_score,
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
    let mut item = SearchItem::from_owned_with_usage(
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
    );
    item.pre_score = value.get("pre_score").and_then(Value::as_i64);
    Some(item)
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
        "sync" => sync_long_poll(request, service, indexing),
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
        // Tantivy-only seed for the UI process. The UI re-ranks these
        // candidates with its own filter and personalization; this avoids
        // the UI's full-cache scan at scale.
        "search_seed" => {
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("");
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(500) as usize;
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            match service.search_tantivy_seed(query, limit) {
                Ok(items) => IpcResponse::ok(json!({ "items": items_to_values(&items) })),
                Err(error) => IpcResponse::error(error.to_string()),
            }
        }
        other => IpcResponse::error(format!("unknown method: {other}")),
    }
}

/// One immediate `sync` answer (the pre-long-poll behavior).
fn sync_once(
    request: &IpcRequest,
    service: &Arc<RwLock<CoreService>>,
    indexing: &AtomicBool,
) -> IpcResponse {
        // Reported on every sync, not only during the startup cycle:
        // watcher resyncs and config reloads run full passes too.
        let full_pass = service
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .full_pass_progress();
        let mut response = (|| -> IpcResponse {
            let known = request
                .params
                .get("generation")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let client_instance = request
                .params
                .get("instance")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let service = service.read().unwrap_or_else(|e| e.into_inner());
            let indexing = indexing.load(Ordering::Acquire);
            // While a rebuild runs the cache is mid-refresh: echo the caller's
            // generation so it does not advance past an item set it never saw.
            if indexing {
                return IpcResponse::ok(json!({
                    "generation": known,
                    "changed": false,
                    "indexing": true,
                }));
            }

            match service.publish_outcome(known, client_instance) {
                crate::core_service::PublishOutcome::Unchanged {
                    generation,
                    instance,
                } => IpcResponse::ok(json!({
                    "generation": generation,
                    "instance": instance,
                    "changed": false,
                    "indexing": false,
                })),
                crate::core_service::PublishOutcome::Delta {
                    generation,
                    instance,
                    upserts,
                    deleted,
                } => IpcResponse::ok(json!({
                    "generation": generation,
                    "instance": instance,
                    "changed": true,
                    "indexing": false,
                    "delta": {
                        "upserts": items_to_values(&upserts),
                        "deleted": deleted,
                    },
                })),
                crate::core_service::PublishOutcome::Snapshot {
                    generation,
                    instance,
                } => match service.hot_snapshot_items() {
                    Ok(items) => IpcResponse::ok(json!({
                        "generation": generation,
                        "instance": instance,
                        "changed": true,
                        "indexing": false,
                        "items": items_to_values(&items),
                    })),
                    Err(error) => {
                        IpcResponse::error(format!("snapshot failed: {error}"))
                    }
                },
            }
        })();
        if let (Some(progress), Some(Value::Object(result))) =
            (full_pass, response.result.as_mut())
        {
            result.insert("full_pass".to_string(), progress.to_json());
        }
        response
}

/// Longest a `sync` may park on the server; the caller's read timeout must
/// exceed it.
pub const SYNC_MAX_WAIT: Duration = Duration::from_secs(30);

struct SyncNotifier {
    epoch: Mutex<u64>,
    changed: Condvar,
}

static SYNC_NOTIFIER: SyncNotifier = SyncNotifier {
    epoch: Mutex::new(0),
    changed: Condvar::new(),
};

/// Wake parked `sync` long-polls: the changelog, the indexing flag or the
/// full-pass state moved.
pub fn notify_sync_waiters() {
    let mut epoch = SYNC_NOTIFIER.epoch.lock().unwrap_or_else(|e| e.into_inner());
    *epoch = epoch.wrapping_add(1);
    drop(epoch);
    SYNC_NOTIFIER.changed.notify_all();
}

fn sync_epoch() -> u64 {
    *SYNC_NOTIFIER.epoch.lock().unwrap_or_else(|e| e.into_inner())
}

/// Block until the epoch differs from `seen` or `deadline` passes.
fn wait_for_sync_epoch(seen: u64, deadline: Instant) {
    let mut epoch = SYNC_NOTIFIER.epoch.lock().unwrap_or_else(|e| e.into_inner());
    while *epoch == seen {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        epoch = SYNC_NOTIFIER
            .changed
            .wait_timeout(epoch, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

fn sync_state(response: &IpcResponse) -> (bool, bool) {
    let result = response.result.as_ref();
    let flag = |key: &str| {
        result
            .and_then(|r| r.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    (flag("indexing"), result.is_some_and(|r| r.get("full_pass").is_some()))
}

/// `sync` with an optional `wait_ms`: answers at once when the changelog
/// moved, otherwise parks until it moves, the indexing/full-pass state flips,
/// or the wait elapses. Each connection has its own thread, so a parked
/// long-poll never blocks other requests. A caller that omits `wait_ms` gets
/// the plain immediate answer.
fn sync_long_poll(
    request: &IpcRequest,
    service: &Arc<RwLock<CoreService>>,
    indexing: &AtomicBool,
) -> IpcResponse {
    let wait = Duration::from_millis(
        request
            .params
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )
    .min(SYNC_MAX_WAIT);
    let deadline = Instant::now() + wait;
    let mut first_state = None;
    loop {
        // Read the epoch before computing so a notification that lands
        // mid-computation is not lost.
        let seen = sync_epoch();
        let response = sync_once(request, service, indexing);
        let state = sync_state(&response);
        let changed = response
            .result
            .as_ref()
            .and_then(|r| r.get("changed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let first = *first_state.get_or_insert(state);
        if !response.ok || changed || state != first || Instant::now() >= deadline {
            return response;
        }
        wait_for_sync_epoch(seen, deadline);
    }
}

fn write_response(stream: &mut TcpStream, response: &IpcResponse) -> std::io::Result<()> {
    let encoded = serde_json::to_string(response).unwrap_or_default();
    writeln!(stream, "{encoded}")?;
    stream.flush()
}

pub fn ipc_call(addr: &str, request: &IpcRequest) -> Result<IpcResponse, String> {
    ipc_call_with_timeout(addr, request, Duration::from_secs(60))
}

pub fn ipc_call_with_timeout(
    addr: &str,
    request: &IpcRequest,
    read_timeout: Duration,
) -> Result<IpcResponse, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|e| e.to_string())?;
    let encoded = serde_json::to_string(request).map_err(|e| e.to_string())?;
    writeln!(stream, "{encoded}").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}

/// Keystroke budget for the indexer seed query. A busy indexer (rebuild at
/// background priority) must not stall typing; on timeout the caller ranks
/// its local cache instead.
pub const SEARCH_SEED_TIMEOUT: Duration = Duration::from_millis(500);

/// Ask the indexer for Tantivy candidates. The UI re-ranks them with its own
/// filter and personalization; an IPC failure or an empty index lets the
/// caller fall back to the local cache.
pub fn search_seed(
    addr: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchItem>, String> {
    let response = ipc_call_with_timeout(
        addr,
        &IpcRequest::new("search_seed", json!({ "query": query, "limit": limit })),
        SEARCH_SEED_TIMEOUT,
    )?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "indexer search failed".to_string()));
    }
    let result = response.result.unwrap_or(Value::Null);
    let items = result
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items_from_values(items))
        .unwrap_or_default();
    Ok(items)
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
    // CREATE_NO_WINDOW: the indexer is a console-subsystem binary, and
    // without it Windows allocates a visible console titled with the exe path.
    let mut command = std::process::Command::new(indexer);
    command
        .arg("--parent-pid")
        .arg(std::process::id().to_string());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let _ = command.spawn();

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

    fn memory_service() -> Arc<RwLock<CoreService>> {
        let db = crate::index_store::open_memory().unwrap();
        Arc::new(RwLock::new(
            CoreService::with_connection(crate::config::Config::default(), db).unwrap(),
        ))
    }

    fn sync_request(known: &Value, wait_ms: u64) -> IpcRequest {
        IpcRequest::new(
            "sync",
            json!({
                "generation": known["generation"],
                "instance": known["instance"],
                "wait_ms": wait_ms,
            }),
        )
    }

    fn synced_state(service: &Arc<RwLock<CoreService>>, indexing: &AtomicBool) -> Value {
        let first = sync_once(&IpcRequest::new("sync", json!({})), service, indexing);
        first.result.expect("sync result")
    }

    #[test]
    fn sync_without_wait_answers_immediately() {
        let service = memory_service();
        let indexing = AtomicBool::new(false);
        let known = synced_state(&service, &indexing);
        let started = Instant::now();
        let response = sync_long_poll(&sync_request(&known, 0), &service, &indexing);
        assert!(response.ok);
        assert_eq!(response.result.unwrap()["changed"], false);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sync_long_poll_parks_until_timeout_when_idle() {
        let service = memory_service();
        let indexing = AtomicBool::new(false);
        let known = synced_state(&service, &indexing);
        let started = Instant::now();
        let response = sync_long_poll(&sync_request(&known, 300), &service, &indexing);
        assert_eq!(response.result.unwrap()["changed"], false);
        assert!(started.elapsed() >= Duration::from_millis(280));
    }

    #[test]
    fn sync_long_poll_wakes_when_changelog_moves() {
        let service = memory_service();
        let indexing = AtomicBool::new(false);
        let known = synced_state(&service, &indexing);
        let writer = Arc::clone(&service);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let guard = writer.read().unwrap();
            guard
                .upsert_item(&SearchItem::new("a", "app", "Alpha", "C:\\alpha.exe"))
                .unwrap();
        });
        let started = Instant::now();
        let response = sync_long_poll(&sync_request(&known, 20_000), &service, &indexing);
        handle.join().unwrap();
        let result = response.result.unwrap();
        assert_eq!(result["changed"], true);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
