//! Elevated helper binary for nex hotkey detection.
//!
//! Runs at High Integrity Level (requireAdministrator manifest) so
//! `WH_KEYBOARD_LL` fires even when elevated/UWP windows (Task Manager,
//! Settings) are foreground.  Forwards hotkey events to nex.exe via a
//! named pipe.
//!
//! Usage:
//!   NexHelper.exe --config "%APPDATA%\Nex\helper-config.json"
//!   NexHelper.exe --pipe \\.\pipe\nex-hotkey-<pid> --target-vk 0x20 --mod-ctrl --hotkey "Ctrl+Space"

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

// (SetForegroundWindow from the helper doesn't work because the overlay
// window is created with .with_visible(false) — hidden windows cannot be
// made foreground.  Instead, AllowSetForegroundWindow (above) grants
// nex.exe permission to call SetForegroundWindow itself after showing.)

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
    #[allow(dead_code)]
    event_name: String,
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

    fn CloseHandle(hObject: PipeHandle) -> i32;
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

    /// Find the overlay window by class name (High IL, no UIPI issue).
    fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> PipeHandle;

}

const PIPE_ACCESS_OUTBOUND: u32 = 0x00000002;
const PIPE_TYPE_MESSAGE: u32 = 0x00000004;
const PIPE_WAIT: u32 = 0x00000000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;

// ---------------------------------------------------------------------------
// Config source: CLI args or JSON config file
// ---------------------------------------------------------------------------

/// Parse config from either `--config <file>` (JSON file) or individual CLI args.
fn parse_args() -> Result<HotkeyConfig, String> {
    let args: Vec<String> = std::env::args().collect();

    // Check for --config first
    if let Some(pos) = args.iter().position(|a| a == "--config") {
        if let Some(path) = args.get(pos + 1) {
            return parse_config_file(path);
        }
    }

    // Fall back to individual CLI args
    parse_args_cli(&args)
}

fn parse_config_file(path: &str) -> Result<HotkeyConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config '{path}': {e}"))?;

    let pipe_path = json_str(&content, "pipe")?.to_string();
    let hotkey_desc = json_str(&content, "hotkey").unwrap_or("unknown").to_string();
    let target_pid = json_u32(&content, "target_pid")?;
    let target_vk = json_u32(&content, "target_vk")?;
    let target_is_win = json_bool(&content, "target_is_win");
    let mod_ctrl = json_bool(&content, "mod_ctrl");
    let mod_alt = json_bool(&content, "mod_alt");
    let mod_shift = json_bool(&content, "mod_shift");
    let mod_win = json_bool(&content, "mod_win");
    let event_name = json_str(&content, "event").unwrap_or("").to_string();

    if pipe_path.is_empty() {
        return Err("config: 'pipe' is required".into());
    }
    if target_vk == 0 && !target_is_win {
        return Err("config: 'target_vk' required (or target_is_win)".into());
    }
    if target_pid == 0 {
        return Err("config: 'target_pid' is required".into());
    }
    if event_name.is_empty() {
        return Err("config: 'event' is required".into());
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
        event_name,
    })
}

/// Parse config from individual `--pipe`, `--target-vk`, etc CLI args.
fn parse_args_cli(args: &[String]) -> Result<HotkeyConfig, String> {
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
        event_name: String::new(),
    })
}

/// Simple manual JSON helpers (no serde dependency needed).
/// Expected format: `"key": value` where strings are `"..."` and numbers are bare.
fn json_str<'a>(s: &'a str, key: &str) -> Result<&'a str, String> {
    let pattern = format!("\"{key}\":");
    let Some(start) = s.find(&pattern) else {
        return Err(format!("config: key '{key}' not found"));
    };
    let after = &s[start + pattern.len()..];
    let trimmed = after.trim_start();
    if !trimmed.starts_with('"') {
        return Err(format!("config: key '{key}' value is not a string"));
    }
    let inner = &trimmed[1..];
    let end = inner.find('"').ok_or(format!("config: key '{key}' unterminated string"))?;
    Ok(&inner[..end])
}

fn json_u32(s: &str, key: &str) -> Result<u32, String> {
    let pattern = format!("\"{key}\":");
    let Some(start) = s.find(&pattern) else {
        return Err(format!("config: key '{key}' not found"));
    };
    let after = s[start + pattern.len()..].trim_start();
    let end = after.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(after.len());
    after[..end].parse::<u32>()
        .map_err(|e| format!("config: key '{key}' invalid u32: {e}"))
}

