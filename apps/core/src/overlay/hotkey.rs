//! Global hotkey via a low-level keyboard hook (`WH_KEYBOARD_LL`).
//!
//! Unlike `RegisterHotKey`, a low-level keyboard hook can intercept
//! any key — including the Win key pressed alone — without
//! consuming a global hotkey slot. The hook runs on a dedicated OS
//! thread that owns the `GetMessageW` pump required by
//! `SetWindowsHookExW`.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;

use crate::logging;
use crate::overlay::model::OverlayEvent;

// ---------------------------------------------------------------------------
// Static global: the hook proc has no user-data parameter, so we stash
// everything we need in a process-wide `OnceLock`.
// ---------------------------------------------------------------------------

struct HookContext {
    sender: Sender<OverlayEvent>,
    hotkey_id: i32,
    target_key: u32,
    /// When true, accept either VK_LWIN or VK_RWIN as the target key.
    target_is_win: bool,
    /// VK codes that must be held (from the parsed modifier list).
    required_mods: Vec<u32>,
}

static HOOK_CTX: OnceLock<HookContext> = OnceLock::new();

// ---------------------------------------------------------------------------
// Helper: async key state
// ---------------------------------------------------------------------------

/// Returns `true` if the virtual-key key is currently pressed.
fn is_key_down(vk: u32) -> bool {
    // The high bit of the return value is set when the key is down.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// All modifier VK codes we care about when checking for "no extra
/// modifiers".
const ALL_MODS: [u32; 5] = [VK_CTRL, VK_ALT, VK_SHIFT, VK_LWIN, VK_RWIN];

const VK_CTRL: u32 = 0x11;
const VK_ALT: u32 = 0x12;
const VK_SHIFT: u32 = 0x10;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

// ---------------------------------------------------------------------------
// Hook proc (unsafe, extern "system")
// ---------------------------------------------------------------------------

/// Low-level keyboard hook procedure.
///
/// # Safety
/// Called by the OS hook dispatcher. `wParam` and `lParam` carry
/// valid message/key data per the Win32 API contract.
unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;

    if n_code >= 0 {
        let msg = w_param as u32;
        if msg == 0x0100 || msg == 0x0104 {
            // WM_KEYDOWN or WM_SYSKEYDOWN
            if let Some(ctx) = HOOK_CTX.get() {
                // SAFETY: l_param is a valid pointer to KBDLLHOOKSTRUCT per Win32 contract
                let kb = unsafe { *(l_param as *const KBDLLHOOKSTRUCT) };
                let vk = kb.vkCode;

                let is_target = if ctx.target_is_win {
                    vk == VK_LWIN || vk == VK_RWIN
                } else {
                    vk == ctx.target_key
                };

                if is_target {
                    // Check required modifiers are held.
                    // VK_LWIN and VK_RWIN are treated as equivalent.
                    let mods_ok = ctx.required_mods.iter().all(|&m| {
                        match m {
                            VK_LWIN | VK_RWIN => is_key_down(VK_LWIN) || is_key_down(VK_RWIN),
                            other => is_key_down(other),
                        }
                    });

                    // Check NO extra unexpected modifiers are held.
                    // The target key itself is always excluded — it's expected
                    // to be down (it triggered the hook). For example, when
                    // intercepting VK_LWIN alone, VK_LWIN being down should
                    // not disqualify the match.
                    let is_target_mod = |m: u32| -> bool {
                        m == ctx.target_key
                            || (ctx.target_is_win && (m == VK_LWIN || m == VK_RWIN))
                    };
                    let extra_free = ALL_MODS.iter().all(|&m| {
                        if is_target_mod(m) {
                            true // this IS the triggered key, always ok
                        } else if ctx.required_mods.contains(&m)
                            || (m == VK_LWIN && ctx.required_mods.contains(&VK_RWIN))
                            || (m == VK_RWIN && ctx.required_mods.contains(&VK_LWIN))
                        {
                            true // required by hotkey config
                        } else {
                            !is_key_down(m)
                        }
                    });

                    if mods_ok && extra_free {
                        let _ = ctx.sender.send(OverlayEvent::Hotkey(ctx.hotkey_id));
                        return 1; // swallow the keystroke
                    }
                }
            }
        }
    }

    // SAFETY: n_code, w_param, l_param are valid per the hook contract.
    // Passing null_mut() as hhk for LL hooks (no chained hook).
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) }
}

// ---------------------------------------------------------------------------
// HotkeyListener
// ---------------------------------------------------------------------------

/// Owns a dedicated OS thread that holds a `WH_KEYBOARD_LL` hook.
/// Drop the listener to unhook and join the thread.
pub(crate) struct HotkeyListener {
    inner: Option<HotkeyListenerInner>,
}

struct HotkeyListenerInner {
    should_exit: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    id: i32,
    thread_id: Arc<OnceLock<u32>>,
}

