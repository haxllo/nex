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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

mod mft;
// Wire format shared with the indexer; the helper cannot depend on the core crate.
#[path = "../../core/src/mft_protocol.rs"]
mod mft_protocol;

// ---------------------------------------------------------------------------
// Static state shared with hook proc
// ---------------------------------------------------------------------------

static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static CONSUMED_WIN_VK: AtomicU32 = AtomicU32::new(0);
static CHORD_DETECTED: AtomicBool = AtomicBool::new(false);
static WIN_KEY_RELEASED: AtomicBool = AtomicBool::new(false);

/// Custom message to wake GetMessageW after the hook proc sets HOTKEY_FIRED.
/// WM_APP (0x8000) + unique magic bytes to avoid collisions.
const WM_NEX_WAKE: u32 = 0x8000 + 0x4E45;

/// Timer id used to release the Win menu mask after nex has shown the overlay.
const MASK_RELEASE_TIMER_ID: usize = 0x4E46;

/// How long the helpers holds the mask past a bare Win key-up. nex needs to
/// show the panel and take foreground in that window; releasing earlier lets
/// the shell begin opening Start, which steals focus from the overlay.
const MASK_RELEASE_DELAY_MS: u32 = 120;

/// Cached thread ID of the helper's message-loop thread.  The hook proc
/// calls PostThreadMessageW with this to wake GetMessageW after setting
/// HOTKEY_FIRED, since RegisterHotKeyW cannot register bare-Win hotkeys.
static HELPER_THREAD_ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

// (SetForegroundWindow from the helper doesn't work because the overlay
// window is created with .with_visible(false) — hidden windows cannot be
// made foreground.  Instead, AllowSetForegroundWindow (above) grants
// nex.exe permission to call SetForegroundWindow itself after showing.)
#[derive(Clone)]
struct HotkeyConfig {
    pipe_path: String,
    hotkey_desc: String,
    target_pid: u32,
    target_vk: u32,
    target_is_win: bool,
    mod_ctrl: bool,
    mod_alt: bool,
    mod_shift: bool,
    mod_win: bool,
    event_name: String,
    /// Name of nex's config-changed doorbell event (empty when unset).
    config_event_name: String,
    /// When false, this helper does NOT detect hotkeys — nex owns
    /// detection via raw input; the helper only assists foreground.
    detect_enabled: bool,
    /// Helper generation counter written by nex. A value newer than the one
    /// this process loaded means it has been superseded and must exit.
    generation: u64,
}

static CFG: std::sync::RwLock<Option<std::sync::Arc<HotkeyConfig>>> = std::sync::RwLock::new(None);

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

#[derive(Clone, Copy)]
struct SendHandle(isize);
// SAFETY: these are kernel event handles — designed to be signaled
// and waited on from any thread.
unsafe impl Send for SendHandle {}

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

    fn CreateEventW(
        lpEventAttributes: *mut core::ffi::c_void,
        bManualReset: i32,
        bInitialState: i32,
        lpName: *const u16,
    ) -> PipeHandle;
    fn SetEvent(hEvent: PipeHandle) -> i32;

    fn GetCurrentProcess() -> PipeHandle;

    fn ConnectNamedPipe(hNamedPipe: PipeHandle, lpOverlapped: *mut core::ffi::c_void) -> i32;

    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut core::ffi::c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: PipeHandle,
    ) -> PipeHandle;

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

const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;
const FILE_SHARE_READ: u32 = 0x00000001;
const FILE_SHARE_WRITE: u32 = 0x00000002;
const OPEN_EXISTING: u32 = 3;

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

    let pipe_path = json_str(&content, "pipe")?;
    let hotkey_desc = json_str(&content, "hotkey").unwrap_or_else(|_| String::from("unknown"));
    let target_pid = json_u32(&content, "target_pid")?;
    let target_vk = json_u32(&content, "target_vk")?;
    let target_is_win = json_bool(&content, "target_is_win");
    let mod_ctrl = json_bool(&content, "mod_ctrl");
    let mod_alt = json_bool(&content, "mod_alt");
    let mod_shift = json_bool(&content, "mod_shift");
    let mod_win = json_bool(&content, "mod_win");
    let event_name = json_str(&content, "event").unwrap_or_default();
    let config_event_name = json_str(&content, "config_event").unwrap_or_default();
    // nex owns hotkey detection — the helper's hook stays disabled unless
    // a legacy config explicitly opts in.
    let detect_enabled = json_bool(&content, "detect");
    let generation = json_u64(&content, "generation").unwrap_or(0);

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
        config_event_name,
        detect_enabled,
        generation,
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
        config_event_name: String::new(),
        detect_enabled: false,
        generation: 0,
    })
}

