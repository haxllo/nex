//! Global hotkey via a low-level keyboard hook (`WH_KEYBOARD_LL`).
//!
//! Unlike `RegisterHotKey`, a low-level keyboard hook can intercept
//! any key — including the Win key pressed alone — without
//! consuming a global hotkey slot. The hook runs on a dedicated OS
//! thread that owns the `GetMessageW` pump required by
//! `SetWindowsHookExW`.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;

use crate::logging;
use crate::overlay::model::OverlayEvent;

// ---------------------------------------------------------------------------
// Static globals
// ---------------------------------------------------------------------------

struct HookContext {
    sender: Sender<OverlayEvent>,
    hotkey_id: i32,
    target_key: u32,
    target_is_win: bool,
    required_mods: Vec<u32>,
}

static HOOK_CTX: OnceLock<HookContext> = OnceLock::new();

// Track VK of a Win key-down that was consumed by the hotkey, so the
// matching key-up is also consumed.
static CONSUMED_WIN_VK: AtomicU32 = AtomicU32::new(0);
static OVERLAY_HAS_FOCUS: AtomicBool = AtomicBool::new(false);
static SUPPRESS_FOCUS_ESCAPE: AtomicBool = AtomicBool::new(false);

/// True from a consumed bare-Win key-down through its matching key-up.
/// The overlay host uses this to avoid treating the Win press's transient
/// focus loss as click-outside dismissal; the hotkey event owns that toggle.
pub(crate) fn is_bare_win_press_active() -> bool {
    SUPPRESS_FOCUS_ESCAPE.load(Ordering::SeqCst)
}

pub(crate) fn set_overlay_focus(focused: bool) {
    OVERLAY_HAS_FOCUS.store(focused, Ordering::SeqCst);
}

