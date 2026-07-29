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

#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::os::windows::io::FromRawHandle;

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
pub(crate) fn is_win_key_hotkey() -> bool {
    HOOK_CTX.get().map(|ctx| ctx.target_is_win).unwrap_or(false)
}

/// Check if raw-input VK matches the configured hotkey (target key +
/// required modifiers, no extra modifiers held).  Uses GetAsyncKeyState
/// for modifier checks.  Used by the WM_INPUT handler when the overlay
/// is visible and Chromium's WH_KEYBOARD_LL hook intercepts our hook.
pub(crate) fn check_raw_input_hotkey(vk: u16) -> bool {
    let Some(ctx) = HOOK_CTX.get() else { return false };
    if ctx.target_is_win {
        return vk == VK_LWIN as u16 || vk == VK_RWIN as u16;
    }
    // VK must match target key.
    if vk as u32 != ctx.target_key { return false; }
    // All required modifiers must be held.
    let mods_ok = ctx.required_mods.iter().all(|&m| match m {
        VK_LWIN | VK_RWIN => is_key_down(VK_LWIN) || is_key_down(VK_RWIN),
        VK_CTRL => is_key_down(VK_LCONTROL) || is_key_down(VK_RCONTROL),
        VK_ALT => is_key_down(VK_LMENU) || is_key_down(VK_RMENU),
        VK_SHIFT => is_key_down(VK_LSHIFT) || is_key_down(VK_RSHIFT),
        other => is_key_down(other),
    });
    if !mods_ok { return false; }
    // No extra modifiers (that aren't the target or required) may be held.
    ALL_MODS.iter().all(|&m| {
        if m == ctx.target_key || ctx.required_mods.contains(&m) { return true; }
        !is_key_down(m)
    })
}

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

/// Set by the elevated helper's pipe reader on Win key hotkey detection.
pub(crate) fn set_suppress_focus_escape(suppress: bool) {
    SUPPRESS_FOCUS_ESCAPE.store(suppress, Ordering::SeqCst);
}

/// Check if the configured hotkey is currently pressed using
/// GetAsyncKeyState.  Works regardless of foreground window or thread
/// context.  Used by the polling fallback thread (runtime_loop) when
/// WH_KEYBOARD_LL and WM_INPUT both fail to fire (e.g. Task Manager /
/// elevated UWP windows have focus).
pub(crate) fn is_hotkey_pressed() -> bool {
    let Some(ctx) = HOOK_CTX.get() else { return false };
    if ctx.target_is_win {
        return is_key_down(VK_LWIN) || is_key_down(VK_RWIN);
    }
    // Target key must be down.
    if !is_key_down(ctx.target_key) { return false; }
    // All required modifiers must be held.
    if !ctx.required_mods.iter().all(|&m| match m {
        VK_LWIN | VK_RWIN => is_key_down(VK_LWIN) || is_key_down(VK_RWIN),
        VK_CTRL => is_key_down(VK_LCONTROL) || is_key_down(VK_RCONTROL),
        VK_ALT => is_key_down(VK_LMENU) || is_key_down(VK_RMENU),
        VK_SHIFT => is_key_down(VK_LSHIFT) || is_key_down(VK_RSHIFT),
        other => is_key_down(other),
    }) { return false; }
    // No extra (non-target, non-required) modifiers may be held.
    ALL_MODS.iter().all(|&m| {
        if m == ctx.target_key || ctx.required_mods.contains(&m) { return true; }
        if ctx.target_is_win && (m == VK_LWIN || m == VK_RWIN) { return true; }
        !is_key_down(m)
    })
}

// ---------------------------------------------------------------------------
// Menu suppression strategy
//
// The Start Menu detects a bare Win tap via raw input, which bypasses
// WH_KEYBOARD_LL entirely. Returning 1 from our hook cannot block it.
//
// We use the AutoHotkey #MenuMaskKey approach but with a critical twist:
// the mask key (unassigned VK 0xE8) is HELD DOWN for the entire Win
// key press AND re-held during the hide transition. This ensures that
// whenever the overlay hide triggers a focus change to Explorer,
// Explorer's raw input check sees Win + 0xE8 simultaneously — never a
// bare Win.
//
//   Win keydown  → send 0xE8 DOWN (held), eat keydown, fire hotkey
//   Win keyup    → send 0xE8 UP, eat keyup
//   Hide         → re-send 0xE8 DOWN, hide window, send 0xE8 UP
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