/// Simple manual JSON helpers (no serde dependency needed).
/// Expected format: `"key": value` where strings are `"..."` and numbers are bare.
fn json_str(s: &str, key: &str) -> Result<String, String> {
    let pattern = format!("\"{key}\":");
    let Some(start) = s.find(&pattern) else {
        return Err(format!("config: key '{key}' not found"));
    };
    let after = &s[start + pattern.len()..];
    let trimmed = after.trim_start();
    if !trimmed.starts_with('"') {
        return Err(format!("config: key '{key}' value is not a string"));
    }
    // nex writes this file as JSON, so a Windows path arrives with its
    // backslashes doubled. Unescape here: the pipe and event names are used
    // verbatim, and a still-escaped path is not a pipe name.
    let mut out = String::new();
    let mut chars = trimmed[1..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Ok(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16)
                        .map_err(|e| format!("config: key '{key}' bad unicode escape: {e}"))?;
                    out.push(
                        char::from_u32(code)
                            .ok_or_else(|| format!("config: key '{key}' invalid code point"))?,
                    );
                }
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    Err(format!("config: key '{key}' unterminated string"))
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

fn json_u64(s: &str, key: &str) -> Option<u64> {
    let pattern = format!("\"{key}\":");
    let start = s.find(&pattern)?;
    let after = s[start + pattern.len()..].trim_start();
    let end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    after[..end].parse::<u64>().ok()
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

/// Unassigned VK sent only to break a pending Start gesture — the same guard
/// as nex's `hotkey::VK_START_GUARD`. A key-up of a key that was never down
/// cannot type anything, but it cancels Start-menu-on-Win-key-up.
const VK_START_GUARD: u16 = 0xFF;

fn invalidate_start_gesture() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT { wVk: VK_START_GUARD, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
}

fn send_alt_tap() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut inputs: [INPUT; 2] = unsafe { std::mem::zeroed() };
    inputs[0].r#type = INPUT_KEYBOARD;
    inputs[0].Anonymous.ki = KEYBDINPUT { wVk: 0x12, wScan: 0, dwFlags: 0, time: 0, dwExtraInfo: 0 }; // Alt down
    inputs[1].r#type = INPUT_KEYBOARD;
    inputs[1].Anonymous.ki = KEYBDINPUT { wVk: 0x12, wScan: 0, dwFlags: KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 };
    let _ = unsafe { SendInput(2, &mut inputs as *mut _ as *mut INPUT, std::mem::size_of::<INPUT>() as i32) };
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

/// Whether this process currently holds the mask down. Centralizes the
/// pairing so a missed key-up path can't double-hold or leak the hold.
static MASK_HELD: AtomicBool = AtomicBool::new(false);

/// Set by the Win key-up path when the press toggled the overlay. The mask
/// must outlive the key-up so the shell never starts opening Start while nex
/// is still taking foreground; the loop thread releases it after the grace
/// window. Without this, releasing at key-up let Start open, which stole
/// focus, produced a phantom outside-click and hid the panel that had just
/// been shown.
static DEFER_MASK_RELEASE: AtomicBool = AtomicBool::new(false);

/// Hold the mask unless already held. Returns true on transition.
fn hold_mask() -> bool {
    if MASK_HELD.swap(true, Ordering::SeqCst) {
        false
    } else {
        send_mask_down();
        true
    }
}

/// Release the mask if held. Returns true on transition.
fn release_mask() -> bool {
    if MASK_HELD.swap(false, Ordering::SeqCst) {
        send_mask_up();
        true
    } else {
        false
    }
}

/// Write "HOTKEY\n" to the pipe. Returns Err on failure (client disconnected).
fn write_hotkey(pipe: &mut std::fs::File) -> Result<(), std::io::Error> {
    debug_log("firing HOTKEY to nex");
    pipe.write_all(b"HOTKEY\n")?;
    pipe.flush()?;
    Ok(())
}

/// Inbound command pipe (nex writes, helper executes). Same per-pid
/// scoping as the log pipe.

/// `\\.\pipe\nex-hotkey\<pid>` -> `\\.\pipe\NexHelperCmd\<pid>`.
fn cmd_pipe_for(pipe_path: &str) -> String {
    match pipe_path.rsplit_once('\\') {
        Some((_, pid)) if !pid.is_empty() => format!("\\\\.\\pipe\\NexHelperCmd\\{pid}"),
        _ => format!("{pipe_path}-cmd"),
    }
}

/// One synthetic key tap, optionally with Ctrl held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SimTap {
    vk: u32,
    ctrl: bool,
}

/// Allowlisted simulation commands. Media keys tap blind; edit chords
/// hold Ctrl. Anything else is rejected before touching input.
fn sim_action(cmd: &str) -> Option<SimTap> {
    let ctrl_tap = |vk: u32| SimTap { vk, ctrl: true };
    let tap = |vk: u32| SimTap { vk, ctrl: false };
    Some(match cmd {
        "play_pause" => tap(0xB3),
        "next" => tap(0xB0),
        "prev" => tap(0xB1),
        "stop" => tap(0xB2),
        "mute" => tap(0xAD),
        "vol_up" => tap(0xAF),
        "vol_down" => tap(0xAE),
        "copy" => ctrl_tap(0x43),
        "paste" => ctrl_tap(0x56),
        "cut" => ctrl_tap(0x58),
        "select_all" => ctrl_tap(0x41),
        "undo" => ctrl_tap(0x5A),
        _ => return None,
    })
}

fn exec_sim(action: SimTap) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut seq: Vec<INPUT> = Vec::with_capacity(4);
    let mut key = |vk: u32, up: bool| {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki = KEYBDINPUT {
            wVk: vk as u16,
            wScan: 0,
            dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        };
        seq.push(input);
    };
    if action.ctrl {
        key(0x11, false);
    }
    key(action.vk, false);
    key(action.vk, true);
    if action.ctrl {
        key(0x11, true);
    }
    let _ = unsafe {
        SendInput(
            seq.len() as u32,
            seq.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
}

/// Connect to the UI-owned command pipe and execute allowlisted lines.
///
/// The UI creates this pipe at Medium IL and owns the write end. UIPI blocks a
/// Medium-IL process from writing into a High-IL pipe, so the elevated helper
/// must be the client here; reading down is permitted.
fn spawn_cmd_pipe_client(cmd_path: String) {
    std::thread::Builder::new()
        .name("NexHelper-cmd-pipe".into())
        .spawn(move || {
            let wide = to_wide(&cmd_path);
            let mut pipe = None;
            // The UI opens its server end right after the detection pipe
            // handshake; retry briefly so a slow supervisor is not fatal.
            for _ in 0..40 {
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
                    pipe = Some(unsafe { std::fs::File::from_raw_handle(handle as *mut _) });
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }

            let Some(file) = pipe else {
                debug_log("cmd pipe connect failed");
                return;
            };
            debug_log("cmd pipe client connected");
            // Single buffered reader for the session: recreating it per
            // line would drop buffered bytes with it.
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(file);
            loop {
                let mut buf = String::new();
                match reader.read_line(&mut buf) {
                    Ok(0) | Err(_) => return, // client gone
                    Ok(_) => {}
                }
                let cmd = buf.trim().to_string();
                match sim_action(&cmd) {
                    Some(action) => {
                        debug_log(&format!("sim exec: {cmd}"));
                        exec_sim(action);
                    }
                    None => debug_log(&format!("sim rejected: {cmd}")),
                }
            }
        })
        .ok();
}

// ---------------------------------------------------------------------------
// Hook proc
// ---------------------------------------------------------------------------

/// Win key-up decision: only a press that stayed bare toggles. Kept pure so
/// the chord/auto-repeat rules are testable without an input hook.
fn win_press_fires(consumed_vk: u32, chord_detected: bool) -> bool {
    consumed_vk != 0 && !chord_detected
}

/// Report a low-level hook that took long enough to be felt as input lag.
///
/// Windows gives a low-level hook a few seconds to return before it stalls the
/// whole input queue, and unhooks a proc that keeps exceeding it. Samples
/// above a generous budget are logged so a regression is visible in the log.
fn note_hook_duration(kind: &str, elapsed: std::time::Duration) {
    if elapsed >= std::time::Duration::from_millis(30) {
        debug_log(&format!("[stall] {kind} hook proc took {}ms", elapsed.as_millis()));
    }
}

unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    let entered = std::time::Instant::now();
    let result = unsafe { keyboard_hook_proc_inner(n_code, w_param, l_param) };
    note_hook_duration("keyboard", entered.elapsed());
    result
}

unsafe fn keyboard_hook_proc_inner(
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

    // Win mask handling runs whenever the hotkey IS Win — even though
    // detection belongs to nex, holding the mask is what blocks Start.
    let target_is_win = HK_IS_WIN.load(Ordering::SeqCst);
    let target_vk = HK_VK.load(Ordering::SeqCst);
    let detect_enabled = HK_ENABLED.load(Ordering::SeqCst);

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

    // --- Win hotkey path (mask hold + chord passthrough) ---
    //
    // Runs whenever the hotkey is Win, REGARDLESS of detect_enabled:
    // holding the injected mask key while Win is physically down is what
    // blocks the Start menu — including over Task Manager, where this
    // High-IL hook still receives events. nex toggles via its own raw
    // input on Win key-up; chords (Win+E, Win+D…) pass through.
    if target_is_win && !injected {
        let is_win_vk = vk == VK_LWIN || vk == VK_RWIN;
        let is_mod = vk == VK_LCONTROL || vk == VK_RCONTROL
            || vk == VK_LMENU || vk == VK_RMENU
            || vk == VK_LSHIFT || vk == VK_RSHIFT;

        if is_win_vk {
            if is_keydown {
                if CONSUMED_WIN_VK.load(Ordering::SeqCst) == 0 {
                    CONSUMED_WIN_VK.store(vk, Ordering::SeqCst);
                    CHORD_DETECTED.store(false, Ordering::SeqCst);
                    hold_mask();
                }
                return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
            }
            if is_keyup {
                let consumed_vk = CONSUMED_WIN_VK.swap(0, Ordering::SeqCst);
                if consumed_vk != 0 {
                    let chord = CHORD_DETECTED.swap(false, Ordering::SeqCst);
                    // Chord keys must never toggle — Win+E opens Explorer
                    // and nothing else. CHORD_DETECTED is set by any real
                    // non-modifier key-down during the press, and it is NOT
                    // reset by Win auto-repeat (that re-arm made a held Win
                    // key fire after every chord).
                    if !win_press_fires(consumed_vk, chord) {
                        // A chord owns this press, so the OS needs the mask
                        // out of the way to run Win+E / Win+D normally.
                        release_mask();
                        debug_log(&format!(
                            "win key-up: no toggle (vk={consumed_vk} chord={chord})",
                        ));
                        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
                    }
                    debug_log(&format!("bare Win press vk={consumed_vk} -> reporting HOTKEY"));
                    // ORDER MATTERS. Kill the Start gesture *before* the mask
                    // goes away, and keep the mask held until nex has shown
                    // the overlay and taken foreground. Releasing first let
                    // the shell start opening Start on this same key-up; nex
                    // then took focus while Start was up, saw a phantom
                    // outside-click + Escape and hid the panel immediately —
                    // reported as "the hotkey opens Start instead of nex".
                    invalidate_start_gesture();
                    // Fire: the helper MUST report Win presses — over
                    // Task Manager / elevated focus, nex's raw-input sink
                    // receives nothing (UIPI), so without this the
                    // hotkey goes dead exactly there. Do not drop this
                    // for tidiness: it is the only detector elevated
                    // windows have.
                    //
                    // Normal focus makes nex's raw input report the same
                    // key-up, which is expected overlap. It is collapsed
                    // in nex by `hotkey::send_hotkey_event` (50 ms
                    // same-press gate at the sender), NOT by the runtime:
                    // a busy worker dequeues the two copies seconds
                    // apart, so receipt-time dedup let one press toggle
                    // twice for the first seconds after launch.
                    HOTKEY_FIRED.store(true, Ordering::SeqCst);
                    WIN_KEY_RELEASED.store(true, Ordering::SeqCst);
                    // The mask stays held across this key-up (see the order
                    // note above). The loop thread releases it once nex has
                    // acknowledged the press or the grace window expires —
                    // never from the hook, which cannot wait.
                    DEFER_MASK_RELEASE.store(true, Ordering::SeqCst);
                    if let Some(tid) = HELPER_THREAD_ID.get() {
                        unsafe { PostThreadMessageW(*tid, WM_NEX_WAKE, 0, 0); }
                    }
                }
                return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
            }
        }

        if is_keydown && !is_mod && !is_win_vk
            && CONSUMED_WIN_VK.load(Ordering::SeqCst) != 0
        {
            // Chord detected — release the mask so the OS handles Win+X and
            // remember the chord until this press ends. CONSUMED_WIN_VK keeps
            // its VK across Win auto-repeat on purpose: clearing it here let
            // the next repeat start a *new* bare press that fired on key-up.
            debug_log(&format!(
                "chord key vk={vk} joined the Win press; mask released, no toggle",
            ));
            CHORD_DETECTED.store(true, Ordering::SeqCst);
            release_mask();
        }
    }

    // Only key-down triggers matter for hotkey dispatch.
    if !is_keydown {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    }

    let is_target = if target_is_win {
        vk == VK_LWIN || vk == VK_RWIN
    } else {
        vk == target_vk
    };

    if is_target && !injected && detect_enabled && check_modifiers_atomic() {
        if target_is_win {
            // Win key-down: hold the mask (blocks Start) and DON'T EAT so
            // Win+ chords still reach the OS. The Win key-up branch above
            // owns the hotkey report — this key-down fires nothing.
            if CONSUMED_WIN_VK.load(Ordering::SeqCst) == 0 {
                CONSUMED_WIN_VK.store(vk, Ordering::SeqCst);
                CHORD_DETECTED.store(false, Ordering::SeqCst);
                hold_mask();
            }
            // DO NOT EAT — pass Win through so Win+ chords work
            return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
        }
        // Non-Win hotkey: eat key-down (prevents key from reaching focused window).
        debug_log(&format!(
            "hook: non-win target match vk={} (hk_vk={}) — firing",
            vk, target_vk
        ));
        HOTKEY_FIRED.store(true, Ordering::SeqCst);
        if let Some(tid) = HELPER_THREAD_ID.get() {
            unsafe { PostThreadMessageW(*tid, WM_NEX_WAKE, 0, 0); }
        }
        return 1;
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) }
}