impl HotkeyListener {
    /// Install `hotkey_str` and start listening. `event_tx` receives
    /// `OverlayEvent::Hotkey(id)` whenever the user presses the
    /// chord.
    pub(crate) fn start(
        hotkey_str: &str,
        event_tx: Sender<OverlayEvent>,
    ) -> Result<Self, String> {
        let parsed = parse_hotkey(hotkey_str)
            .map_err(|e| format!("invalid hotkey '{hotkey_str}': {e}"))?;
        let required_mods = modifier_vks_from_names(&parsed.modifiers)?;
        let target_key = vk_from_key(&parsed.key)?;
        let target_is_win = target_key == VK_LWIN;

        let id: i32 = 1;
        let should_exit = Arc::new(AtomicBool::new(false));
        let should_exit_for_thread = should_exit.clone();
        let thread_id: Arc<OnceLock<u32>> = Arc::new(OnceLock::new());
        let thread_id_for_thread = thread_id.clone();

        // Populate the global hook context. The hook proc reads it
        // without synchronisation because it is set once before the
        // hook is installed and never modified afterward.
        let _ = HOOK_CTX.set(HookContext {
            sender: event_tx,
            hotkey_id: id,
            target_key,
            target_is_win,
            required_mods,
        });

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = thread::Builder::new()
            .name("nex-hotkey-listener".into())
            .spawn(move || {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetMessageW, SetWindowsHookExW, TranslateMessage, DispatchMessageW,
                    MSG, WH_KEYBOARD_LL,
                };

                // Install the low-level keyboard hook on this thread.
                // SetWindowsHookExW with WH_KEYBOARD_LL must be called
                // from the thread whose message loop will dispatch the
                // hook callbacks.
                let hook_id = unsafe {
                    SetWindowsHookExW(
                        WH_KEYBOARD_LL as i32,
                        Some(keyboard_hook_proc),
                        std::ptr::null_mut(), // hMod — not needed for LL hooks
                        0, // dwThreadId — 0 = all threads, but LL hooks always run on installing thread
                    )
                };

                if hook_id.is_null() {
                    let _ = ready_tx.send(Err("SetWindowsHookExW failed".into()));
                    return;
                }

                let tid = unsafe {
                    windows_sys::Win32::System::Threading::GetCurrentThreadId()
                };
                let _ = thread_id_for_thread.set(tid);
                let _ = ready_tx.send(Ok(()));

                // GetMessageW loop — required to deliver hook messages.
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                while !should_exit_for_thread.load(Ordering::SeqCst) {
                    let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
                    if status == -1 || status == 0 {
                        break;
                    }
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }

                // Clean up the hook before exiting.
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook_id);
                }

                if !should_exit_for_thread.load(Ordering::SeqCst) {
                    logging::warn("[nex] hotkey message loop exited unexpectedly");
                }
            })
            .map_err(|e| format!("failed to spawn hotkey thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("hotkey thread panicked".into()),
        }

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

            // Post WM_QUIT to wake the GetMessageW loop.
            if let Some(&tid) = inner.thread_id.get() {
                logging::info(&format!("[nex] shutdown: posting WM_QUIT to hotkey thread {tid}"));
                post_quit_to_thread(tid);
            } else {
                logging::warn("[nex] shutdown: hotkey thread_id not available, cannot post WM_QUIT");
            }

            if let Some(handle) = inner.thread.take() {
                logging::info("[nex] shutdown: joining hotkey thread");
                let _ = handle.join();
                logging::info("[nex] shutdown: hotkey thread joined");
            }
        }
    }
}

/// Post `WM_QUIT` to the listener thread so its `GetMessageW` returns.
fn post_quit_to_thread(thread_id: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
    unsafe {
        let _ = PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
    }
}

// ---------------------------------------------------------------------------
// Hotkey string parsing
// ---------------------------------------------------------------------------

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

/// Map modifier names to VK codes (not MOD_* flags — those were for
/// RegisterHotKey).
fn modifier_vks_from_names(names: &[String]) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for name in names {
        let vk = match name.to_ascii_lowercase().as_str() {
            "alt" => VK_ALT,
            "ctrl" | "control" => VK_CTRL,
            "shift" => VK_SHIFT,
            "win" | "meta" | "super" => VK_LWIN,
            other => return Err(format!("unsupported modifier: {other}")),
        };
        out.push(vk);
    }
    Ok(out)
}

/// Map a key name to its virtual-key code.
fn vk_from_key(key: &str) -> Result<u32, String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9,
        VK_SPACE,
    };
    let upper = key.to_ascii_uppercase();
    let vk: u32 = match upper.as_str() {
        "WIN" | "META" | "SUPER" => VK_LWIN,
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

    #[test]
    fn vk_win_key() {
        assert_eq!(vk_from_key("Win").unwrap(), VK_LWIN);
    }

    #[test]
    fn modifier_vks() {
        let mods = modifier_vks_from_names(&["Ctrl".into(), "Shift".into()]).unwrap();
        assert_eq!(mods, vec![VK_CTRL, VK_SHIFT]);
    }
}
