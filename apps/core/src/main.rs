// GUI subsystem: Windows never allocates a console for `nex.exe`, so
// double-click / startup launches don't flash a black cmd window. CLI
// commands that print to the terminal reattach to the parent console in
// `attach_parent_console_if_present()` below, so `--status`, `--quit`,
// etc. still work when run from cmd/PowerShell/Windows Terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    attach_parent_console_if_present();

    let stdio_enabled = std::env::var("NEX_SUPPRESS_STDIO")
        .or_else(|_| std::env::var("SWIFTFIND_SUPPRESS_STDIO"))
        .map(|value| !(value == "1" || value.eq_ignore_ascii_case("true")))
        .unwrap_or(true);

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

    // Force-exit to terminate lingering background threads (icon
    // prefetch, tray updater, warm-release timer). The CRT's implicit
    // exit after main() returns does not reliably call ExitProcess on
    // all Windows toolchains, leaving detached threads alive.
    std::process::exit(0);
}

/// Reattach to the parent's console (if any) so CLI commands that print
/// to stdout/stderr (`--status`, `--status-json`, `--quit`, ... and the
/// `eprintln!` error paths above) keep working under the GUI subsystem.
///
/// When nex is started from cmd / PowerShell / Windows Terminal, this
/// attaches to that console and stdout/stderr land there as expected.
/// When started from Explorer, the shell, or the Run key (no parent
/// console), `AttachConsole` fails silently — no console is allocated,
/// which is exactly what prevents the startup flash.
#[cfg(windows)]
fn attach_parent_console_if_present() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        // Returns 0 on failure (e.g. no parent console); ignore it.
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