static HOTKEY_FIRED: AtomicBool = AtomicBool::new(false);

/// Set by the mouse hook proc when a real button-down arrives; the message
/// loop forwards it to nex as `MOUSEDOWN`.
static MOUSE_DOWN_FIRED: AtomicBool = AtomicBool::new(false);

/// Forward button-down events to nex so it can dismiss the overlay.
///
/// nex's own `WH_MOUSE_LL` hook is Medium IL, and low-level hooks are
/// UIPI-gated: they never fire for input routed to an elevated window. This
/// High-IL hook sees clicks on Task Manager / Settings, which is exactly the
/// case nex cannot observe itself.
unsafe extern "system" fn mouse_hook_proc(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    let entered = std::time::Instant::now();
    let result = unsafe { mouse_hook_proc_inner(n_code, w_param, l_param) };
    note_hook_duration("mouse", entered.elapsed());
    result
}

unsafe fn mouse_hook_proc_inner(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, MSLLHOOKSTRUCT};

    if n_code < 0 {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    }

    let msg = w_param as u32;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_RBUTTONDOWN: u32 = 0x0204;
    const WM_MBUTTONDOWN: u32 = 0x0207;
    const WM_XBUTTONDOWN: u32 = 0x020B;
    const WM_NCLBUTTONDOWN: u32 = 0x00A1;
    const WM_NCRBUTTONDOWN: u32 = 0x00A4;

    if matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_RBUTTONDOWN
            | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN
            | WM_NCLBUTTONDOWN
            | WM_NCRBUTTONDOWN
    ) {
        let msll = unsafe { *(l_param as *const MSLLHOOKSTRUCT) };
        // Skip injected input: nex evaluates genuine clicks only, and
        // SendInput-driven clicks must not dismiss the panel.
        const LLMHF_INJECTED: u32 = 0x1;
        if (msll.flags & LLMHF_INJECTED) == 0 {
            MOUSE_DOWN_FIRED.store(true, Ordering::SeqCst);
            if let Some(tid) = HELPER_THREAD_ID.get() {
                unsafe { PostThreadMessageW(*tid, WM_NEX_WAKE, 0, 0) };
            }
        }
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) }
}

// Hot-path hotkey definition, published as plain atomics.
//
// `keyboard_hook_proc` runs inside the OS input pipeline for every
// keystroke on the system: it must never take a lock. Blocking there
// stalls all input (Windows also silently unhooks procs that exceed
// LowLevelHooksTimeout), and a poisoned-lock `unwrap` would panic
// inside the hook and kill the process. `CFG` stays for cold paths
// (target_pid, event_name) that only run on the message-loop thread.
static HK_VK: AtomicU32 = AtomicU32::new(0);
static HK_IS_WIN: AtomicBool = AtomicBool::new(false);
static HK_MOD_CTRL: AtomicBool = AtomicBool::new(false);
static HK_MOD_ALT: AtomicBool = AtomicBool::new(false);
static HK_MOD_SHIFT: AtomicBool = AtomicBool::new(false);
static HK_MOD_WIN: AtomicBool = AtomicBool::new(false);
/// Master gate: when false, the helper's hook is pass-through (nex owns
/// hotkey detection).
static HK_ENABLED: AtomicBool = AtomicBool::new(false);

fn publish_hotkey_atomics(cfg: &HotkeyConfig) {
    HK_VK.store(cfg.target_vk, Ordering::SeqCst);
    HK_IS_WIN.store(cfg.target_is_win, Ordering::SeqCst);
    HK_MOD_CTRL.store(cfg.mod_ctrl, Ordering::SeqCst);
    HK_MOD_ALT.store(cfg.mod_alt, Ordering::SeqCst);
    HK_MOD_SHIFT.store(cfg.mod_shift, Ordering::SeqCst);
    HK_MOD_WIN.store(cfg.mod_win, Ordering::SeqCst);
    HK_ENABLED.store(cfg.detect_enabled, Ordering::SeqCst);
}