/// Clears the focus-loss guard after the runtime has handled the Win toggle.
pub(crate) fn finish_bare_win_press() {
    SUPPRESS_FOCUS_ESCAPE.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Menu suppression strategy
//
// A bare Win press is a shell gesture. `WH_KEYBOARD_LL` runs before the
// input is posted, so consuming the event and applying a menu mask lets us
// prevent that gesture.
//
// We use the AutoHotkey #MenuMaskKey approach: send a completed inert
// VK 0xE8 key stroke while Win is held. The toggle is dispatched only after
// Win is released: hiding on key-down can move focus to Explorer while Win
// is still held and reintroduce the Start-menu gesture it prevents.
//
//   Win keydown  → send 0xE8 DOWN + UP, eat keydown
//   Win keyup    → eat keyup, fire hotkey
//
// Non-Win hotkeys eat the target keydown as before (prevents the key
// from reaching the focused window).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ALL_MODS: [u32; 5] = [VK_CTRL, VK_ALT, VK_SHIFT, VK_LWIN, VK_RWIN];

const VK_CTRL: u32 = 0x11;
const VK_ALT: u32 = 0x12;
const VK_SHIFT: u32 = 0x10;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

// Physical VKs — keyboards send side-specific codes, not generic ones.
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;

// Track modifier state via atomic flags updated from the hook proc.
// GetAsyncKeyState reports false once the WebView consumes Ctrl-down.
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_key_down(vk: u32) -> bool {
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Unassigned VK used as menu-mask key. Mirrors AutoHotkey's
/// `#MenuMaskKey vkE8` — never conflicts with real keys.
pub(crate) const VK_MENU_MASK: u16 = 0xE8;

/// Send an inert key press to make the shell treat a bare Win press as a
/// chord. The mask must be a completed press during Win-down.
fn send_menu_mask_key() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut inputs: [INPUT; 2] = unsafe { std::mem::zeroed() };
    inputs[0].r#type = INPUT_KEYBOARD;
    inputs[0].Anonymous.ki = KEYBDINPUT { wVk: VK_MENU_MASK, wScan: 0, dwFlags: 0, time: 0, dwExtraInfo: 0 };
    inputs[1].r#type = INPUT_KEYBOARD;
    inputs[1].Anonymous.ki = KEYBDINPUT { wVk: VK_MENU_MASK, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(inputs.len() as u32, inputs.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

/// Move keyboard focus away from the overlay to the shell/desktop.
/// Called from the runtime loop (main thread) just before hide, so the
/// hide's window.set_visible(false) does not trigger a focus transition
/// that could make Explorer re-check raw input for the Start menu gesture.
pub(crate) fn hand_focus_to_shell() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, SetForegroundWindow};

    let shell = unsafe { GetShellWindow() };
    if !shell.is_null() {
        unsafe { SetForegroundWindow(shell); }
    }
}

// ---------------------------------------------------------------------------
// Hook proc
// ---------------------------------------------------------------------------

unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;

    if n_code < 0 {
        return unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
        };
    }

    let msg = w_param as u32;
    let is_keydown = msg == 0x0100 || msg == 0x0104;
    let is_keyup = msg == 0x0101 || msg == 0x0105;

    let Some(ctx) = HOOK_CTX.get() else {
        return unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
        };
    };

    let kb = unsafe { *(l_param as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let injected = (kb.flags & 0x10) != 0;

    // Track modifiers via atomics — GetAsyncKeyState lies when
    // the WebView has focus and has consumed the Ctrl key-down.
    if !injected {
        match vk {
            VK_LCONTROL | VK_RCONTROL => CTRL_DOWN.store(is_keydown, Ordering::SeqCst),
            VK_LMENU | VK_RMENU => ALT_DOWN.store(is_keydown, Ordering::SeqCst),
            VK_LSHIFT | VK_RSHIFT => SHIFT_DOWN.store(is_keydown, Ordering::SeqCst),
            _ => {}
        }
    }

    // --- Win key-up: toggle only after the physical key is up ---
    if ctx.target_is_win && is_keyup && !injected {
        let consumed_vk = CONSUMED_WIN_VK.load(Ordering::SeqCst);
        if consumed_vk != 0 && vk == consumed_vk {
            CONSUMED_WIN_VK.store(0, Ordering::SeqCst);
            // Dispatch only after physical Win-up, so hiding cannot transfer focus
            // to Explorer during the key press.
            let _ = ctx.sender.send(OverlayEvent::Hotkey(ctx.hotkey_id));
            return 1;
        }
    }

    // Only key-down triggers matter for hotkey dispatch.
    if !is_keydown {
        return unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
        };
    }

    let is_target = if ctx.target_is_win {
        vk == VK_LWIN || vk == VK_RWIN
    } else {
        vk == ctx.target_key
    };

    if is_target && !injected {
        let mods_ok = ctx.required_mods.iter().all(|&m| match m {
            VK_LWIN | VK_RWIN => is_key_down(VK_LWIN) || is_key_down(VK_RWIN),
            VK_CTRL => CTRL_DOWN.load(Ordering::SeqCst),
            VK_ALT => ALT_DOWN.load(Ordering::SeqCst),
            VK_SHIFT => SHIFT_DOWN.load(Ordering::SeqCst),
            other => is_key_down(other),
        });

        let is_target_mod = |m: u32| -> bool {
            m == ctx.target_key || (ctx.target_is_win && (m == VK_LWIN || m == VK_RWIN))
        };
        let extra_free = ALL_MODS.iter().all(|&m| {
            if is_target_mod(m) || ctx.required_mods.contains(&m)
                || (m == VK_LWIN && ctx.required_mods.contains(&VK_RWIN))
                || (m == VK_RWIN && ctx.required_mods.contains(&VK_LWIN))
            {
                true
            } else {
                !is_key_down(m)
            }
        });

        if mods_ok && extra_free {
            if ctx.target_is_win {
                // This completed mask press reaches the shell while Win is
                // held; injected events pass through due to the guard above.
                CONSUMED_WIN_VK.store(vk, Ordering::SeqCst);
                SUPPRESS_FOCUS_ESCAPE.store(true, Ordering::SeqCst);
                // Mask injected synchronously from hook thread so shell
                // sees Win+0xE8 before Win is released. Focus handoff
                // happens in runtime_loop before hide — safer on main thread.
                send_menu_mask_key();
                return 1;
            }
            let _ = ctx.sender.send(OverlayEvent::Hotkey(ctx.hotkey_id));
            return 1;
        }
    }

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
    }
}

// ---------------------------------------------------------------------------
// HotkeyListener
// ---------------------------------------------------------------------------

pub(crate) struct HotkeyListener {
    inner: Option<HotkeyListenerInner>,
}

struct HotkeyListenerInner {
    should_exit: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    thread_id: Arc<OnceLock<u32>>,
}

impl HotkeyListener {
    pub(crate) fn start(hotkey_str: &str, event_tx: Sender<OverlayEvent>) -> Result<Self, String> {
        let parsed = parse_hotkey(hotkey_str)
            .map_err(|e| format!("invalid hotkey '{hotkey_str}': {e}"))?;
        let required_mods = modifier_vks_from_names(&parsed.modifiers)?;
        let target_key = vk_from_key(&parsed.key)?;
        let target_is_win = target_key == VK_LWIN;

        let should_exit = Arc::new(AtomicBool::new(false));
        let should_exit_for_thread = should_exit.clone();
        let thread_id: Arc<OnceLock<u32>> = Arc::new(OnceLock::new());
        let thread_id_for_thread = thread_id.clone();

        let _ = HOOK_CTX.set(HookContext { sender: event_tx, hotkey_id: 1, target_key, target_is_win, required_mods });

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = thread::Builder::new()
            .name("nex-hotkey-listener".into())
            .spawn(move || {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, MSG,
                    WH_KEYBOARD_LL,
                };
                let hook_id = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL as i32, Some(keyboard_hook_proc), std::ptr::null_mut(), 0)
                };
                if hook_id.is_null() {
                    let _ = ready_tx.send(Err("SetWindowsHookExW failed".into()));
                    return;
                }
                let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
                let _ = thread_id_for_thread.set(tid);
                let _ = ready_tx.send(Ok(()));

