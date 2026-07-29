//! Elevated helper binary for nex hotkey detection.
//!
//! Runs at High Integrity Level (requireAdministrator manifest) so
//! `WH_KEYBOARD_LL` fires even when elevated/UWP windows (Task Manager,
//! Settings) are foreground.  Forwards hotkey events to nex.exe via a
//! named pipe.
//!
//! Usage:
//!   nex-helper.exe --pipe \\.\pipe\nex-hotkey-<pid> --target-vk 0x20 --mod-ctrl --hotkey "Ctrl+Space"

#![cfg(target_os = "windows")]
#![windows_subsystem = "windows"]

use std::ffi::OsStr;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Static state shared with hook proc
// ---------------------------------------------------------------------------

static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static CONSUMED_WIN_VK: AtomicU32 = AtomicU32::new(0);
static WIN_KEY_RELEASED: AtomicBool = AtomicBool::new(false);

/// Custom message to wake GetMessageW after the hook proc sets HOTKEY_FIRED.
/// WM_APP (0x8000) + unique magic bytes to avoid collisions.
const WM_NEX_WAKE: u32 = 0x8000 + 0x4E45;

/// Cached thread ID of the helper's message-loop thread.  The hook proc
/// calls PostThreadMessageW with this to wake GetMessageW after setting
/// HOTKEY_FIRED, since RegisterHotKeyW cannot register bare-Win hotkeys.
static HELPER_THREAD_ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// Cached HWND of the nex overlay window ("NexOverlayWindowClass").
/// Found once via FindWindowW on first hotkey dispatch, then reused.
static OVERLAY_HWND: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

struct HotkeyConfig {
    pipe_path: String,
    #[allow(dead_code)]
    hotkey_desc: String,
    target_pid: u32,
    target_vk: u32,
    target_is_win: bool,
    mod_ctrl: bool,
    mod_alt: bool,
    mod_shift: bool,
    mod_win: bool,
}

static CFG: OnceLock<HotkeyConfig> = OnceLock::new();

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_MENU_MASK: u16 = 0xE8;

// ---------------------------------------------------------------------------
// Win32 raw imports (not all available via windows-sys feature gates)
// ---------------------------------------------------------------------------

type PipeHandle = *mut core::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;

    fn CreateNamedPipeW(
        lpName: *const u16,
        dwOpenMode: u32,
        dwPipeMode: u32,
        nMaxInstances: u32,
        nOutBufferSize: u32,
        nInBufferSize: u32,
        nDefaultTimeOut: u32,
        lpSecurityAttributes: *mut core::ffi::c_void,
    ) -> PipeHandle;

    fn ConnectNamedPipe(hNamedPipe: PipeHandle, lpOverlapped: *mut core::ffi::c_void) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    /// Grant nex.exe a one-time ability to call SetForegroundWindow even
    /// when an elevated (High IL) window like Task Manager has focus.
    /// Must be called from High IL (helper process) with nex.exe's PID.
    fn AllowSetForegroundWindow(dwProcessId: u32) -> i32;

    /// Post a message to the specified thread's message queue. Used to
    /// wake GetMessageW from the hook proc after HOTKEY_FIRED is set.
    fn PostThreadMessageW(idThread: u32, Msg: u32, wParam: usize, lParam: isize) -> i32;
}

const PIPE_ACCESS_OUTBOUND: u32 = 0x00000002;
const PIPE_TYPE_MESSAGE: u32 = 0x00000004;
const PIPE_WAIT: u32 = 0x00000000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