/// Lock-free modifier check for the hook proc — same logic as
/// [`check_modifiers`], reading the published atomics.
fn check_modifiers_atomic() -> bool {
    if HK_MOD_CTRL.load(Ordering::SeqCst)
        && !CTRL_DOWN.load(Ordering::SeqCst)
        && !is_key_down(VK_LCONTROL)
        && !is_key_down(VK_RCONTROL)
    {
        return false;
    }
    if HK_MOD_ALT.load(Ordering::SeqCst)
        && !ALT_DOWN.load(Ordering::SeqCst)
        && !is_key_down(VK_LMENU)
        && !is_key_down(VK_RMENU)
    {
        return false;
    }
    if HK_MOD_SHIFT.load(Ordering::SeqCst)
        && !SHIFT_DOWN.load(Ordering::SeqCst)
        && !is_key_down(VK_LSHIFT)
        && !is_key_down(VK_RSHIFT)
    {
        return false;
    }
    if HK_MOD_WIN.load(Ordering::SeqCst) && !is_key_down(VK_LWIN) && !is_key_down(VK_RWIN) {
        return false;
    }
    true
}

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

/// Server out-buffer for the helper's pipes. A zero-sized buffer makes every
/// write wait for the reader, and the message loop writes on this thread — it
/// also pumps the global input hooks. 64 KiB holds a full log burst and a run
/// of click/hotkey lines without ever blocking the hook thread.
const PIPE_OUT_BUFFER_BYTES: u32 = 64 * 1024;

/// Create a server pipe this process owns.
///
/// Only used for the log pipe, which nex merely reads: a Medium-IL process may
/// read *down* from a High-IL server, so the helper may own that one. The
/// detection pipe is the opposite direction and is owned by nex — see
/// [`connect_detection_pipe`].
fn create_named_pipe(pipe_path: &str) -> Result<std::fs::File, String> {
    let wide_path = to_wide(pipe_path);
    let handle = unsafe {
        CreateNamedPipeW(
            wide_path.as_ptr(),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_MESSAGE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_OUT_BUFFER_BYTES,
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

/// Connect to the UI-owned detection pipe as the client.
///
/// nex (Medium IL) creates this pipe and reads it. UIPI refuses a Medium-IL
/// read of a High-IL pipe, so this process must never own the pipe object —
/// when it did, `write_hotkey` reported success into its own server end while
/// nex received nothing, and every bare-Win press was detected then dropped.
/// Writing down into a Medium-IL pipe is permitted, which is why the elevated
/// helper is the client here.
fn connect_detection_pipe(pipe_path: &str) -> Result<std::fs::File, String> {
    let wide = to_wide(pipe_path);
    let mut last_error = 0;
    // nex creates the pipe before launching this process, but the scheduled
    // task can start us before its accept loop is ready.
    for _ in 0..60 {
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if !handle.is_null() && handle as isize != -1 {
            return Ok(unsafe { std::fs::File::from_raw_handle(handle as *mut _) });
        }
        last_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(format!(
        "connect_detection_pipe failed for {pipe_path}: err={last_error}"
    ))
}

// ---------------------------------------------------------------------------
// Overlay foreground helper (called from message loop on event signal)
// ---------------------------------------------------------------------------

/// Monotonic activation generation. Each overlay-ready signal claims the
/// next number; a retry loop whose number is stale abandons instead of
/// fighting the newer activation (a newer signal always wins).
static FG_SEQ: AtomicU64 = AtomicU64::new(0);

/// True when a newer activation superseded `seq`.
fn is_superseded(seq: u64) -> bool {
    FG_SEQ.load(Ordering::SeqCst) != seq
}

/// Best-effort `hwnd=0x.. title='..' pid=.. proc='..'` snapshot.
/// Never panics; unknown fields render as `?`.
fn describe_window(hwnd: isize) -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW;
    let mut pid = 0u32;
    let _ = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            hwnd as _,
            &mut pid,
        )
    };
    let mut title = String::from("?");
    if hwnd != 0 {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd as _, buf.as_mut_ptr(), buf.len() as i32) };
        if n > 0 {
            title = String::from_utf16_lossy(&buf[..n as usize]);
            title = title.chars().take(80).collect();
        }
    }
    let mut proc = String::from("?");
    if pid != 0 {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h as isize != 0 {
            let mut buf = [0u16; 260];
            let mut len = buf.len() as u32;
            if unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) } != 0 {
                let full = String::from_utf16_lossy(&buf[..len as usize]);
                proc = full.rsplit(['\\', '/']).next().unwrap_or("?").to_string();
            }
            unsafe {
                CloseHandle(h);
            }
        }
    }
    format!("hwnd=0x{hwnd:x} title='{title}' pid={pid} proc='{proc}'")
}

/// One-line activation report: `force_activate_window: <outcome> target=.. actual=..`.
fn format_activation_report(outcome: &str, target: &str, actual: &str) -> String {
    format!("force_activate_window: {outcome} target={target} actual={actual}")
}

/// Write a diagnostic line to the helper debug log.
///
/// The helper has no console (windows_subsystem = "windows"), so file logging
/// is the only way to observe failures. Lines also fan out to the log pipe
/// (when nex connected) so they land in nex's own log.
///
/// **Never blocks the caller.** This runs inside the global low-level input
/// hooks (every key-down and key-up), where a file open, a `flush`, or a pipe
/// write waiting on a zero-buffer server stalls system-wide input for as long
/// as the I/O takes. The line is handed to a dedicated writer thread over a
/// bounded channel; when that channel is saturated the line is dropped rather
/// than delaying the input pipeline.
pub(crate) fn debug_log(msg: &str) {
    // `get` never blocks. Startup calls `init_log_sink` before the hooks are
    // installed, so this is set for every hook-driven line.
    let Some(tx) = LOG_TX.get() else {
        return;
    };
    let line = format!("[{}] {msg}", std::process::id());
    // try_send: a full queue means the sink is behind. Dropping one
    // diagnostic line beats stalling the keyboard and mouse.
    let _ = tx.try_send(line);
}

/// Log lines buffered between the hooks and the writer thread.
const LOG_QUEUE_LINES: usize = 1024;

static LOG_TX: OnceLock<std::sync::mpsc::SyncSender<String>> = OnceLock::new();

/// Start the log sink ahead of the hooks.
///
/// Called once from `main` before any hook can run, so the hook path never
/// pays for creating a thread. `debug_log` only ever calls `get`.
pub(crate) fn init_log_sink() {
    let _ = LOG_TX.set(spawn_log_sink());
}

fn spawn_log_sink() -> std::sync::mpsc::SyncSender<String> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<String>(LOG_QUEUE_LINES);
    if let Err(error) = std::thread::Builder::new()
        .name("NexHelper-log".into())
        .spawn(move || log_writer_loop(rx))
    {
        // No sink: lines are dropped below. Never fall back to writing
        // inline — that is what stalled input.
        eprintln!("NexHelper: log writer spawn failed: {error}");
    }
    tx
}

/// The only thread that touches the debug log file and the log pipe.
fn log_writer_loop(rx: std::sync::mpsc::Receiver<String>) {
    use std::io::Write;
    let path = std::env::temp_dir().join("NexHelper-debug.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    while let Ok(line) = rx.recv() {
        if let Some(f) = file.as_mut() {
            if writeln!(f, "{line}").is_err() {
                file = None;
            }
        }
        if let Ok(mut slot) = LOG_PIPE.lock() {
            if let Some(pipe) = slot.as_mut() {
                if writeln!(pipe, "{line}").is_err() {
                    *slot = None; // reader gone; file log continues
                }
            }
        }
    }
    if let Some(f) = file.as_mut() {
        let _ = f.flush();
    }
}

/// Outbound log pipe (helper serves, nex reads), same per-pid scoping
/// as the hotkey pipe.
/// Naming convention for new nex processes: future binaries are
/// `Nex.<Role>.exe`, e.g. `Nex.Elevated.exe`; pipes stay
/// `\\.\pipe\<Name>\<pid>`.
static LOG_PIPE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// `\\.\pipe\nex-hotkey\<pid>` -> `\\.\pipe\NexHelperLog\<pid>`.
/// nex.exe mirrors this rule to connect; pure so both ends agree.
fn log_pipe_for(pipe_path: &str) -> String {
    match pipe_path.rsplit_once('\\') {
        Some((_, pid)) if !pid.is_empty() => format!("\\\\.\\pipe\\NexHelperLog\\{pid}"),
        _ => format!("{pipe_path}-log"),
    }
}

