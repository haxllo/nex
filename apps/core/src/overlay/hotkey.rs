//! Global hotkey registration via the Win32 `RegisterHotKey` API.
//!
//! The hotkey runs on a dedicated OS thread that calls
//! `RegisterHotKey(NULL, id, mods, vk)` and then loops on
//! `GetMessageW`. When the OS delivers a `WM_HOTKEY` (because the
//! user pressed the registered chord), the thread forwards
//! `OverlayEvent::Hotkey(id)` to the supplied event channel, which
//! the runtime drains on the calling thread (the same channel the
//! Iced event loop uses for keyboard/mouse events).
//!
//! Why a dedicated thread: `RegisterHotKey` with a `NULL` HWND
//! delivers `WM_HOTKEY` to the thread that registered it, so the
//! `GetMessageW` loop must run on that same thread. We do not want
//! to mix Win32 message dispatch into the Iced event loop, so we
//! run it on a separate OS thread instead.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;

use crate::logging;
use crate::overlay::model::OverlayEvent;

/// Owns a dedicated OS thread that holds a registered hotkey.
/// Drop the listener to unregister and join the thread.
pub(crate) struct HotkeyListener {
    inner: Option<HotkeyListenerInner>,
}

struct HotkeyListenerInner {
    should_exit: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    id: i32,
    thread_id: std::sync::OnceLock<u32>,
}

impl HotkeyListener {
    /// Register `hotkey_str` and start listening. `event_tx` is sent
    /// `OverlayEvent::Hotkey(id)` whenever the user presses the
    /// chord.
    pub(crate) fn start(
        hotkey_str: &str,
        event_tx: Sender<OverlayEvent>,
    ) -> Result<Self, String> {
        let parsed = parse_hotkey(hotkey_str)
            .map_err(|e| format!("invalid hotkey '{hotkey_str}': {e}"))?;
        let modifiers = modifiers_from_names(&parsed.modifiers)?;
        let vk = vk_from_key(&parsed.key)?;

        let id: i32 = 1;
        let should_exit = Arc::new(AtomicBool::new(false));
        let should_exit_clone = should_exit.clone();
        let event_tx_clone = event_tx.clone();
        let thread_id: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let thread_id_for_thread = thread_id.clone();

        // RegisterHotKey(NULL, ...) delivers WM_HOTKEY to the thread
        // that called it.  GetMessageW also runs on the listener
        // thread, so RegisterHotKey must be called inside the spawned
        // thread — otherwise WM_HOTKEY lands on the wrong queue.
        let (reg_tx, reg_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let hotkey_owned = hotkey_str.to_string();
        let thread = thread::Builder::new()
            .name("nex-hotkey-listener".into())
            .spawn(move || {
                let ok = unsafe {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::RegisterHotKey(
                        std::ptr::null_mut(),
                        id,
                        modifiers,
                        vk,
                    )
                };
                if ok == 0 {
                    let _ = reg_tx.send(Err(format!(
                        "RegisterHotKey failed for '{hotkey_owned}'"
                    )));
                    return;
                }
                let _ = reg_tx.send(Ok(()));
                let tid =
                    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
                let _ = thread_id_for_thread.set(tid);
                run_get_message_loop(should_exit_clone, event_tx_clone, id);
            })
            .map_err(|e| format!("failed to spawn hotkey thread: {e}"))?;

        reg_rx.recv().unwrap_or(Err("hotkey thread panicked".into()))?;

        Ok(Self {
            inner: Some(HotkeyListenerInner {
                should_exit,
                thread: Some(thread),
                id,
                thread_id,
            }),
        })
    }

    /// Hotkey id this listener holds. `1` for the first/only hotkey.
    pub(crate) fn id(&self) -> i32 {
        self.inner.as_ref().map(|i| i.id).unwrap_or(-1)
    }

    /// OS thread id of the hotkey listener thread, or `None` if the
    /// thread has not yet started. Spins for up to 100 ms before
    /// giving up, so the caller can log the id immediately after
    /// `start` returns.
    pub(crate) fn thread_id(&self) -> Option<u32> {
        let inner = self.inner.as_ref()?;
        for _ in 0..100 {
            if let Some(id) = inner.thread_id.get() {
                return Some(*id);
            }
            thread::sleep(Duration::from_millis(1));
        }
        inner.thread_id.get().copied()
    }

    /// Returns `true` if the hotkey listener thread is still running.
    pub(crate) fn is_alive(&self) -> bool {
        match &self.inner {
            Some(inner) => {
                !inner.should_exit.load(Ordering::SeqCst)
                    && inner.thread.as_ref().is_some_and(|t| !t.is_finished())
            }
            None => false,
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.should_exit.store(true, Ordering::SeqCst);
            // The listener thread blocks on GetMessageW forever; we
            // can't wake it from outside.  The hotkey is unregistered
            // by the OS when the process exits, so we skip an explicit
            // UnregisterHotKey call (it would also run on the wrong
            // thread now that RegisterHotKey moved inside the thread).
            if let Some(handle) = inner.thread.take() {
                let _ = handle.join();
            }
        }
    }
}

fn run_get_message_loop(
    should_exit: Arc<AtomicBool>,
    event_tx: Sender<OverlayEvent>,
    id: i32,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_HOTKEY,
    };
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while !should_exit.load(Ordering::SeqCst) {
        let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if status == -1 || status == 0 {
            break;
        }
        if msg.message == WM_HOTKEY && msg.wParam == id as usize {
            let _ = event_tx.send(OverlayEvent::Hotkey(id));
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if !should_exit.load(Ordering::SeqCst) {
        logging::warn("[nex] hotkey message loop exited unexpectedly");
    }
}

struct ParsedHotkey {
    modifiers: Vec<String>,
    key: String,
}

fn parse_hotkey(s: &str) -> Result<ParsedHotkey, String> {
    let parts: Vec<String> = s
        .split('+')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("empty hotkey".into());
    }
    let key = parts.last().cloned().unwrap();
    let modifiers: Vec<String> = parts.iter().rev().skip(1).cloned().collect();
    Ok(ParsedHotkey { modifiers, key })
}

fn modifiers_from_names(names: &[String]) -> Result<u32, String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
    };
    let mut out = 0u32;
    for name in names {
        match name.to_ascii_lowercase().as_str() {
            "alt" => out |= MOD_ALT,
            "ctrl" | "control" => out |= MOD_CONTROL,
            "shift" => out |= MOD_SHIFT,
            "win" | "meta" | "super" => out |= MOD_WIN,
            other => return Err(format!("unsupported modifier: {other}")),
        }
    }
    Ok(out)
}