/// Send a single key-down for the menu-mask key. The mask is held
/// (no corresponding key-up) so raw input consumers see Win+mask
/// simultaneously during the critical hide/window-hide window.
fn send_mask_down() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT { wVk: VK_MENU_MASK, wScan: 0, dwFlags: 0, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
}

/// Release the menu-mask key held by [`send_mask_down`].
fn send_mask_up() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT { wVk: VK_MENU_MASK, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
}

/// Re-send mask down and spin-wait for RIT to register it.
/// Called from the UI thread inside UiCommand::Hide, right before
/// window.set_visible(false), so raw input consumers see Win+mask
/// during the hide's focus transition.
pub(crate) fn hold_mask_before_hide() {
    send_mask_down();
    for _ in 0..10_000 {
        if is_key_down(VK_MENU_MASK as u32) {
            break;
        }
        std::hint::spin_loop();
    }
}

/// Release mask after hide completes.
pub(crate) fn release_mask_after_hide() {
    send_mask_up();
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

    // --- Win key-up: release held mask but DO NOT eat the message ---
    // Key-up never triggers Start, and eating it confuses the system's
    // key-state tracking (GetAsyncKeyState stays TRUE → Win+E/D/R fire).
    if ctx.target_is_win && is_keyup && !injected {
        let consumed_vk = CONSUMED_WIN_VK.load(Ordering::SeqCst);
        if consumed_vk != 0 && vk == consumed_vk {
            CONSUMED_WIN_VK.store(0, Ordering::SeqCst);
            send_mask_up();
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
                // Hold mask key down for entire Win press.
                crate::runtime::log_info(&format!(
                    "[nex::debug] Hook: consuming Win key-down vk={}",
                    vk,
                ));
                CONSUMED_WIN_VK.store(vk, Ordering::SeqCst);
                SUPPRESS_FOCUS_ESCAPE.store(true, Ordering::SeqCst);
                send_mask_down();
                let _ = ctx.sender.send(OverlayEvent::Hotkey(ctx.hotkey_id));
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
    // Helper mode fields (when elevated helper is used instead of in-process hook)
    is_helper: bool,
    helper_process_handle: Option<isize>,
    pipe_reader_thread: Option<thread::JoinHandle<()>>,
}

impl HotkeyListener {
    pub(crate) fn start(hotkey_str: &str, event_tx: Sender<OverlayEvent>) -> Result<Self, String> {
        let parsed = parse_hotkey(hotkey_str)
            .map_err(|e| format!("invalid hotkey '{hotkey_str}': {e}"))?;
        let required_mods = modifier_vks_from_names(&parsed.modifiers)?;
        let target_key = vk_from_key(&parsed.key)?;
        let target_is_win = target_key == VK_LWIN;

        let should_exit = Arc::new(AtomicBool::new(false));
        let thread_id: Arc<OnceLock<u32>> = Arc::new(OnceLock::new());

        let _ = HOOK_CTX.set(HookContext { sender: event_tx.clone(), hotkey_id: 1, target_key, target_is_win, required_mods: required_mods.clone() });

        // --- Try elevated helper first (handles Task Manager / elevated windows) ---
        let helper_result = spawn_and_connect_helper(
            target_key, target_is_win, &required_mods, hotkey_str,
        );

        match helper_result {
            Ok((helper_handle, pipe_file)) => {
                logging::info("[nex] hotkey: using elevated helper for detection");
                let pipe_event_tx = event_tx.clone();
                let pipe_reader_thread = thread::Builder::new()
                    .name("nex-helper-pipe-reader".into())
                    .spawn(move || {
                        use std::io::BufRead;
                        let reader = std::io::BufReader::new(pipe_file);
                        for line in reader.lines() {
                            match line {
                                Ok(l) if l == "HOTKEY" => {
                                    let _ = pipe_event_tx.send(OverlayEvent::Hotkey(1));
                                }
                                Ok(l) if l == "SUPPRESS_ON" => {
                                    crate::overlay::hotkey::set_suppress_focus_escape(true);
                                }
                                Ok(l) if l == "SUPPRESS_OFF" => {
                                    crate::overlay::hotkey::set_suppress_focus_escape(false);
                                }
                                Ok(_) => {} // ignore other lines
                                Err(_) => break, // pipe disconnected
                            }
                        }
                    })
                    .map_err(|e| format!("failed to spawn pipe reader: {e}"))?;
                return Ok(Self { inner: Some(HotkeyListenerInner {
                    should_exit,
                    thread: None,
                    thread_id,
                    is_helper: true,
                    helper_process_handle: Some(helper_handle),
                    pipe_reader_thread: Some(pipe_reader_thread),
                })});
            }
            Err(e) => {
                logging::warn(&format!("[nex] helper elevation failed: {e}"));
                logging::info("[nex] falling back to in-process hook thread");
            }
        }

        // --- Fallback: in-process hook thread (existing code path) ---
        let should_exit_for_thread = should_exit.clone();
        let thread_id_for_thread = thread_id.clone();

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        // RegisterHotKeyW / UnregisterHotKey / LoadLibraryA / GetProcAddress
        // are not directly available in windows-sys 0.61.2's API-sets layout.
        // Load via raw extern "system" + user32 import.
        const WM_HOTKEY: u32 = 0x0312u32;
        const MOD_ALT: u32 = 0x0001;
        const MOD_CONTROL: u32 = 0x0002;
        const MOD_SHIFT: u32 = 0x0004;
        const MOD_WIN: u32 = 0x0008;
        type RegisterHotKeyFn = unsafe extern "system" fn(
            hWnd: *mut core::ffi::c_void, id: i32, fsModifiers: u32, vk: u32,
        ) -> i32;
        type UnregisterHotKeyFn = unsafe extern "system" fn(
            hWnd: *mut core::ffi::c_void, id: i32,
        ) -> i32;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryA(lpLibFileName: *const u8) -> *mut core::ffi::c_void;
            fn GetProcAddress(hModule: *mut core::ffi::c_void, lpProcName: *const u8) -> *mut core::ffi::c_void;
            fn GetModuleHandleA(lpModuleName: *const u8) -> *mut core::ffi::c_void;
        }
        let register_hotkey: Option<RegisterHotKeyFn> = unsafe {
            let lib = LoadLibraryA("user32.dll\0".as_ptr());
            if lib.is_null() { None } else {
                let ptr = GetProcAddress(lib, "RegisterHotKeyW\0".as_ptr());
                if ptr.is_null() { None } else {
                    Some(std::mem::transmute::<_, RegisterHotKeyFn>(ptr))
                }
            }
        };
        let unregister_hotkey: Option<UnregisterHotKeyFn> = unsafe {
            let lib = GetModuleHandleA("user32.dll\0".as_ptr());
            if lib.is_null() { None } else {
                let ptr = GetProcAddress(lib, "UnregisterHotKey\0".as_ptr());
                if ptr.is_null() { None } else {
                    Some(std::mem::transmute::<_, UnregisterHotKeyFn>(ptr))
                }
            }
        };

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

                // Register system-level hotkey fallback via RegisterHotKeyW.
                // Unlike WH_KEYBOARD_LL + WM_INPUT, RegisterHotKeyW delivers
                // WM_HOTKEY to this thread's message queue regardless of
                // foreground window or integrity level.  This ensures the
                // hotkey works even when elevated/UWP windows (Task Manager,
                // advanced settings) are foreground.
                // Only registered for non-Win hotkeys (Win key cannot be
                // registered via RegisterHotKeyW).
                let mut fallback_id: u32 = 0;
                let ctx = HOOK_CTX.get().unwrap();
                if !ctx.target_is_win {
                    let mod_map: &[(u32, u32)] = &[
                        (0x11, 0x0002), // VK_CTRL  → MOD_CONTROL
                        (0x12, 0x0001), // VK_ALT   → MOD_ALT
                        (0x10, 0x0004), // VK_SHIFT → MOD_SHIFT
                        (0x5B, 0x0008), // VK_LWIN  → MOD_WIN
                        (0x5C, 0x0008), // VK_RWIN  → MOD_WIN
                    ];
                    let mut mods: u32 = 0;
                    for &(vk, flag) in mod_map {
                        if ctx.required_mods.contains(&vk) { mods |= flag; }
                    }
                    // Generate a unique hotkey ID that won't collide.
                    fallback_id = (tid as u32).wrapping_mul(7) ^ 0x4E45;
                    let ok = match register_hotkey {
                        Some(f) => unsafe { f(std::ptr::null_mut(), fallback_id as i32, mods, ctx.target_key as u32) },
                        None => { logging::warn("[nex::debug] Hook: RegisterHotKeyW not available (fallback disabled)"); 0 },
                    };
                    if ok == 0 {
                        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                        logging::warn(&format!(
                            "[nex::debug] Hook: RegisterHotKeyW failed err={} (fallback disabled)",
                            err,
                        ));
                        fallback_id = 0;
                    } else {
                        logging::info("[nex::debug] Hook: RegisterHotKeyW fallback active");
                    }
                }

                let mut msg: MSG = unsafe { std::mem::zeroed() };
                let mut msg_count: u64 = 0;
                while !should_exit_for_thread.load(Ordering::SeqCst) {
                    let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
                    if status == 0 {
                        logging::info("[nex::debug] Hook: got WM_QUIT, exiting");
                        break;
                    }
                    if status == -1 {
                        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                        logging::warn(&format!("[nex::debug] Hook: GetMessageW failed err={}, exiting", err));
                        break;
                    }
                    // Check for WM_HOTKEY before dispatching (WM_HOTKEY has
                    // no window proc — it's posted to the thread message queue).
                    if msg.message == WM_HOTKEY && fallback_id != 0 {
                        let hk_id = msg.wParam as u32;
                        if hk_id == fallback_id {
                            logging::info("[nex::debug] Hook: WM_HOTKEY fallback fired, sending toggle");
                            let Some(ref ctx) = HOOK_CTX.get() else { continue; };
                            let _ = ctx.sender.send(OverlayEvent::Hotkey(ctx.hotkey_id));
                        }
                    }
                    unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
                    msg_count += 1;
                    if msg_count % 500 == 0 {
                        logging::info(&format!("[nex::debug] Hook: heartbeat {} msgs processed", msg_count));
                    }
                }
                if fallback_id != 0 {
                    if let Some(f) = unregister_hotkey {
                        unsafe { f(std::ptr::null_mut(), fallback_id as i32); }
                    }
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
        Ok(Self { inner: Some(HotkeyListenerInner {
            should_exit,
            thread: Some(thread),
            thread_id,
            is_helper: false,
            helper_process_handle: None,
            pipe_reader_thread: None,
        }) })
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
            Some(inner) => {
                if inner.is_helper {
                    // Helper mode: check pipe reader thread is alive
                    !inner.should_exit.load(Ordering::SeqCst)
                        && inner.pipe_reader_thread.as_ref().is_some_and(|t| !t.is_finished())
                } else {
                    // Hook mode: check hook thread is alive
                    !inner.should_exit.load(Ordering::SeqCst)
                        && inner.thread.as_ref().is_some_and(|t| !t.is_finished())
                }
            }
            None => false,
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.should_exit.store(true, Ordering::SeqCst);
            if inner.is_helper {
                // Helper mode: terminate helper process (if directly spawned),
                // then join pipe reader thread.
                if let Some(handle) = inner.helper_process_handle.take() {
                    if handle != 0 {
                        logging::info("[nex] shutdown: terminating helper process");
                        unsafe {
                            let h = handle as *mut core::ffi::c_void;
                            windows_sys::Win32::System::Threading::TerminateProcess(h, 1);
                            windows_sys::Win32::Foundation::CloseHandle(h);
                        }
                    }
                }
                if let Some(handle) = inner.pipe_reader_thread.take() {
                    logging::info("[nex] shutdown: joining pipe reader thread");
                    let _ = handle.join();
                }
            } else {
                // Hook mode: post WM_QUIT, join hook thread
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
}

fn post_quit_to_thread(thread_id: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
    unsafe { let _ = PostThreadMessageW(thread_id, WM_QUIT, 0, 0); }
}

// ---------------------------------------------------------------------------
// Overlay-ready event (nex → helper signal for SetForegroundWindow)
// ---------------------------------------------------------------------------

/// Handle to the named event used to signal the elevated helper.  nex.exe
/// creates this event (Medium IL mandatory label), then the helper opens
/// it for SYNCHRONIZE (read-only wait).  After nex.exe shows the overlay
/// it calls SetEvent; the helper wakes and calls SetForegroundWindow from
/// High IL, bypassing UIPI.
///
/// nex.exe creates the event so that the mandatory label stays at Medium
/// IL.  If the helper created it (High IL), nex.exe couldn't open it with
/// EVENT_MODIFY_STATE due to UIPI write-up restriction.
static OVERLAY_READY_EVENT: std::sync::Mutex<isize> = std::sync::Mutex::new(0);

/// Create the overlay-ready named event.  Called before spawning the
/// helper so the event exists when the helper tries to open it.
fn create_overlay_event() -> String {
    use windows_sys::Win32::System::Threading::CreateEventW;
    let pid = std::process::id();
    let name = format!("Global\\nex-overlay-ready-{pid}");
    let wide = to_wide(&name);
    // bManualReset=0 (auto-reset), bInitialState=0 (not signaled).
    // Auto-reset means MsgWaitForMultipleObjects in the helper will
    // automatically reset the event after WAIT_OBJECT_0, so the helper
    // only needs SYNCHRONIZE access (no EVENT_MODIFY_STATE needed).
    let handle = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, wide.as_ptr()) };
    if let Ok(mut guard) = OVERLAY_READY_EVENT.lock() {
        *guard = handle as isize;
    }
    name
}

/// Signal the elevated helper that the overlay is now visible.  Called
/// from host.rs's Painted handler after force_foreground().
pub(crate) fn signal_overlay_ready() {
    use windows_sys::Win32::System::Threading::SetEvent;
    let handle = match OVERLAY_READY_EVENT.lock() {
        Ok(g) => *g,
        _ => 0,
    };
    if handle != 0 {
        unsafe { SetEvent(handle as *mut core::ffi::c_void); }
    }
}

// ---------------------------------------------------------------------------
// Elevated helper (scheduled task + JSON config, no UAC prompt)
// ---------------------------------------------------------------------------

/// Well-known pipe name (PID-independent, single nex instance).
const HELPER_PIPE_NAME: &str = r"\\.\pipe\nex-hotkey";

/// Spawn the elevated helper via scheduled task and connect to its pipe.
/// Returns (process_handle=0, pipe_file) — task-spawned helpers can't be
/// terminated by nex (helper exits on pipe break automatically).
fn spawn_and_connect_helper(
    target_key: u32,
    target_is_win: bool,
    required_mods: &[u32],
    hotkey_str: &str,
) -> Result<(isize, std::fs::File), String> {
    // 1. Create the overlay-ready event BEFORE spawning the helper so that
    //    the mandatory label stays at Medium IL (nex.exe's integrity level).
    //    If the helper created it (High IL), nex.exe couldn't open it with
    //    EVENT_MODIFY_STATE due to UIPI write-up restriction.
    let event_name = create_overlay_event();

    // 2. Write JSON config for the helper (includes event name)
    let config_path = helper_config_path();
    write_helper_config(&config_path, target_key, target_is_win, required_mods, hotkey_str, &event_name)?;

    // 3. Ensure scheduled task exists (one-time UAC if not yet created)
    let helper_path = find_helper_exe()?;
    ensure_helper_task(&helper_path, &config_path)?;

    // 4. Run the scheduled task (no UAC)
    run_helper_task()?;

    // 5. Connect to the helper's named pipe (retry — helper may still be starting)
    let pipe_file = connect_pipe(HELPER_PIPE_NAME)?;

    // Process handle = 0 (scheduled task, not directly manageable)
    Ok((0, pipe_file))
}

/// Locate nex-helper.exe beside nex.exe.
fn find_helper_exe() -> Result<std::path::PathBuf, String> {
    let exe_path = std::env::current_exe()
        .map_err(|e| format!("can't get exe path: {e}"))?;
    let helper_path = exe_path.parent()
        .unwrap_or(&exe_path)
        .join("nex-helper.exe");
    if !helper_path.exists() {
        return Err(format!("helper not found: {}", helper_path.display()));
    }
    Ok(helper_path)
}

/// Path to the helper config JSON file in `%APPDATA%\Nex\`.
fn helper_config_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            // fallback
            let home = std::env::var("USERPROFILE")
                .unwrap_or_else(|_| "C:\\Users\\Default".into());
            std::path::PathBuf::from(home).join("AppData").join("Roaming")
        });
    base.join("Nex").join("helper-config.json")
}