/// Parked thread: serve the log pipe, then hold it for debug_log fan-out.
fn spawn_log_pipe_server(log_path: String) {
    std::thread::Builder::new()
        .name("NexHelper-log-pipe".into())
        .spawn(move || {
            let pipe = match create_named_pipe(&log_path) {
                Ok(p) => p,
                Err(e) => {
                    debug_log(&format!("log pipe create failed: {e}"));
                    return;
                }
            };
            if wait_for_client(&pipe).is_err() {
                return;
            }
            debug_log("log pipe client connected");
            if let Ok(mut slot) = LOG_PIPE.lock() {
                *slot = Some(pipe);
            }
        })
        .ok();
}

/// True when the config bytes differ from the last applied ones.
/// Pure: the event doorbell and the mtime safety-net funnel through one
/// reload so a single write applies exactly once.
fn config_changed(last_applied: &[u8], next: &[u8]) -> bool {
    last_applied != next
}

// ---------------------------------------------------------------------------
// Startup privilege self-check: elevation, integrity, UI access.
// ---------------------------------------------------------------------------

/// Integrity-level RID to name (SECURITY_MANDATORY_*_RID).
fn integrity_label(rid: u32) -> &'static str {
    match rid {
        0x1000 => "Low",
        0x2000 => "Medium",
        0x2100 => "MediumPlus",
        0x3000 => "High",
        0x4000 => "System",
        _ => "Unknown",
    }
}

/// TOKEN_ELEVATION_TYPE value to name.
fn elevation_label(t: u32) -> &'static str {
    match t {
        1 => "Default",
        2 => "Full",
        3 => "Limited",
        _ => "Unknown",
    }
}

fn format_startup_report(elev: &str, integrity: &str, ui_access: bool) -> String {
    format!("privilege: elevation={elev} integrity={integrity} uiaccess={ui_access}")
}

/// Best-effort current-process privilege snapshot. Unknowns render as `?`.
fn startup_privilege_report() -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenElevationType,
        TokenIntegrityLevel, TokenUIAccess, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut elev = String::from("?");
    let mut integrity = String::from("?");
    let mut ui_access = false;
    let mut ui_known = false;
    unsafe {
        let mut tok = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) != 0 {
            let mut val = 0u32;
            let mut len = 0u32;
            if GetTokenInformation(
                tok,
                TokenElevationType,
                &mut val as *mut u32 as *mut _,
                4,
                &mut len,
            ) != 0
            {
                elev = elevation_label(val).to_string();
            }
            let mut tml = [0u64; 8];
            let mut tml_len = 0u32;
            if GetTokenInformation(
                tok,
                TokenIntegrityLevel,
                tml.as_mut_ptr() as *mut _,
                (tml.len() * 8) as u32,
                &mut tml_len,
            ) != 0
            {
                let sid = *(tml.as_ptr() as *const *mut core::ffi::c_void);
                let n = *GetSidSubAuthorityCount(sid);
                if n > 0 {
                    let rid = *GetSidSubAuthority(sid, (n - 1) as u32);
                    integrity = integrity_label(rid).to_string();
                }
            }
            let mut ui_val = 0u32;
            let mut ui_len = 0u32;
            if GetTokenInformation(
                tok,
                TokenUIAccess,
                &mut ui_val as *mut u32 as *mut _,
                4,
                &mut ui_len,
            ) != 0
            {
                ui_access = ui_val != 0;
                ui_known = true;
            }
            CloseHandle(tok);
        }
    }
    let ui = if ui_known {
        ui_access.to_string()
    } else {
        String::from("?")
    };
    format!(
        "privilege: elevation={elev} integrity={integrity} uiaccess={ui}"
    )
}

/// Called when nex.exe signals that the overlay is now visible.
/// Uses High IL to bring the overlay to front, bypassing UIPI.
///
/// Strategy (from most reliable to fallback):
/// 1. Attach to the current foreground thread (same High IL, no UIPI),
///    then call SetForegroundWindow so the system grants the call.
/// 2. Also grant nex.exe foreground permission as a backup.
///
/// `seq` is this activation's generation: the retry loop abandons when a
/// newer overlay-ready signal supersedes it instead of stealing focus back.
fn set_overlay_foreground(seq: u64) {
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
            BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId,
            SetForegroundWindow,
        };
        use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

        // Already foreground: nothing to do.
        if GetForegroundWindow() == hwnd {
            debug_log(&format_activation_report(
                "already foreground",
                &describe_window(hwnd as isize),
                &describe_window(hwnd as isize),
            ));
            return;
        }

        // 1. Grant nex.exe foreground permission (backup path)
        if let Some(cfg) = CFG.read().unwrap().as_ref() {
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

        // Verify + retry: Task Manager / elevated windows re-assert focus
        // shortly after losing it, so a single call loses the race. Poll
        // until the overlay actually owns the foreground (bounded).
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
        while unsafe { GetForegroundWindow() } != hwnd {
            if is_superseded(seq) {
                debug_log(&format!(
                    "force_activate_window: abandoning superseded activation seq={seq}"
                ));
                return;
            }
            if std::time::Instant::now() > deadline {
                debug_log(&format_activation_report(
                    "failed",
                    &describe_window(hwnd as isize),
                    &describe_window(unsafe { GetForegroundWindow() } as isize),
                ));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));

            // Re-attach each attempt — the foreground thread may change.
            let fg = GetForegroundWindow();
            let fg_tid = if fg as isize == 0 {
                0
            } else {
                GetWindowThreadProcessId(fg, std::ptr::null_mut())
            };
            let helper_tid = GetCurrentThreadId();
            let should_attach = fg_tid != 0 && fg_tid != helper_tid;
            if should_attach {
                AttachThreadInput(helper_tid, fg_tid, 1);
            }
            // Alt-tap: synthetic input marks input as recent, satisfying
            // the foreground-lock check that elevated TM otherwise fails.
            send_alt_tap();
            SetForegroundWindow(hwnd);
            BringWindowToTop(hwnd);
            if should_attach {
                AttachThreadInput(helper_tid, fg_tid, 0);
            }
        }
        debug_log(&format_activation_report(
            "verified",
            &describe_window(hwnd as isize),
            &describe_window(hwnd as isize),
        ));
    }
}

/// RegisterHotKeyW id owned by whichever thread registered it.
///
/// The reload worker registers it (RegisterHotKey targets a thread), while the
/// pump loop matches incoming WM_HOTKEY against it, so both read one atomic.
static FALLBACK_HOTKEY_ID: AtomicU32 = AtomicU32::new(0);

/// Set when a reload decided this helper is superseded and must exit.
static RELOAD_EXIT_SIGNAL: AtomicBool = AtomicBool::new(false);

/// Ask the reload worker to re-read the hotkey config.
///
/// Never blocks: a full queue means a reload is already pending, and pending
/// reloads coalesce because the worker re-reads the file itself.
fn request_hotkey_reload(_path: &str, tx: &std::sync::mpsc::SyncSender<()>) {
    let _ = tx.try_send(());
}