                let mut msg: MSG = unsafe { std::mem::zeroed() };
                while !should_exit_for_thread.load(Ordering::SeqCst) {
                    let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
                    if status == -1 || status == 0 { break; }
                    unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
                }
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook_id); }
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
        Ok(Self { inner: Some(HotkeyListenerInner { should_exit, thread: Some(thread), thread_id }) })
    }

    pub(crate) fn thread_id(&self) -> Option<u32> {
        let inner = self.inner.as_ref()?;
        for _ in 0..100 {
            if let Some(id) = inner.thread_id.get() { return Some(*id); }
            thread::sleep(Duration::from_millis(1));
        }
        inner.thread_id.get().copied()
    }

    pub(crate) fn is_alive(&self) -> bool {
        match &self.inner {
            Some(inner) => !inner.should_exit.load(Ordering::SeqCst)
                && inner.thread.as_ref().is_some_and(|t| !t.is_finished()),
            None => false,
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.should_exit.store(true, Ordering::SeqCst);
            if let Some(&tid) = inner.thread_id.get() {
                logging::info(&format!("[nex] shutdown: posting WM_QUIT to hotkey thread {tid}"));
                post_quit_to_thread(tid);
            }
            if let Some(handle) = inner.thread.take() {
                logging::info("[nex] shutdown: joining hotkey thread");
                let _ = handle.join();
            }
        }
    }
}

fn post_quit_to_thread(thread_id: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
    unsafe { let _ = PostThreadMessageW(thread_id, WM_QUIT, 0, 0); }
}

// ---------------------------------------------------------------------------
// Hotkey string parsing
// ---------------------------------------------------------------------------

struct ParsedHotkey { modifiers: Vec<String>, key: String }

fn parse_hotkey(s: &str) -> Result<ParsedHotkey, String> {
    let parts: Vec<String> = s.split('+').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { return Err("empty hotkey".into()); }
    Ok(ParsedHotkey {
        key: parts.last().cloned().unwrap(),
        modifiers: parts.iter().rev().skip(1).cloned().collect(),
    })
}

fn modifier_vks_from_names(names: &[String]) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for name in names {
        out.push(match name.to_ascii_lowercase().as_str() {
            "alt" => VK_ALT, "ctrl" | "control" => VK_CTRL, "shift" => VK_SHIFT,
            "win" | "meta" | "super" => VK_LWIN,
            other => return Err(format!("unsupported modifier: {other}")),
        });
    }
    Ok(out)
}

fn vk_from_key(key: &str) -> Result<u32, String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_F1, VK_F10, VK_F11, VK_F12, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_SPACE,
    };
    let upper = key.to_ascii_uppercase();
    Ok(match upper.as_str() {
        "WIN" | "META" | "SUPER" => VK_LWIN, "SPACE" => VK_SPACE as u32,
        "F1" => VK_F1 as u32, "F2" => VK_F2 as u32, "F3" => VK_F3 as u32, "F4" => VK_F4 as u32,
        "F5" => VK_F5 as u32, "F6" => VK_F6 as u32, "F7" => VK_F7 as u32, "F8" => VK_F8 as u32,
        "F9" => VK_F9 as u32, "F10" => VK_F10 as u32, "F11" => VK_F11 as u32, "F12" => VK_F12 as u32,
        _ if upper.len() == 1 => upper.as_bytes()[0] as u32,
        _ => return Err(format!("unsupported key: {key}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parse_ctrl_space() { let p = parse_hotkey("Ctrl+Space").unwrap(); assert_eq!(p.modifiers, vec!["Ctrl"]); assert_eq!(p.key, "Space"); }
    #[test] fn parse_ctrl_shift_f5() { let p = parse_hotkey("Ctrl+Shift+F5").unwrap(); assert_eq!(p.modifiers, vec!["Shift", "Ctrl"]); assert_eq!(p.key, "F5"); }
    #[test] fn parse_single_key() { let p = parse_hotkey("F1").unwrap(); assert!(p.modifiers.is_empty()); assert_eq!(p.key, "F1"); }
    #[test] fn parse_rejects_empty() { assert!(parse_hotkey("").is_err()); assert!(parse_hotkey("++").is_err()); }
    #[test] fn vk_space() { assert_eq!(vk_from_key("Space").unwrap(), windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SPACE as u32); }
    #[test] fn vk_f5() { assert_eq!(vk_from_key("F5").unwrap(), windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F5 as u32); }
    #[test] fn vk_single_letter() { assert_eq!(vk_from_key("A").unwrap(), b'A' as u32); }
    #[test] fn vk_win_key() { assert_eq!(vk_from_key("Win").unwrap(), VK_LWIN); }
    #[test] fn modifier_vks() { let mods = modifier_vks_from_names(&["Ctrl".into(), "Shift".into()]).unwrap(); assert_eq!(mods, vec![VK_CTRL, VK_SHIFT]); }
}