/// Write JSON config file that the helper reads at startup.
fn write_helper_config(
    path: &std::path::Path,
    target_key: u32,
    target_is_win: bool,
    required_mods: &[u32],
    hotkey_str: &str,
    overlay_event_name: &str,
) -> Result<(), String> {
    let pid = std::process::id();
    let mod_ctrl = required_mods.contains(&0x11);
    let mod_alt = required_mods.contains(&0x12);
    let mod_shift = required_mods.contains(&0x10);
    let mod_win = required_mods.contains(&0x5B) || required_mods.contains(&0x5C);

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let json = format!(
        r#"{{"pipe":"{}","target_pid":{},"target_vk":{},"target_is_win":{},"mod_ctrl":{},"mod_alt":{},"mod_shift":{},"mod_win":{},"hotkey":"{}","event":"{}"}}"#,
        HELPER_PIPE_NAME,
        pid,
        target_key,
        if target_is_win { "true" } else { "false" },
        if mod_ctrl { "true" } else { "false" },
        if mod_alt { "true" } else { "false" },
        if mod_shift { "true" } else { "false" },
        if mod_win { "true" } else { "false" },
        hotkey_str,
        overlay_event_name,
    );

    std::fs::write(path, &json)
        .map_err(|e| format!("failed to write helper config '{:?}': {e}", path))
}

