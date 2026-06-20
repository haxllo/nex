//! Console Ctrl+C / Ctrl+Break handling for `--foreground` runs.
//!
//! `nex.exe` uses the `windows` GUI subsystem (no console is allocated
//! on double-click / Run-key start). When launched from a terminal
//! (`nex --foreground`), it reattaches to the parent console for
//! stdout/stderr, but Ctrl+C is NOT routed into the runtime's message
//! loops — there is no handler, so the process keeps running the tao
//! event loop and the hotkey `GetMessageW` loop, and lingers in
//! Task Manager after Ctrl+C.
//!
//! This module installs a `SetConsoleCtrlHandler` that:
//!   1. On the first Ctrl+C / Break / close: sends `ExternalQuit` down
//!      the same event channel the tray uses, triggering the graceful
//!      shutdown path (overlay hide → event-loop exit → hotkey WM_QUIT
//!      join). Returns TRUE so the OS does not also terminate us.
//!   2. On a second signal (or if graceful shutdown has not completed
//!      within the OS's timeout): `std::process::exit(130)` so the user
//!      is never stuck waiting on a hung shutdown.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crossbeam_channel::Sender;

use crate::overlay::model::OverlayEvent;

/// Holds the event-channel sender the handler posts `ExternalQuit` to.
/// Set once by [`install`] at runtime startup; `None` before that or
/// after [`clear`] at shutdown.
static QUIT_TX: OnceLock<Mutex<Option<Sender<OverlayEvent>>>> = OnceLock::new();

/// Number of console control signals received so far. The first one
/// requests graceful quit; the second forces immediate exit.
static SIGNAL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Install the console control handler. `event_tx` is the same channel
/// the tray and hotkey listener write to; the handler sends
/// `ExternalQuit` on it for graceful shutdown. Safe to call once;
/// later calls replace the stored sender.
pub(crate) fn install(event_tx: Sender<OverlayEvent>) {
    let slot = QUIT_TX.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(event_tx);
    }
    // GUI-subsystem binaries do not receive CTRL_C_EVENT by default,
    // even after AttachConsole. Calling SetConsoleCtrlHandler with
    // NULL handler + TRUE enables the console control signal routing.
    // Without this, our handler never fires and Ctrl+C is silently
    // swallowed by the OS.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1);
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), 1);
    }
}

/// Drop the stored sender so a late signal cannot deliver `ExternalQuit`
/// to a runtime that has already torn its channels down. Called at the
/// end of graceful shutdown.
pub(crate) fn clear() {
    if let Some(slot) = QUIT_TX.get() {
        if let Ok(mut guard) = slot.lock() {
            *guard = None;
        }
    }
}

unsafe extern "system" fn handler(ctrl: u32) -> i32 {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT,
        CTRL_SHUTDOWN_EVENT,
    };
    // Only react to interactive interrupt / close signals. Other event
    // types (logoff, shutdown) fall through to default handling.
    let is_interrupt = matches!(
        ctrl,
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    );
    if !is_interrupt {
        return 0;
    }

    let count = SIGNAL_COUNT.fetch_add(1, Ordering::SeqCst);
    if count >= 1 {
        // Second signal: force-exit. Never make the user wait on a
        // hung graceful shutdown.
        std::process::exit(130);
    }

    // First signal: request graceful quit via the same path the tray
    // uses. If the sender is already gone (runtime mid-teardown), fall
    // back to a forced exit so the process does not appear to ignore
    // the user.
    if let Some(slot) = QUIT_TX.get() {
        if let Ok(guard) = slot.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(OverlayEvent::ExternalQuit);
                // Force-exit after 3 seconds if the graceful shutdown
                // has not completed (hotkey join deadlock, etc.).
                let _ = std::thread::Builder::new()
                    .name("nex-ctrlc-timeout".into())
                    .spawn(|| {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        std::process::exit(130);
                    });
                return 1; // handled — OS will not terminate us
            }
        }
    }
    std::process::exit(130);
}