/// True when a reloaded config asks for behaviour this process cannot
/// adopt in place, so the helper must exit and let nex start a fresh one.
///
/// `target_pid` changes mean nex restarted. So does a generation change:
/// nex writes one before it replaces a helper, and a helper that kept
/// running would hold the single detection pipe against its successor.
fn config_needs_restart(previous: Option<&HotkeyConfig>, next: &HotkeyConfig) -> bool {
    match previous {
        Some(previous) => {
            previous.target_pid != next.target_pid
                || (previous.generation != 0
                    && next.generation != 0
                    && previous.generation != next.generation)
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    // Start the log sink before anything can log or install a hook: the hook
    // path must never create a thread or touch a file inline.
    init_log_sink();
    // Log startup immediately (before anything can fail)
    debug_log("helper started");
    // Orphan release: a dead predecessor may have held the mask down
    // across its death. Raw key-up is a no-op otherwise.
    send_mask_up();

    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("NexHelper: {e}");
            std::process::exit(1);
        }
    };

    debug_log(&format!("config parsed: event='{}'", cfg.event_name));

    let pipe_path = cfg.pipe_path.clone();
    let config_event_name = cfg.config_event_name.clone();
    let config_file_path = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| w[1].clone())
        .unwrap_or_default();
    // Publish the hook's hot fields before the hook can ever run.
    publish_hotkey_atomics(&cfg);
    let watch_pid = cfg.target_pid;
    *CFG.write().unwrap() = Some(std::sync::Arc::new(cfg));

    // Watchdog: exit when nex.exe disappears — including while still
    // blocked in wait_for_client. Without this, helpers whose client never
    // connects linger forever and poison later pipe connections.
    std::thread::spawn(move || {
        let h = unsafe {
            windows_sys::Win32::System::Threading::OpenProcess(0x00100000, 0, watch_pid)
        };
        if h.is_null() || h as isize == -1 {
            debug_log("watchdog: OpenProcess failed, exiting helper");
            std::process::exit(0);
        }
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(h, u32::MAX);
        }
        debug_log("watchdog: nex exited, helper shutting down");
        release_mask();
        std::process::exit(0);
    });

    debug_log(&startup_privilege_report());
    spawn_log_pipe_server(log_pipe_for(&pipe_path));
    spawn_cmd_pipe_client(cmd_pipe_for(&pipe_path));
    // MFT snapshot servers for fixed NTFS volumes: the indexer reads them to
    // avoid cold directory walks. No-op when no volume qualifies.
    mft::spawn_snapshot_servers();
    // Persistent command channel to the indexer; legacy pipes above stay up
    // for indexers that predate it.
    mft::spawn_command_client(watch_pid);

    // Connect to the UI-owned detection pipe. nex creates it and reads; this
    // process only writes. See `connect_detection_pipe`.
    let mut pipe = match connect_detection_pipe(&pipe_path) {
        Ok(p) => p,
        Err(e) => {
            debug_log(&format!("detection pipe connect failed: {e}"));
            eprintln!("NexHelper: {e}");
            std::process::exit(1);
        }
    };
    debug_log("detection pipe connected");

    // Config-changed doorbell. nex (Medium IL) creates and signals it —
    // UIPI forbids Medium→High writes, so the helper can only open it for
    // SYNCHRONIZE, exactly like the overlay-ready event. When it fires we
    // re-read helper-config.json: no respawn, no pipe writes, no races.
    let config_event = SendHandle(unsafe {
        OpenEventW(
            0x00100000, // SYNCHRONIZE
            0,
            to_wide(&config_event_name).as_ptr(),
        )
    } as isize);
    if config_event.0 == 0 || config_event.0 == -1 {
        debug_log(&format!(
            "OpenEventW(config) failed: event='{config_event_name}'"
        ));
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
        let err = unsafe {
            windows_sys::Win32::Foundation::GetLastError()
        };
        debug_log(&format!("SetWindowsHookExW failed: err={err}"));
        eprintln!("NexHelper: SetWindowsHookExW failed");
        std::process::exit(1);
    }
    debug_log("WH_KEYBOARD_LL installed");

    // Install WH_MOUSE_LL so elevated-window clicks still dismiss the overlay.
    // Non-fatal: without it, nex just cannot see clicks over elevated windows.
    let mouse_hook_id = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowsHookExW(
            windows_sys::Win32::UI::WindowsAndMessaging::WH_MOUSE_LL as i32,
            Some(mouse_hook_proc),
            std::ptr::null_mut(),
            0,
        )
    };
    if mouse_hook_id.is_null() {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        debug_log(&format!(
            "WH_MOUSE_LL install failed: err={err}; elevated outside-click disabled"
        ));
    } else {
        debug_log("WH_MOUSE_LL installed");
    }

    // Register system-level hotkey fallback via RegisterHotKeyW (non-Win only).
    /// Reason a binding skips RegisterHotKey, or None when the system
    /// hotkey path serves it. Pure so both registration sites agree.
    fn fallback_demoted(vk: u32, any_mod: bool) -> Option<&'static str> {
        if (0x60..=0x6F).contains(&vk) {
            Some("numpad keys are indistinguishable to RegisterHotKey")
        } else if !any_mod {
            Some("single-tap needs hook press/release tracking")
        } else {
            None
        }
    }

    /// Re-read the config file, republish hook atomics, reset transient
    /// key state, and swap the RegisterHotKeyW fallback. Skips when the
    /// file bytes match the last applied reload.
    ///
    /// Returns true when the helper should exit: a config for a different nex
    /// process or a newer helper generation means this process is superseded.
    fn reload_hotkey_config(config_file_path: &str, last_applied: &mut Vec<u8>) -> bool {
        let bytes = match std::fs::read(config_file_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                debug_log(&format!("reload skipped: config unreadable ({error})"));
                return false;
            }
        };
        if !config_changed(last_applied, &bytes) {
            debug_log("reload skipped: config unchanged");
            return false;
        }
        if config_file_path.is_empty() {
            return false;
        }
        let next = match parse_config_file(config_file_path) {
            Ok(next) => next,
            Err(e) => {
                debug_log(&format!("reload failed: {e}"));
                return false;
            }
        };
        let previous = CFG.read().ok().and_then(|guard| guard.clone());
        if config_needs_restart(previous.as_deref(), &next) {
            debug_log("config names a newer generation or a different nex; exiting");
            // Claim the new bytes so a late mtime tick cannot reload them
            // again, then leave the loop to release the pipes.
            *last_applied = bytes;
            return true;
        }
        let previous_id = FALLBACK_HOTKEY_ID.swap(0, Ordering::SeqCst) as i32;
        if previous_id != 0 {
            unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(
                    std::ptr::null_mut(),
                    previous_id,
                );
            }
        }
        debug_log(&format!(
            "reloaded hotkey='{}' vk={} is_win={}",
            next.hotkey_desc, next.target_vk, next.target_is_win,
        ));
        publish_hotkey_atomics(&next);
        *CFG.write().unwrap() = Some(std::sync::Arc::new(next));
        // Reset the hook's transient state. A stale CONSUMED_WIN_VK (Win
        // key-down whose key-up was eaten by RIDEV_NOHOTKEYS) or a mask key
        // left held by the old Win path makes the new combo undetectable.
        CONSUMED_WIN_VK.store(0, Ordering::SeqCst);
        CHORD_DETECTED.store(false, Ordering::SeqCst);
        HOTKEY_FIRED.store(false, Ordering::SeqCst);
        WIN_KEY_RELEASED.store(false, Ordering::SeqCst);
        CTRL_DOWN.store(false, Ordering::SeqCst);
        ALT_DOWN.store(false, Ordering::SeqCst);
        SHIFT_DOWN.store(false, Ordering::SeqCst);
        release_mask();
        *last_applied = bytes;
        let g = CFG.read().unwrap();
        if let Some(cfg) = g.as_ref() {
            let id = register_fallback(cfg);
            FALLBACK_HOTKEY_ID.store(id as u32, Ordering::SeqCst);
        }
        false
    }

    fn register_fallback(ctx: &HotkeyConfig) -> i32 {
        if ctx.target_is_win || !ctx.detect_enabled { return 0; }
        // Demote bindings RegisterHotKey can't serve well; the hook stays
        // the detector. Same table as the unit-tested nex-side decision.
        if let Some(reason) = fallback_demoted(
            ctx.target_vk,
            ctx.mod_ctrl || ctx.mod_alt || ctx.mod_shift || ctx.mod_win,
        ) {
            debug_log(&format!("dispatches via hook: {reason}"));
            return 0;
        }
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
        let id = 0x4E46; // unique, different from nex.exe's ID

        let ok = unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::RegisterHotKey(
                std::ptr::null_mut(),
                id,
                mods,
                ctx.target_vk,
            )
        };
        if ok == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if err == 1409 {
                debug_log("RegisterHotKey refused (already in use): demoted to hook");
            } else {
                eprintln!("NexHelper: RegisterHotKeyW failed err={err} (fallback disabled)");
            }
            return 0;
        }
        id
    }

    // Register system-level hotkey fallback via RegisterHotKeyW (non-Win hotkeys only).
    let initial_id = {
        let cfg_guard = CFG.read().unwrap();
        register_fallback(cfg_guard.as_ref().unwrap())
    };
    FALLBACK_HOTKEY_ID.store(initial_id as u32, Ordering::SeqCst);
    let cfg_guard = CFG.read().unwrap();
    let ctx = cfg_guard.as_ref().unwrap();
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
    // the overlay-ready event, the nex process handle, and nex's
    // config-changed doorbell.
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_FAILED};
    use windows_sys::Win32::System::Threading::OpenEventW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MsgWaitForMultipleObjects, PeekMessageW, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };
    // WAIT_OBJECT_0 + 2 (or +1): hotkey config changed mid-session —
    // re-read the config file and re-register the RegisterHotKeyW fallback.
    let rehotkey_wake = WAIT_OBJECT_0 + if nex_handle_valid { 2 } else { 1 };

    const WM_HOTKEY: u32 = 0x0312;
    let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = unsafe { std::mem::zeroed() };
    let handles = [
        overlay_ready_event,
        if nex_handle_valid { nex_handle } else { std::ptr::null_mut() },
        config_event.0 as PipeHandle,
    ];
    let n_handles: u32 = if nex_handle_valid { 3 } else { 2 };
    // Bounded wait: the config file is stat'd every iteration, so a missed
    // doorbell signal still converges within this interval.
    const WAIT_SLICE_MS: u32 = 250;
    let mut last_config_mtime: Option<std::time::SystemTime> =
        std::fs::metadata(&config_file_path).ok().and_then(|m| m.modified().ok());
    // Hotkey reloads run on their own thread. Reading the config, parsing it
    // and calling RegisterHotKey on this loop would block the thread that
    // pumps WH_KEYBOARD_LL/WH_MOUSE_LL, and a low-level hook that is slow to
    // return stalls every keystroke and mouse move on the system.
    let (reload_tx, reload_rx) = std::sync::mpsc::sync_channel::<()>(1);
    {
        let path = config_file_path.clone();
        std::thread::Builder::new()
            .name("NexHelper-reload".into())
            .spawn(move || {
                let mut last_applied: Vec<u8> = Vec::new();
                while reload_rx.recv().is_ok() {
                    if reload_hotkey_config(&path, &mut last_applied) {
                        RELOAD_EXIT_SIGNAL.store(true, Ordering::SeqCst);
                        // Wake the pump loop so it leaves and the process
                        // releases its pipes for the successor.
                        if let Some(tid) = HELPER_THREAD_ID.get() {
                            unsafe {
                                windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                                    *tid,
                                    WM_NEX_WAKE,
                                    0,
                                    0,
                                )
                            };
                        }
                        return;
                    }
                }
            })
            .map_err(|error| debug_log(&format!("reload worker spawn failed: {error}")))
            .ok();
    }

    loop {

        // Wait for either a message, the overlay-ready event, or nex exit
        let wait_result = unsafe {
            MsgWaitForMultipleObjects(
                n_handles,                         // number of handles to wait on
                handles.as_ptr(),                  // handle array
                0,                                 // fWaitAll = false (any will do)
                WAIT_SLICE_MS,                     // bounded — see last_config_mtime
                QS_ALLINPUT,                       // wake for any message
            )
        };


        if wait_result == WAIT_FAILED {
            debug_log("MsgWaitForMultipleObjects failed, exiting loop");
            break;
        }

        if wait_result == WAIT_OBJECT_0 {
            // Overlay-ready event was signaled — nex.exe has shown the overlay,
            // now set foreground from High IL.
            // (Event is auto-reset, so it returns to nonsignaled automatically.)
            debug_log("overlay-ready event received, calling set_overlay_foreground");
            let seq = FG_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
            // Off this thread: it pumps the global LL hooks, and the verify
            // loop's sleeps (up to 600 ms) stalled every keystroke and mouse
            // move system-wide. A newer request supersedes via FG_SEQ.
            let spawned = std::thread::Builder::new()
                .name("nex-helper-activate".into())
                .spawn(move || {
                    set_overlay_foreground(seq);
                    debug_log("set_overlay_foreground returned");
                });
            if spawned.is_err() {
                set_overlay_foreground(seq);
                debug_log("set_overlay_foreground returned (inline)");
            }
        }
        // WAIT_OBJECT_0 + 1 = nex process handle signaled (nex exited)
        if nex_handle_valid && wait_result == WAIT_OBJECT_0 + 1 {
            debug_log("nex process exited, shutting down");
            break;
        }
        // Hotkey config changed mid-session — re-read the config file that
        // nex just rewrote, republish the hook's atomics, and swap the
        // RegisterHotKeyW fallback. No respawn, no cross-IL writes.
        if wait_result == rehotkey_wake {
            debug_log("config event received, reloading hotkey");
            request_hotkey_reload(&config_file_path, &reload_tx);
            if RELOAD_EXIT_SIGNAL.load(Ordering::SeqCst) {
                break;
            }
        }

        // Mtime safety-net: catches a missed doorbell signal within one
        // wait slice (250ms) no matter what. Content-skipped when the
        // event path already applied the same bytes.
        if let Ok(m) = std::fs::metadata(&config_file_path) {
            if let Ok(modt) = m.modified() {
                if Some(modt) != last_config_mtime {
                    last_config_mtime = Some(modt);
                    debug_log("mtime changed, reloading hotkey");
                    request_hotkey_reload(&config_file_path, &reload_tx);
                    if RELOAD_EXIT_SIGNAL.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
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
                (msg.message == WM_HOTKEY
                    && FALLBACK_HOTKEY_ID.load(Ordering::SeqCst) != 0
                    && msg.wParam as u32 == FALLBACK_HOTKEY_ID.load(Ordering::SeqCst))
                || HOTKEY_FIRED.load(Ordering::SeqCst);

            if will_send_hotkey {
                if let Some(cfg) = CFG.read().unwrap().as_ref() {
                    unsafe { AllowSetForegroundWindow(cfg.target_pid); }
                }
            }

            // Process WM_HOTKEY (RegisterHotKeyW fallback)
            if msg.message == WM_HOTKEY
                && FALLBACK_HOTKEY_ID.load(Ordering::SeqCst) != 0
                && msg.wParam as u32 == FALLBACK_HOTKEY_ID.load(Ordering::SeqCst)
            {
                if write_hotkey(&mut pipe).is_err() {
                    debug_log("write_hotkey failed — nex pipe reader gone");
                    got_quit = true;
                    break; // nex.exe disconnected
                }
            }

            // Deferred Win mask release: nex has now been told about the press
            // and has had the grace window to take foreground.
            if msg.message == windows_sys::Win32::UI::WindowsAndMessaging::WM_TIMER as u32
                && msg.wParam == MASK_RELEASE_TIMER_ID
            {
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(std::ptr::null_mut(), MASK_RELEASE_TIMER_ID) };
                if DEFER_MASK_RELEASE.swap(false, Ordering::SeqCst) {
                    debug_log("deferred mask release after overlay focus");
                    release_mask();
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
            debug_log("hook fired: target matched, sending to nex");
            if let Some(cfg) = CFG.read().unwrap().as_ref() {
                if cfg.target_is_win {
                    if pipe.write_all(b"SUPPRESS_ON\n").is_err() {
                        break; // nex.exe disconnected
                    }
                }
            }
            if write_hotkey(&mut pipe).is_err() {
                debug_log("write_hotkey failed — nex pipe reader gone");
                break; // nex.exe disconnected
            }
            // Mask release is deferred until nex has had time to show the
            // overlay and take foreground (see DEFER_MASK_RELEASE).
            if DEFER_MASK_RELEASE.load(Ordering::SeqCst) {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(
                        std::ptr::null_mut(),
                        MASK_RELEASE_TIMER_ID,
                        MASK_RELEASE_DELAY_MS,
                        None,
                    )
                };
            }
        }

        // Forward elevated-window clicks (nex's own mouse hook is UIPI-blocked
        // for those). nex decides whether the click was outside the panel.
        if MOUSE_DOWN_FIRED.swap(false, Ordering::SeqCst) {
            if pipe.write_all(b"MOUSEDOWN\n").is_err() {
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
        if !mouse_hook_id.is_null() {
            windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(mouse_hook_id);
        }
    }
    let leftover_id = FALLBACK_HOTKEY_ID.swap(0, Ordering::SeqCst) as i32;
    if leftover_id != 0 {
        unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(
                std::ptr::null_mut(),
                leftover_id,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A held Win key fires after its auto-repeat only when no chord key ever
    // went down: the chord flag lives for the whole press.
    #[test]
    fn win_press_fires_only_when_bare() {
        assert!(win_press_fires(0x5B, false));
        assert!(!win_press_fires(0x5B, true)); // Win+E
        assert!(!win_press_fires(0, false));   // key-up without a press
    }

    #[test]
    fn superseded_only_when_newer_claimed() {
        FG_SEQ.store(7, Ordering::SeqCst);
        assert!(!is_superseded(7));
        assert!(is_superseded(6));
        FG_SEQ.store(8, Ordering::SeqCst);
        assert!(is_superseded(7));
    }

    #[test]
    fn activation_report_carries_both_ends() {
        let r = format_activation_report(
            "failed",
            "hwnd=0x1 title='Nex' pid=1 proc='nex.exe'",
            "hwnd=0x2 title='Task' pid=2 proc='taskmgr.exe'",
        );
        assert!(r.starts_with("force_activate_window: failed"));
        assert!(r.contains("target=hwnd=0x1"));
        assert!(r.contains("actual=hwnd=0x2"));
    }

    #[test]
    fn describe_null_window_never_panics() {
        let d = describe_window(0);
        assert!(d.contains("hwnd=0x0"));
        assert!(d.contains("pid=0"));
    }

    #[test]
    fn log_pipe_mirrors_pid() {
        assert_eq!(
            log_pipe_for(r"\\.\pipe\nex-hotkey\1234"),
            r"\\.\pipe\NexHelperLog\1234"
        );
        assert_eq!(log_pipe_for("odd"), "odd-log");
    }

    #[test]
    fn privilege_labels_cover_known_values() {
        assert_eq!(integrity_label(0x2000), "Medium");
        assert_eq!(integrity_label(0x3000), "High");
        assert_eq!(integrity_label(0x4000), "System");
        assert_eq!(integrity_label(0x9999), "Unknown");
        assert_eq!(elevation_label(2), "Full");
        assert_eq!(elevation_label(3), "Limited");
        assert_eq!(elevation_label(9), "Unknown");
    }

    #[test]
    fn startup_report_is_one_line() {
        let r = format_startup_report("Full", "High", false);
        assert_eq!(r, "privilege: elevation=Full integrity=High uiaccess=false");
        assert!(!r.contains('\n'));
    }

    #[test]
    // nex writes this config as JSON, so the pipe arrives with its
    // backslashes doubled. The helper uses the value as a real pipe name.
    #[test]
    fn json_str_unescapes_a_windows_pipe_path() {
        let json = r#"{"pipe":"\\\\.\\pipe\\nex-hotkey\\42","target_pid":42}"#;
        assert_eq!(json_str(json, "pipe").unwrap(), r"\\.\pipe\nex-hotkey\42");
    }

    #[test]
    fn json_str_handles_plain_and_escaped_quotes() {
        assert_eq!(json_str(r#"{"hotkey":"Win"}"#, "hotkey").unwrap(), "Win");
        assert_eq!(json_str(r#"{"k":"a\"b"}"#, "k").unwrap(), "a\"b");
    }

    #[test]
    fn json_str_rejects_a_missing_key() {
        assert!(json_str(r#"{"a":"b"}"#, "hotkey").is_err());
    }
    fn json_u64_reads_generation() {
        let json = r#"{"generation":42,"target_pid":7}"#;
        assert_eq!(json_u64(json, "generation"), Some(42));
        assert_eq!(json_u64(json, "target_pid"), Some(7));
        assert_eq!(json_u64(json, "absent"), None);
    }

    fn config_with(target_pid: u32, generation: u64) -> HotkeyConfig {
        HotkeyConfig {
            pipe_path: String::from("test-pipe"),
            hotkey_desc: String::from("Win"),
            target_pid,
            target_vk: VK_LWIN,
            target_is_win: true,
            mod_ctrl: false,
            mod_alt: false,
            mod_shift: false,
            mod_win: false,
            event_name: String::from("evt"),
            config_event_name: String::from("cfg"),
            detect_enabled: true,
            generation,
        }
    }

    // A replacement helper takes the single detection pipe, so the previous
    // generation must recognise itself as superseded and exit.
    #[test]
    fn newer_generation_requires_restart() {
        let current = config_with(100, 3);
        let next = config_with(100, 4);
        assert!(config_needs_restart(Some(&current), &next));
    }

    #[test]
    fn same_generation_keeps_running() {
        let current = config_with(100, 3);
        let same = config_with(100, 3);
        assert!(!config_needs_restart(Some(&current), &same));
    }

    #[test]
    fn different_nex_pid_requires_restart() {
        let current = config_with(100, 3);
        let restarted = config_with(200, 3);
        assert!(config_needs_restart(Some(&current), &restarted));
    }

    // A legacy config without a generation must not make a live helper exit.
    #[test]
    fn absent_generation_does_not_force_restart() {
        let current = config_with(100, 3);
        let legacy = config_with(100, 0);
        assert!(!config_needs_restart(Some(&current), &legacy));
    }

    #[test]
    fn first_apply_never_requests_restart() {
        let next = config_with(100, 3);
        assert!(!config_needs_restart(None, &next));
    }

    #[test]
    fn config_changed_detects_new_bytes() {
        assert!(config_changed(b"", b"{\"a\":1}"));
        assert!(!config_changed(b"{\"a\":1}", b"{\"a\":1}"));
        assert!(config_changed(b"{\"a\":1}", b"{\"a\":2}"));
    }

    #[test]
    fn sim_allowlist_maps_known_commands() {
        assert_eq!(sim_action("play_pause"), Some(SimTap { vk: 0xB3, ctrl: false }));
        assert_eq!(sim_action("next"), Some(SimTap { vk: 0xB0, ctrl: false }));
        assert_eq!(sim_action("copy"), Some(SimTap { vk: 0x43, ctrl: true }));
        assert_eq!(sim_action("select_all"), Some(SimTap { vk: 0x41, ctrl: true }));
        assert_eq!(sim_action("vol_up"), Some(SimTap { vk: 0xAF, ctrl: false }));
    }

    #[test]
    fn sim_allowlist_rejects_unknown() {
        assert_eq!(sim_action(""), None);
        assert_eq!(sim_action("format_c:"), None);
        assert_eq!(sim_action("PLAY_PAUSE"), None);
        assert_eq!(sim_action("copy "), None);
    }

    #[test]
    fn cmd_pipe_mirrors_pid() {
        assert_eq!(
            cmd_pipe_for(r"\\.\pipe\nex-hotkey\1234"),
            r"\\.\pipe\NexHelperCmd\1234"
        );
    }

    #[test]
    fn mask_hold_release_transitions_once() {
        MASK_HELD.store(false, Ordering::SeqCst);
        assert!(hold_mask());
        assert!(!hold_mask());
        assert!(release_mask());
        assert!(!release_mask());
        assert!(hold_mask());
        MASK_HELD.store(false, Ordering::SeqCst);
    }
}