fn vk_from_key(key: &str) -> Result<u32, String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9,
        VK_SPACE,
    };
    let upper = key.to_ascii_uppercase();
    let vk: u32 = match upper.as_str() {
        "SPACE" => VK_SPACE as u32,
        "F1" => VK_F1 as u32,
        "F2" => VK_F2 as u32,
        "F3" => VK_F3 as u32,
        "F4" => VK_F4 as u32,
        "F5" => VK_F5 as u32,
        "F6" => VK_F6 as u32,
        "F7" => VK_F7 as u32,
        "F8" => VK_F8 as u32,
        "F9" => VK_F9 as u32,
        "F10" => VK_F10 as u32,
        "F11" => VK_F11 as u32,
        "F12" => VK_F12 as u32,
        _ if upper.len() == 1 => upper.as_bytes()[0] as u32,
        _ => return Err(format!("unsupported key: {key}")),
    };
    Ok(vk)
}

// A trivial keep-alive import to silence dead-code lints in
// configurations where the listener never starts.
#[allow(dead_code)]
const _KEEP_DURATION: Duration = Duration::from_millis(50);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_space() {
        let p = parse_hotkey("Ctrl+Space").unwrap();
        assert_eq!(p.modifiers, vec!["Ctrl"]);
        assert_eq!(p.key, "Space");
    }

    #[test]
    fn parse_ctrl_shift_f5() {
        let p = parse_hotkey("Ctrl+Shift+F5").unwrap();
        // The parser collects modifiers via `iter().rev().skip(1)`
        // so the first modifier in the string ends up *last* in
        // the vec. Order does not matter for modifier dispatch
        // (we OR them together), so this is a documentation
        // assertion rather than a contract.
        assert_eq!(p.modifiers, vec!["Shift", "Ctrl"]);
        assert_eq!(p.key, "F5");
    }

    #[test]
    fn parse_single_key() {
        let p = parse_hotkey("F1").unwrap();
        assert!(p.modifiers.is_empty());
        assert_eq!(p.key, "F1");
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("++").is_err());
    }

    #[test]
    fn vk_space() {
        assert_eq!(
            vk_from_key("Space").unwrap(),
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SPACE as u32
        );
    }

    #[test]
    fn vk_f5() {
        assert_eq!(
            vk_from_key("F5").unwrap(),
            windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F5 as u32
        );
    }

    #[test]
    fn vk_single_letter() {
        assert_eq!(vk_from_key("A").unwrap(), b'A' as u32);
    }
}