fn json_bool(s: &str, key: &str) -> bool {
    let pattern = format!("\"{key}\":");
    s.find(&pattern)
        .and_then(|start| {
            let after = s[start + pattern.len()..].trim_start().to_lowercase();
            if after.starts_with("true") { Some(true) } else { None }
        })
        .unwrap_or(false)
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
    // ConnectNamedPipe returns nonzero on success (client connected).
    // Returns 0 with ERROR_PIPE_CONNECTED (535) if client connected before we called it.
    // GetLastError is STALE on success (nonzero) — do NOT check it there.
    if ok == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if err != 535 {
            return Err(format!("ConnectNamedPipe failed: err={err}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Overlay foreground helper (called from message loop on event signal)
// ---------------------------------------------------------------------------

/// Write a diagnostic line to the helper debug log.
/// The helper has no console (windows_subsystem = "windows"), so file logging
/// is the only way to observe failures.
fn debug_log(msg: &str) {
    let path = std::env::temp_dir().join("NexHelper-debug.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {}", std::process::id(), msg);
        let _ = f.flush();
    }
}

/// Called when nex.exe signals that the overlay is now visible.
/// Uses High IL to bring the overlay to front, bypassing UIPI.
///
/// Strategy (from most reliable to fallback):
/// 1. Attach to the current foreground thread (same High IL, no UIPI),
///    then call SetForegroundWindow so the system grants the call.
/// 2. Also grant nex.exe foreground permission as a backup.
fn set_overlay_foreground() {
    let class = to_wide("NexOverlayWindowClass\0");
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if hwnd as isize == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        debug_log(&format!("FindWindowW failed: err={err}"));
        return;
    }
    debug_log(&format!("FindWindowW found hwnd={:x}", hwnd as isize));

    unsafe {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        };
        use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

        // 1. Grant nex.exe foreground permission (backup path)
        if let Some(cfg) = CFG.get() {
            let ret = AllowSetForegroundWindow(cfg.target_pid);
            debug_log(&format!("AllowSetForegroundWindow(pid={}): ret={ret}", cfg.target_pid));
        }

        // 2. Attach this thread to the foreground thread (both High IL) so
        //    SetForegroundWindow succeeds.  This mirrors nex.exe's
        //    force_foreground() trick but runs at High IL where
        //    AttachThreadInput is not blocked by UIPI.
        let fg = GetForegroundWindow();
        let helper_tid = GetCurrentThreadId();
        let fg_tid = if fg as isize == 0 {
            0
        } else {
            GetWindowThreadProcessId(fg, std::ptr::null_mut())
        };
        debug_log(&format!(
            "fg=0x{:x} helper_tid={} fg_tid={}",
            fg as isize, helper_tid, fg_tid,
        ));

        let should_attach = fg_tid != 0 && fg_tid != helper_tid;
        if should_attach {
            let ret = AttachThreadInput(helper_tid, fg_tid, 1);
            debug_log(&format!("AttachThreadInput(attach): ret={ret} err={}", GetLastError()));
        }

        let ret = SetForegroundWindow(hwnd);
        debug_log(&format!(
            "SetForegroundWindow: ret={ret} err={}",
            GetLastError(),
        ));

        if should_attach {
            let ret = AttachThreadInput(helper_tid, fg_tid, 0);
            debug_log(&format!("AttachThreadInput(detach): ret={ret} err={}", GetLastError()));
        }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    // Log startup immediately (before anything can fail)
    debug_log("helper started");

    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("NexHelper: {e}");
            std::process::exit(1);
        }
    };

    debug_log(&format!("config parsed: event='{}'", cfg.event_name));

    let pipe_path = cfg.pipe_path.clone();
    let _ = CFG.set(cfg);

    debug_log(&format!("creating named pipe: {pipe_path}"));

    // Create the named pipe
    let mut pipe = match create_named_pipe(&pipe_path) {
        Ok(p) => p,
        Err(e) => {
            debug_log(&format!("create_named_pipe failed: {e}"));
            eprintln!("NexHelper: {e}");
            std::process::exit(1);
        }
    };
    debug_log("pipe created, waiting for client");

    // Wait for nex.exe to connect
    if let Err(e) = wait_for_client(&pipe) {
        debug_log(&format!("wait_for_client failed: {e}"));
        eprintln!("NexHelper: {e}");
        std::process::exit(1);
    }
    debug_log("client connected");

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
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        debug_log(&format!("SetWindowsHookExW failed: err={err}"));
        eprintln!("NexHelper: SetWindowsHookExW failed");
        std::process::exit(1);
    }
    debug_log("SetWindowsHookExW succeeded");

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
            eprintln!("NexHelper: RegisterHotKeyW failed err={err} (fallback disabled)");
            fallback_id = 0;
        }
    }

    // Open the overlay-ready event created by nex.exe (Medium IL).
    // The event has a Medium IL mandatory label so nex.exe can call
    // SetEvent (write-level) without UIPI blocking.
    // The helper opens it with SYNCHRONIZE (read-level) for waiting.
    let overlay_ready_event = unsafe {
        OpenEventW(
            0x00100000, // SYNCHRONIZE — sufficient for MsgWaitForMultipleObjects
            0,          // bInheritHandle = false
            to_wide(&ctx.event_name).as_ptr(),
        )
    };
    if overlay_ready_event.is_null() || overlay_ready_event as isize == -1isize {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        debug_log(&format!("OpenEventW failed: event='{}' err={err}", ctx.event_name));
        eprintln!("NexHelper: OpenEventW failed (event='{}')", ctx.event_name);
        std::process::exit(1);
    }
    debug_log("OpenEventW succeeded");

    // Open a handle to nex.exe with SYNCHRONIZE so we are notified when it
    // exits.  This lets us exit cleanly when nex quits (no hanging until
    // the next keyboard event).
    let nex_handle = unsafe {
        windows_sys::Win32::System::Threading::OpenProcess(
            0x00100000,  // PROCESS_SYNCHRONIZE
            0,           // bInheritHandle = false
            ctx.target_pid,
        )
    };
    let nex_handle_valid = !nex_handle.is_null()
        && nex_handle as isize != -1isize
        && nex_handle as isize != 0;
    if nex_handle_valid {
        debug_log("OpenProcess(SYNCHRONIZE) succeeded");
    } else {
        debug_log(&format!(
            "OpenProcess(SYNCHRONIZE) failed: err={}",
            unsafe { windows_sys::Win32::Foundation::GetLastError() },
        ));
    }

    // Cache this thread's ID so the hook proc can wake GetMessageW.
    let _ = HELPER_THREAD_ID.set(unsafe { GetCurrentThreadId() });

    debug_log("helper entering message loop");

    // Message loop — use MsgWaitForMultipleObjects to wait on messages,
    // the overlay-ready event, and the nex process handle.
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_FAILED};
    use windows_sys::Win32::System::Threading::OpenEventW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MsgWaitForMultipleObjects, PeekMessageW, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };

    const WM_HOTKEY: u32 = 0x0312;
    let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = unsafe { std::mem::zeroed() };
    let handles = [
        overlay_ready_event,
        if nex_handle_valid { nex_handle } else { std::ptr::null_mut() },
    ];
    let n_handles: u32 = if nex_handle_valid { 2 } else { 1 };
    let wait_forever = 0xFFFFFFFFu32;

    loop {
        // Wait for either a message, the overlay-ready event, or nex exit
        let wait_result = unsafe {
            MsgWaitForMultipleObjects(
                n_handles,                         // number of handles to wait on
                handles.as_ptr(),                  // handle array
                0,                                 // fWaitAll = false (any will do)
                wait_forever,                      // dwMilliseconds = INFINITE
                QS_ALLINPUT,                       // wake for any message
            )
        };

        if wait_result == WAIT_FAILED {
            break;
        }

        if wait_result == WAIT_OBJECT_0 {
            // Overlay-ready event was signaled — nex.exe has shown the overlay,
            // now set foreground from High IL.
            // (Event is auto-reset, so it returns to nonsignaled automatically.)
            debug_log("overlay-ready event received, calling set_overlay_foreground");
            set_overlay_foreground();
            debug_log("set_overlay_foreground returned");
        }

        // WAIT_OBJECT_0 + 1 = nex process handle signaled (nex exited)
        if nex_handle_valid && wait_result == WAIT_OBJECT_0 + 1 {
            debug_log("nex process exited, shutting down");
            break;
        }

        // Process all pending messages (may be zero if only event woke us)
        let mut got_quit = false;
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_QUIT {
                got_quit = true;
                break;
            }
            // Grant nex.exe foreground permission before every HOTKEY dispatch.
            let will_send_hotkey =
                (msg.message == WM_HOTKEY && fallback_id != 0 && msg.wParam as i32 == fallback_id)
                || HOTKEY_FIRED.load(Ordering::SeqCst);

            if will_send_hotkey {
                if let Some(cfg) = CFG.get() {
                    unsafe { AllowSetForegroundWindow(cfg.target_pid); }
                }
            }

            // Process WM_HOTKEY (RegisterHotKeyW fallback)
            if msg.message == WM_HOTKEY && fallback_id != 0 && msg.wParam as i32 == fallback_id {
                if write_hotkey(&mut pipe).is_err() {
                    got_quit = true;
                    break; // nex.exe disconnected
                }
            }

            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            }
        }

        if got_quit {
            break;
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
    }

    // Cleanup
    unsafe {
        CloseHandle(overlay_ready_event);
        if nex_handle_valid {
            CloseHandle(nex_handle);
        }
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
