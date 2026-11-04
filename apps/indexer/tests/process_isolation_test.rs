//! End-to-end checks for process-isolated indexing.
//!
//! Network + process spawning makes these slow and port-bound, so they are
//! opt-in on Windows CI:
//!
//! ```text
//! NEX_RUN_PROCESS_ISOLATION_TEST=1 cargo test -p nex --test process_isolation_test
//! ```

#![cfg(windows)]

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use nex_core::ipc::{self, items_from_values, IpcRequest, IpcResponse, PROTOCOL_VERSION};
use nex_core::search::SearchFilter;
use serde_json::Value;

fn enabled() -> bool {
    std::env::var("NEX_RUN_PROCESS_ISOLATION_TEST").ok().as_deref() == Some("1")
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

fn temp_root(name: &str) -> PathBuf {
    // Not the system temp dir: LOCALAPPDATA\Temp is a builtin discovery
    // exclusion, so files there would never be indexed.
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir().expect("cwd").join(path)
            }
        })
        .unwrap_or_else(|| std::env::current_dir().expect("cwd").join("target"))
        .join("process-isolation-tests");
    let root = base.join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("files")).expect("create temp root");
    root
}

fn write_config(root: &Path) -> PathBuf {
    let config_path = root.join("config.toml");
    let files = root.join("files").to_string_lossy().replace('\\', "/");
    let db = root.join("index.sqlite3").to_string_lossy().replace('\\', "/");
    let mut file = std::fs::File::create(&config_path).expect("create config");
    write!(
        file,
        "index_db_path = \"{db}\"\ndiscovery_roots = [\"{files}\"]\nshow_files = true\nshow_folders = true\n"
    )
    .expect("write config");
    config_path
}

struct Indexer {
    child: Child,
    addr: String,
    log_path: PathBuf,
}

impl Indexer {
    fn start(addr: &str, config_path: &Path) -> Self {
        let log_path = config_path
            .parent()
            .expect("config parent")
            .join("indexer-output.log");
        let _ = std::fs::remove_file(&log_path);
        let stdout = std::fs::File::create(&log_path).expect("indexer log");
        let stderr = stdout.try_clone().expect("clone log handle");
        let child = Command::new(env!("CARGO_BIN_EXE_NexIndexerProcess"))
            .arg("--config")
            .arg(config_path)
            .arg("--addr")
            .arg(addr)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("spawn indexer");
        Self {
            child,
            addr: addr.to_string(),
            log_path,
        }
    }

    fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if ipc::indexer_is_alive(&self.addr) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn output(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    fn call(&self, method: &str, params: Value) -> IpcResponse {
        ipc::ipc_call(&self.addr, &IpcRequest::new(method, params)).expect("ipc call")
    }

    fn await_item(&self, needle: &str, timeout: Duration) -> Vec<nex_core::model::SearchItem> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let response = self.call("snapshot", Value::Null);
            if response.ok {
                if let Some(items) = response.result.as_ref().and_then(|r| r.get("items")) {
                    let items = items_from_values(items.as_array().expect("items array"));
                    if items.iter().any(|item| item.path.ends_with(needle)) {
                        return items;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!("indexer never published {needle}");
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn indexer_publishes_searchable_snapshot_and_recovers_after_crash() {
    if !enabled() {
        eprintln!("skipping: set NEX_RUN_PROCESS_ISOLATION_TEST=1 to run");
        return;
    }

    let root = temp_root("snapshot");
    std::fs::write(root.join("files").join("message.txt"), "hello").expect("seed file");
    let config_path = write_config(&root);
    let addr = format!("127.0.0.1:{}", free_port());

    let indexer = Indexer::start(&addr, &config_path);
    assert!(indexer.wait_ready(Duration::from_secs(30)), "indexer never listened");

    let items = indexer.await_item("message.txt", Duration::from_secs(120));
    assert!(
        items.iter().any(|item| item.path.ends_with("message.txt")),
        "indexed file missing from snapshot"
    );

    // Search over IPC returns the same file. Tantivy syncs after the
    // store snapshot lands, so poll until the indexed search catches up.
    let mut found_via_search = false;
    let search_deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < search_deadline {
        let search = indexer.call("search", serde_json::json!({ "query": "message.txt", "limit": 20 }));
        assert!(search.ok, "search failed: {:?}", search.error);
        let search_items = items_from_values(
            search
                .result
                .as_ref()
                .and_then(|r| r.get("items"))
                .and_then(Value::as_array)
                .expect("search items"),
        );
        if search_items
            .iter()
            .any(|item| item.path.ends_with("message.txt"))
        {
            found_via_search = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(
        found_via_search,
        "search did not return the indexed file within 120s"
    );

    // Version mismatch is rejected explicitly instead of being guessed.
    let mismatch = ipc::ipc_call(
        &addr,
        &IpcRequest {
            version: PROTOCOL_VERSION + 1,
            method: "ping".to_string(),
            params: Value::Null,
        },
    )
    .expect("mismatch call");
    assert!(!mismatch.ok);
    assert!(mismatch
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("protocol version mismatch"));

    // UI-side parity: the same snapshot searched without Tantivy finds the file.
    let config = nex_core::config::load(Some(&config_path)).expect("load config");
    let ui = nex_core::core_service::CoreService::new_ui_client(config).expect("ui service");
    ui.apply_external_snapshot(items);
    let ranked = ui
        .search_with_filter("message.txt", 20, &SearchFilter::default())
        .expect("ui search");
    assert!(
        ranked.iter().any(|item| item.path.ends_with("message.txt")),
        "UI snapshot search missed the file"
    );

    // Crash recovery: once the indexer is gone the UI must see it down, and a
    // restart must publish a usable snapshot again.
    drop(indexer);
    let mut died = false;
    for _ in 0..50 {
        if !ipc::indexer_is_alive(&addr) {
            died = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(died, "indexer kept answering after being killed");

    let mut restarted = Indexer::start(&addr, &config_path);
    assert!(
        restarted.wait_ready(Duration::from_secs(30)),
        "restarted indexer never listened (exited={:?})\n{}",
        restarted.child.try_wait(),
        restarted.output()
    );
    let status = restarted.call("status", Value::Null);
    assert!(status.ok, "status failed after restart: {:?}", status.error);
    let recovered = restarted.await_item("message.txt", Duration::from_secs(120));
    assert!(
        recovered.iter().any(|item| item.path.ends_with("message.txt")),
        "restarted indexer never republished the file"
    );

    // Graceful shutdown: the indexer exits on request so `nex --quit` never
    // leaves it running behind.
    assert!(
        ipc::shutdown_indexer(&addr),
        "shutdown request was not acknowledged"
    );
    let mut stopped = false;
    for _ in 0..50 {
        if !ipc::indexer_is_alive(&addr) {
            stopped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(stopped, "indexer kept answering after shutdown request");
    let _ = restarted.child.wait();
    assert!(
        restarted.child.try_wait().expect("poll exit").is_some(),
        "indexer process did not exit after shutdown"
    );

    let _ = std::fs::remove_dir_all(&root);
}