fn parse_args() -> Result<HotkeyConfig, String> {
    let args: Vec<String> = std::env::args().collect();

    let mut pipe_path = String::new();
    let mut hotkey_desc = String::from("unknown");
    let mut target_pid: u32 = 0;
    let mut target_vk: u32 = 0;
    let mut target_is_win = false;
    let mut mod_ctrl = false;
    let mut mod_alt = false;
    let mut mod_shift = false;
    let mut mod_win = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pipe" => {
                i += 1;
                pipe_path = args.get(i).ok_or("--pipe requires a value")?.clone();
            }
            "--hotkey" => {
                i += 1;
                hotkey_desc = args.get(i).ok_or("--hotkey requires a value")?.clone();
            }
            "--target-vk" => {
                i += 1;
                let val = args.get(i).ok_or("--target-vk requires a value")?;
                target_vk = u32::from_str_radix(
                    val.trim_start_matches("0x").trim_start_matches("0X"),
                    16,
                )
                .map_err(|e| format!("invalid --target-vk '{val}': {e}"))?;
            }
            "--target-is-win" => {
                target_is_win = true;
            }
            "--target-pid" => {
                i += 1;
                let val = args.get(i).ok_or("--target-pid requires a value")?;
                target_pid = val.parse::<u32>()
                    .map_err(|e| format!("invalid --target-pid '{val}': {e}"))?;
            }
            "--mod-ctrl" => mod_ctrl = true,
            "--mod-alt" => mod_alt = true,
            "--mod-shift" => mod_shift = true,
            "--mod-win" => mod_win = true,
            _ => {}
        }
        i += 1;
    }

    if pipe_path.is_empty() {
        return Err("--pipe is required".into());
    }
    if target_vk == 0 && !target_is_win {
        return Err("--target-vk is required (or use --target-is-win)".into());
    }
    if target_pid == 0 {
        return Err("--target-pid is required".into());
    }

    Ok(HotkeyConfig {
        pipe_path,
        hotkey_desc,
        target_pid,
        target_vk,
        target_is_win,
        mod_ctrl,
        mod_alt,
        mod_shift,
        mod_win,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn is_key_down(vk: u32) -> bool {
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk as i32) as u16
            & 0x8000
            != 0
    }
}

fn send_mask_down() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT { wVk: VK_MENU_MASK, wScan: 0, dwFlags: 0, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
}

fn send_mask_up() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT { wVk: VK_MENU_MASK, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
}

/// Write "HOTKEY\n" to the pipe. Returns Err on failure (client disconnected).
fn write_hotkey(pipe: &mut std::fs::File) -> Result<(), std::io::Error> {
    pipe.write_all(b"HOTKEY\n")?;
    pipe.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Hook proc
// ---------------------------------------------------------------------------

unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT};

    if n_code < 0 {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    }

    let msg = w_param as u32;
    let is_keydown = msg == 0x0100 || msg == 0x0104; // WM_KEYDOWN / WM_SYSKEYDOWN
    let is_keyup = msg == 0x0101 || msg == 0x0105;   // WM_KEYUP / WM_SYSKEYUP

    let Some(ctx) = CFG.get() else {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    };

    let kb = unsafe { *(l_param as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let injected = (kb.flags & 0x10) != 0;

    // Track modifier state via atomics — GetAsyncKeyState lies when
    // WebView has focus and consumed the modifier key-down.
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
            WIN_KEY_RELEASED.store(true, Ordering::SeqCst);
            send_mask_up();
        }
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    }

    // Only key-down triggers matter for hotkey dispatch.
    if !is_keydown {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    }

    let is_target = if ctx.target_is_win {
        vk == VK_LWIN || vk == VK_RWIN
    } else {
        vk == ctx.target_vk
    };

    if is_target && !injected && check_modifiers(ctx) {
        HOTKEY_FIRED.store(true, Ordering::SeqCst);

        // Wake the helper's message loop so it observes HOTKEY_FIRED and
        // sends the event over the pipe.  For non-Win hotkeys, the
        // RegisterHotKeyW fallback posts WM_HOTKEY which already wakes
        // GetMessageW — but posting unconditionally is harmless.
        if let Some(tid) = HELPER_THREAD_ID.get() {
            unsafe { PostThreadMessageW(*tid, WM_NEX_WAKE, 0, 0); }
        }

        if ctx.target_is_win {
            // Hold mask key down for entire Win press.
            CONSUMED_WIN_VK.store(vk, Ordering::SeqCst);
            send_mask_down();
            return 1;
        }
        // Non-Win hotkey: eat key-down (prevents key from reaching focused window).
        return 1;
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) }
}