const SCHTASK_NAME: &str = "NexHelperV2";

/// Run `schtasks` with `runas` verb (elevated), wait for completion,
/// return error if exit code != 0.
fn run_schtasks_elevated(args: &str) -> Result<(), String> {
    let verb = to_wide("runas");
    let file = to_wide("schtasks");
    let params = to_wide(args);

    #[repr(C)]
    struct SHELLEXECUTEINFOW {
        cb_size: u32, f_mask: u32, hwnd: *mut core::ffi::c_void,
        lpVerb: *const u16, lpFile: *const u16, lpParameters: *const u16,
        lpDirectory: *const u16, nShow: i32, hInstApp: *mut core::ffi::c_void,
        lpIDList: *mut core::ffi::c_void, lpClass: *const u16,
        hkeyClass: *mut core::ffi::c_void, dwHotKey: u32,
        hIcon: *mut core::ffi::c_void, hProcess: *mut core::ffi::c_void,
    }
    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x00000040;
    const SW_HIDE: i32 = 0;

    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cb_size = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.f_mask = SEE_MASK_NOCLOSEPROCESS;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = params.as_ptr();
    sei.nShow = SW_HIDE;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetExitCodeProcess(hProcess: *mut core::ffi::c_void, lpExitCode: *mut u32) -> i32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteExW(lpExecInfo: *const SHELLEXECUTEINFOW) -> i32;
    }

    let ok = unsafe { ShellExecuteExW(&sei) };
    if ok == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(format!("ShellExecuteExW failed: getlasterror={err}"));
    }

    let handle = sei.hProcess as isize;
    if handle == 0 || handle == -1isize {
        return Err("ShellExecuteExW returned invalid process handle".into());
    }

    // Wait for schtasks.exe to finish
    let h = handle as *mut core::ffi::c_void;
    unsafe {
        windows_sys::Win32::System::Threading::WaitForSingleObject(h, 30_000);
    }

    // Check exit code — schtasks can start (ShellExecuteExW succeeds) but
    // still fail (e.g. invalid arguments).  Without this check, a failed
    // /create is treated as success and the task silently doesn't exist.
    let mut exit_code: u32 = 0;
    let ec_ok = unsafe { GetExitCodeProcess(h, &mut exit_code) };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h); }
    if ec_ok == 0 {
        return Err("GetExitCodeProcess failed".into());
    }
    if exit_code != 0 {
        return Err(format!("schtasks exited with code {exit_code}"));
    }
    Ok(())
}

