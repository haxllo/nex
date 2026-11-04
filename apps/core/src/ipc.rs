use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::core_service::CoreService;

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcRequest {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn run_indexer_server(service: Arc<RwLock<CoreService>>, addr: &str) {
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[nex-ipc] bind failed on {addr}: {e}");
            return;
        }
    };
    eprintln!("[nex-ipc] listening on {addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let service = Arc::clone(&service);
                std::thread::spawn(move || handle_connection(stream, service));
            }
            Err(e) => eprintln!("[nex-ipc] accept failed: {e}"),
        }
    }
}

fn handle_connection(mut stream: TcpStream, service: Arc<RwLock<CoreService>>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let request: IpcRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            let resp = IpcResponse { ok: false, result: None, error: Some(format!("bad request: {e}")) };
            let _ = writeln!(stream, "{}", serde_json::to_string(&resp).unwrap_or_default());
            return;
        }
    };

    let response = match request.method.as_str() {
        "ping" => IpcResponse { ok: true, result: Some(serde_json::json!("pong")), error: None },
        "index_status" => {
            let svc = service.read().unwrap_or_else(|e| e.into_inner());
            IpcResponse {
                ok: true,
                result: Some(serde_json::json!({
                    "cached_items": svc.cached_items_len(),
                })),
                error: None,
            }
        }
        "search" => {
            let query = request.params.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let limit = request.params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            let svc = service.read().unwrap_or_else(|e| e.into_inner());
            match svc.search(&query, limit) {
                Ok(items) => {
                    let arr: Vec<serde_json::Value> = items
                        .iter()
                        .map(|i| serde_json::json!({
                            "id": i.id,
                            "kind": i.kind,
                            "title": i.title,
                            "path": i.path,
                            "subtitle": i.subtitle,
                        }))
                        .collect();
                    IpcResponse {
                        ok: true,
                        result: Some(serde_json::Value::Array(arr)),
                        error: None,
                    }
                }
                Err(e) => IpcResponse { ok: false, result: None, error: Some(e.to_string()) },
            }
        }
        other => IpcResponse { ok: false, result: None, error: Some(format!("unknown method: {other}")) },
    };

    let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap_or_default());
}

pub fn ensure_indexer_running() {
    if ipc_call("127.0.0.1:45821", &IpcRequest { method: "ping".to_string(), params: serde_json::Value::Null }).is_ok() {
        return;
    }
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST_SPAWN: Mutex<Option<Instant>> = Mutex::new(None);
    let now = Instant::now();
    {
        let mut last = LAST_SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        if last.map(|t| now.duration_since(t) < Duration::from_secs(5)).unwrap_or(false) {
            return;
        }
        *last = Some(now);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::process::Command::new(dir.join("NexIndexerProcess.exe")).spawn();
        }
    }
    std::thread::sleep(Duration::from_millis(300));
}

pub fn ipc_call(addr: &str, request: &IpcRequest) -> Result<IpcResponse, String> {
    use std::io::{BufRead, BufReader};
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    writeln!(stream, "{}", serde_json::to_string(request).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}