static HOTKEY_FIRED: AtomicBool = AtomicBool::new(false);

fn check_modifiers(ctx: &HotkeyConfig) -> bool {
    if ctx.mod_ctrl
        && !CTRL_DOWN.load(Ordering::SeqCst)
        && !is_key_down(VK_LCONTROL)
        && !is_key_down(VK_RCONTROL)
    {
        return false;
    }
    if ctx.mod_alt
        && !ALT_DOWN.load(Ordering::SeqCst)
        && !is_key_down(VK_LMENU)
        && !is_key_down(VK_RMENU)
    {
        return false;
    }
    if ctx.mod_shift
        && !SHIFT_DOWN.load(Ordering::SeqCst)
        && !is_key_down(VK_LSHIFT)
        && !is_key_down(VK_RSHIFT)
    {
        return false;
    }
    if ctx.mod_win && !is_key_down(VK_LWIN) && !is_key_down(VK_RWIN) {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Named pipe (server side — helper creates, nex.exe connects)
// ---------------------------------------------------------------------------

fn create_named_pipe(pipe_path: &str) -> Result<std::fs::File, String> {
    let wide_path = to_wide(pipe_path);
    let handle = unsafe {
        CreateNamedPipeW(
            wide_path.as_ptr(),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_MESSAGE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            0,
            0,
            0,
            std::ptr::null_mut(),
        )
    };

    if handle.is_null() || handle as isize == -1 {
        return Err(format!(
            "CreateNamedPipeW failed for {pipe_path}: getlasterror={}",
            unsafe { windows_sys::Win32::Foundation::GetLastError() }
        ));
    }

    Ok(unsafe { std::fs::File::from_raw_handle(handle as *mut _) })
}

fn wait_for_client(pipe: &std::fs::File) -> Result<(), String> {
    let handle = pipe.as_raw_handle() as PipeHandle;
    // ConnectNamedPipe blocks until nex.exe connects via CreateFileW.
    let ok = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    // ERROR_PIPE_CONNECTED (535) means client connected before we called ConnectNamedPipe.
    // Both 0 (success) and ERROR_PIPE_CONNECTED are acceptable.
    if ok != 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if err != 535 {
            return Err(format!("ConnectNamedPipe failed: err={err}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nex-helper: {e}");
            std::process::exit(1);
        }
    };

    let pipe_path = cfg.pipe_path.clone();
    let _ = CFG.set(cfg);

    // Create the named pipe
    let mut pipe = match create_named_pipe(&pipe_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("nex-helper: {e}");
            std::process::exit(1);
        }
    };

    // Wait for nex.exe to connect
    if let Err(e) = wait_for_client(&pipe) {
        eprintln!("nex-helper: {e}");
        std::process::exit(1);
    }

    // Install WH_KEYBOARD_LL hook
    let hook_id = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowsHookExW(
            windows_sys::Win32::UI::WindowsAndMessaging::WH_KEYBOARD_LL as i32,
            Some(keyboard_hook_proc),
            std::ptr::null_mut(),
            0,
        )
    };

    if hook_id.is_null() {
        eprintln!("nex-helper: SetWindowsHookExW failed");
        std::process::exit(1);
    }

    // Register system-level hotkey fallback via RegisterHotKeyW (non-Win hotkeys only).
    let ctx = CFG.get().unwrap();
    let mut fallback_id: i32 = 0;
    if !ctx.target_is_win {
        let mut mods: u32 = 0;
        if ctx.mod_ctrl {
            mods |= 0x0002; // MOD_CONTROL
        }
        if ctx.mod_alt {
            mods |= 0x0001; // MOD_ALT
        }
        if ctx.mod_shift {
            mods |= 0x0004; // MOD_SHIFT
        }
        if ctx.mod_win {
            mods |= 0x0008; // MOD_WIN
        }
        fallback_id = 0x4E46; // unique, different from nex.exe's ID

        let ok = unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::RegisterHotKey(
                std::ptr::null_mut(),
                fallback_id,
                mods,
                ctx.target_vk,
            )
        };
        if ok == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            eprintln!("nex-helper: RegisterHotKeyW failed err={err} (fallback disabled)");
            fallback_id = 0;
        }
    }

    // Cache this thread's ID so the hook proc can wake GetMessageW.
    let _ = HELPER_THREAD_ID.set(unsafe { GetCurrentThreadId() });

    // Message loop
    const WM_HOTKEY: u32 = 0x0312;
    let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = unsafe { std::mem::zeroed() };

    loop {
        let status = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW(
                &mut msg,
                std::ptr::null_mut(),
                0,
                0,
            )
        };

        if status == 0 {
            // WM_QUIT
            break;
        }
        if status == -1 {
            // Error
            break;
        }

        // Wake-up messages are posted by hook proc to unblock GetMessageW
        // after setting HOTKEY_FIRED.  Let them fall through so the loop
        // body processes HOTKEY_FIRED below — do NOT `continue` here.

        // Grant nex.exe foreground permission before every HOTKEY dispatch,
        // so SetForegroundWindow succeeds even when Task Manager (High IL)
        // is the foreground window.
        let will_send_hotkey =
            (msg.message == WM_HOTKEY && fallback_id != 0 && msg.wParam as i32 == fallback_id)
            || HOTKEY_FIRED.load(Ordering::SeqCst);

        if will_send_hotkey {
            if let Some(cfg) = CFG.get() {
                // AllowSetForegroundWindow is called from High IL (helper).
                // nex.exe (Medium IL) can then call SetForegroundWindow once.
                unsafe { AllowSetForegroundWindow(cfg.target_pid); }

                // Directly SetForegroundWindow on the overlay from High IL.
                // This bypasses UIPI entirely — FindWindowW locates the
                // overlay by its registered class name.  The HWND is
                // cached after first lookup.
                let overlay_hwnd = OVERLAY_HWND.get_or_init(|| {
                    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
                    let class: Vec<u16> = "NexOverlayWindowClass\0".encode_utf16().collect();
                    unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) as isize }
                });
                if *overlay_hwnd != 0 {
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                            *overlay_hwnd as *mut core::ffi::c_void,
                        );
                    }
                }
            }
        }

        // Check for WM_HOTKEY (RegisterHotKeyW fallback)
        if msg.message == WM_HOTKEY && fallback_id != 0 && msg.wParam as i32 == fallback_id {
            if write_hotkey(&mut pipe).is_err() {
                break; // nex.exe disconnected
            }
        }

        // Check if hook proc fired
        if HOTKEY_FIRED.swap(false, Ordering::SeqCst) {
            if let Some(cfg) = CFG.get() {
                if cfg.target_is_win && CONSUMED_WIN_VK.load(Ordering::SeqCst) != 0 {
                    if pipe.write_all(b"SUPPRESS_ON\n").is_err() {
                        break; // nex.exe disconnected
                    }
                }
            }
            if write_hotkey(&mut pipe).is_err() {
                break; // nex.exe disconnected
            }
        }

        // Check if Win key was released (send SUPPRESS_OFF to nex.exe)
        if WIN_KEY_RELEASED.swap(false, Ordering::SeqCst) {
            if pipe.write_all(b"SUPPRESS_OFF\n").is_err() {
                break; // nex.exe disconnected
            }
        }

        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }

    // Cleanup
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook_id);
    }
    if fallback_id != 0 {
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(
                std::ptr::null_mut(),
                fallback_id,
            );
        }
    }
}