/// Create the scheduled task if it doesn't exist (one-time UAC prompt).
fn ensure_helper_task(helper_path: &std::path::Path, config_path: &std::path::Path) -> Result<(), String> {
    // Check if task already exists
    let query = std::process::Command::new("schtasks")
        .args(["/query", "/tn", SCHTASK_NAME])
        .output()
        .map_err(|e| format!("schtasks /query failed: {e}"))?;

    if query.status.success() {
        return Ok(()); // task exists, good
    }

    // Task doesn't exist — create it via XML (avoids /tr quoting hell with UAC).
    // XML separates <Command> and <Arguments> so no escaping issues.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Date>2000-01-01T00:00:00Z</Date>
    <Author>Nex</Author>
  </RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <StartBoundary>2000-01-01T00:00:00Z</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <Enabled>true</Enabled>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Hidden>false</Hidden>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>--config {}</Arguments>
    </Exec>
  </Actions>
</Task>"#,
        helper_path.display(),
        config_path.display(),
    );

    // Write XML to %APPDATA%\Nex\nex-task.xml (persistent, so user can inspect on failure)
    let appdata = std::env::var("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let xml_path = appdata.join("Nex").join("nex-task.xml");
    let _ = std::fs::create_dir_all(xml_path.parent().unwrap());

    // Task Scheduler XML parser expects UTF-16LE with BOM, not UTF-8.
    // "unable to switch the encoding" at (1,40) if encoding="UTF-8" in UTF-8 file.
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&xml_path)
            .map_err(|e| format!("failed to create task XML {:?}: {e}", xml_path))?;
        // Write UTF-16LE BOM
        f.write_all(&[0xFF, 0xFE])
            .map_err(|e| format!("failed to write XML BOM: {e}"))?;
        // Encode XML as UTF-16LE
        for code_unit in xml.encode_utf16() {
            f.write_all(&code_unit.to_le_bytes())
                .map_err(|e| format!("failed to write XML content: {e}"))?;
        }
    }

    logging::info(&format!("[nex] task XML written to {:?}", xml_path));
    logging::info("[nex] creating scheduled task NexHelperV2 (one-time UAC)");
    let result = run_schtasks_elevated(&format!(
        "/create /tn {} /xml \"{}\" /f",
        SCHTASK_NAME,
        xml_path.display(),
    ));

    // Keep XML file on failure for debugging
    if result.is_ok() {
        let _ = std::fs::remove_file(&xml_path);
    }

    result?;

    logging::info(&format!("[nex] scheduled task {} created successfully", SCHTASK_NAME));
    Ok(())
}

