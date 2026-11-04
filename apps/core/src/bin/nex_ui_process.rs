#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    attach_parent_console_if_present();

    let stdio_enabled = std::env::var("NEX_SUPPRESS_STDIO")
        .or_else(|_| std::env::var("SWIFTFIND_SUPPRESS_STDIO"))
        .map(|value| !(value == "1" || value.eq_ignore_ascii_case("true")))
        .unwrap_or(true);

    unsafe { std::env::set_var("NEX_UI_PROCESS", "1"); }
    // Start (or reuse) the indexer before the UI builds its service, so the
    // first keystroke has a snapshot to search. Failure is non-fatal: the UI
    // keeps its last store snapshot and retries on the next monitor tick.
    if !nex_core::ipc::ensure_indexer_running(nex_core::ipc::INDEXER_ADDR) && stdio_enabled {
        eprintln!("[nex] indexer not reachable yet; search uses the last snapshot until it starts");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match nex_core::runtime::parse_cli_args(&args) {
        Ok(options) => options,
        Err(error) => {
            if stdio_enabled {
                eprintln!("[nex] {error}");
            }
            std::process::exit(2);
        }
    };

    if let Err(error) = nex_core::runtime::run_with_options(options) {
        if stdio_enabled {
            eprintln!("[nex] runtime failed: {error}");
        }
        std::process::exit(1);
    }

    std::process::exit(0);
}

#[cfg(windows)]
fn attach_parent_console_if_present() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}