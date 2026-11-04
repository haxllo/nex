use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let once = args.iter().any(|a| a == "--once");

    #[cfg(windows)]
    attach_parent_console_if_present();

    let stdio_enabled = std::env::var("NEX_SUPPRESS_STDIO")
        .or_else(|_| std::env::var("SWIFTFIND_SUPPRESS_STDIO"))
        .map(|value| !(value == "1" || value.eq_ignore_ascii_case("true")))
        .unwrap_or(true);

    let config = match nex_core::config::load(None) {
        Ok(cfg) => cfg,
        Err(e) => {
            if stdio_enabled {
                eprintln!("[nex-indexer] config load failed: {e}");
            }
            std::process::exit(2);
        }
    };

    use std::sync::{Arc, RwLock};
    let service = match nex_core::core_service::CoreService::new(config.clone()) {
        Ok(s) => Arc::new(RwLock::new(s.with_runtime_providers())),
        Err(e) => {
            if stdio_enabled {
                eprintln!("[nex-indexer] service init failed: {e}");
            }
            std::process::exit(2);
        }
    };

    {
        let svc = service.read().unwrap_or_else(|e| e.into_inner());
        match svc.rebuild_index() {
            Ok(n) => {
                if stdio_enabled {
                    println!("[nex-indexer] indexed {n} items");
                }
            }
            Err(e) => {
                if stdio_enabled {
                    eprintln!("[nex-indexer] rebuild failed: {e}");
                }
                std::process::exit(1);
            }
        }
    }

    if once {
        std::process::exit(0);
    }

    nex_core::ipc::run_indexer_server(service, "127.0.0.1:45821");
}

#[cfg(windows)]
fn attach_parent_console_if_present() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}