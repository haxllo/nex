//! `NexIndexerProcess`: owns discovery, file watchers, SQLite, and Tantivy.
//!
//! `NexUIProcess` spawns this process and mirrors its item snapshots over the
//! local IPC socket. Keeping every index write here removes cross-process
//! Tantivy races and keeps heavy indexing off the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use nex_core::core_service::{CoreService, IndexRefreshReport};
use nex_core::ipc::{self, INDEXER_ADDR, PROTOCOL_VERSION};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(600);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let once = args.iter().any(|arg| arg == "--once");
    let flag_value = |flag: &str| -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let parent_pid: Option<u32> = flag_value("--parent-pid").and_then(|value| value.parse().ok());
    let config_path = flag_value("--config").map(std::path::PathBuf::from);
    let addr = flag_value("--addr").unwrap_or_else(|| INDEXER_ADDR.to_string());

    #[cfg(windows)]
    attach_parent_console_if_present();

    let stdio_enabled = std::env::var("NEX_SUPPRESS_STDIO")
        .or_else(|_| std::env::var("SWIFTFIND_SUPPRESS_STDIO"))
        .map(|value| !(value == "1" || value.eq_ignore_ascii_case("true")))
        .unwrap_or(true);

    // Bind first: a second instance must not run a duplicate discovery pass.
    let Some(listener) = ipc::bind_indexer_listener(&addr) else {
        if stdio_enabled {
            println!("[nex-indexer] another indexer already owns {addr}; exiting");
        }
        return;
    };

    let config = match nex_core::config::load(config_path.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            if stdio_enabled {
                eprintln!("[nex-indexer] config load failed: {error}");
            }
            std::process::exit(2);
        }
    };

    let service = match CoreService::new(config.clone()) {
        Ok(service) => Arc::new(RwLock::new(service.with_runtime_providers())),
        Err(error) => {
            if stdio_enabled {
                eprintln!("[nex-indexer] service init failed: {error}");
            }
            std::process::exit(2);
        }
    };

    let indexing = Arc::new(AtomicBool::new(true));

    // Serve before the first scan finishes: readiness is "listening", not
    // "indexed". The UI can then read status (indexing=true) instead of
    // waiting for a full discovery pass before the socket answers.
    {
        let service = Arc::clone(&service);
        let indexing = Arc::clone(&indexing);
        std::thread::spawn(move || {
            nex_core::logging::info(&format!(
                "[nex-indexer] protocol_version={PROTOCOL_VERSION} serving {addr}"
            ));
            ipc::serve_indexer(listener, service, indexing);
        });
    }

    run_index_cycle(&service, &indexing, stdio_enabled);

    if once {
        std::process::exit(0);
    }

    start_watchers(&service);

    {
        let service = Arc::clone(&service);
        let indexing = Arc::clone(&indexing);
        std::thread::spawn(move || {
            maintenance_loop(
                service,
                indexing,
                config,
                config_path,
                stdio_enabled,
                parent_pid,
            )
        });
    }

    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn run_index_cycle(
    service: &Arc<RwLock<CoreService>>,
    indexing: &AtomicBool,
    stdio_enabled: bool,
) -> Option<IndexRefreshReport> {
    indexing.store(true, Ordering::Release);
    let outcome = {
        let service = service.read().unwrap_or_else(|error| error.into_inner());
        service.rebuild_index_incremental_with_report()
    };
    indexing.store(false, Ordering::Release);

    match outcome {
        Ok(report) => {
            nex_core::logging::info(&format!(
                "[nex-indexer] indexed_items={} discovered={} upserted={} removed={}",
                report.indexed_total,
                report.discovered_total,
                report.upserted_total,
                report.removed_total
            ));
            if stdio_enabled {
                println!("[nex-indexer] indexed {} items", report.indexed_total);
            }
            Some(report)
        }
        Err(error) => {
            nex_core::logging::warn(&format!("[nex-indexer] index cycle failed: {error}"));
            if stdio_enabled {
                eprintln!("[nex-indexer] index cycle failed: {error}");
            }
            None
        }
    }
}

fn start_watchers(service: &Arc<RwLock<CoreService>>) {
    let service_guard = service.read().unwrap_or_else(|error| error.into_inner());
    service_guard.start_stale_pruner(service);
    if let Err(error) = service_guard.start_file_watchers(service) {
        nex_core::logging::warn(&format!("[nex-indexer] watcher start failed: {error}"));
    }
    service_guard.start_application_watchers(service);
}

/// Picks up config edits (roots/excludes) and periodically re-reconciles so a
/// dropped watcher event cannot leave the index permanently stale.
fn maintenance_loop(
    service: Arc<RwLock<CoreService>>,
    indexing: Arc<AtomicBool>,
    config: nex_core::config::Config,
    config_path: Option<std::path::PathBuf>,
    stdio_enabled: bool,
    parent_pid: Option<u32>,
) {
    let mut last_modified = config_file_modified(&config.config_path);
    let mut last_reconcile = Instant::now();
    loop {
        std::thread::sleep(CONFIG_POLL_INTERVAL);

        // Orphan guard: without a UI process there is nobody to serve, and a
        // lingering indexer would hold the SQLite/Tantivy files open.
        if parent_pid.is_some_and(|pid| !process_is_alive(pid)) {
            nex_core::logging::info("[nex-indexer] UI process exited; shutting down");
            std::process::exit(0);
        }

        let modified = config_file_modified(&config.config_path);
        if modified != last_modified {
            last_modified = modified;
            match nex_core::config::load(config_path.as_deref()) {
                Ok(reloaded) => {
                    nex_core::logging::info("[nex-indexer] config changed; reloading index");
                    {
                        let service_guard =
                            service.read().unwrap_or_else(|error| error.into_inner());
                        if let Err(error) = service_guard.reconfigure_runtime_providers(&reloaded) {
                            nex_core::logging::warn(&format!(
                                "[nex-indexer] config reload failed: {error}"
                            ));
                        }
                    }
                    run_index_cycle(&service, &indexing, stdio_enabled);
                    start_watchers(&service);
                    last_reconcile = Instant::now();
                }
                Err(error) => nex_core::logging::warn(&format!(
                    "[nex-indexer] config reload skipped: {error}"
                )),
            }
            continue;
        }

        if last_reconcile.elapsed() >= RECONCILE_INTERVAL {
            run_index_cycle(&service, &indexing, stdio_enabled);
            last_reconcile = Instant::now();
        }
    }
}

fn config_file_modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0_u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(not(windows))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(windows)]
fn attach_parent_console_if_present() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