/// Run the scheduled task (no UAC).
fn run_helper_task() -> Result<(), String> {
    let output = std::process::Command::new("schtasks")
        .args(["/run", "/tn", SCHTASK_NAME])
        .output()
        .map_err(|e| format!("schtasks /run failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("schtasks /run failed: {stderr}"));
    }

    Ok(())
}

/// Connect to an existing named pipe (client side, read-only).
fn connect_pipe(pipe_name: &str) -> Result<std::fs::File, String> {
    let wide = to_wide(pipe_name);

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut core::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut core::ffi::c_void,
        ) -> *mut core::ffi::c_void;
    }

    const GENERIC_READ: u32 = 0x80000000;
    const FILE_SHARE_READ: u32 = 0x00000001;
    const FILE_SHARE_WRITE: u32 = 0x00000002;
    const OPEN_EXISTING: u32 = 3;

    for attempt in 0..40 {
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };

        if !handle.is_null() && handle as isize != -1 {
            // Wrap the raw HANDLE into a File for BufReader
            let file = unsafe {
                std::fs::File::from_raw_handle(handle as *mut _)
            };
            return Ok(file);
        }

        // Retry with backoff (helper may still be starting)
        thread::sleep(Duration::from_millis(150 * (attempt as u64 + 1)));
    }

    Err(format!("failed to connect to pipe '{pipe_name}' after retries"))
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
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
