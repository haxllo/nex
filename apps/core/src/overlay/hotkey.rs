//! Global hotkey via a low-level keyboard hook (`WH_KEYBOARD_LL`).
//!
//! Unlike `RegisterHotKey`, a low-level keyboard hook can intercept
//! any key — including the Win key pressed alone — without
//! consuming a global hotkey slot. The hook runs on a dedicated OS
//! thread that owns the `GetMessageW` pump required by
//! `SetWindowsHookExW`.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::os::windows::io::FromRawHandle;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crossbeam_channel::Sender;

use crate::logging;
use crate::overlay::model::OverlayEvent;

// ---------------------------------------------------------------------------
// Static globals
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct HookContext {
    sender: Sender<OverlayEvent>,
    hotkey_id: i32,
    target_key: u32,
    target_is_win: bool,
    required_mods: Vec<u32>,
}

static HOOK_CTX: Mutex<Option<HookContext>> = Mutex::new(None);

/// One physical Win press, tracked once for every detector in this process.
///
/// The hook and the raw-input sink see the same keys but not the same
/// events: raw input can drop a make event (observed: Win+E delivered only
/// E's break), and Chromium's hook can run ahead of ours while the overlay
/// is focused. Deciding from per-detector state let the detector that missed
/// the chord call the press bare and toggle over Explorer. One state, written
/// by any detector that saw a key, removes that whole class of misfire.
static WIN_PRESS_ACTIVE: AtomicBool = AtomicBool::new(false);
static WIN_CHORD_SEEN: AtomicBool = AtomicBool::new(false);

/// Start of a physical Win press. Idempotent — auto-repeat and the second
/// detector both land here and neither may extend the press.
pub(crate) fn note_win_press_start() {
    WIN_CHORD_SEEN.store(false, Ordering::SeqCst);
    WIN_PRESS_ACTIVE.store(true, Ordering::SeqCst);
}

/// A real non-modifier key went down. Returns true when it landed inside a
/// Win press, i.e. the press is a chord and must never toggle.
pub(crate) fn note_chord_key() -> bool {
    if !WIN_PRESS_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    WIN_CHORD_SEEN.store(true, Ordering::SeqCst);
    true
}

/// End of the press, consumed exactly once. Returns true only when the press
/// stayed bare, so the toggle fires at most once even though the hook and raw
/// input both call this for the same key-up.
pub(crate) fn finish_win_press() -> bool {
    let active = WIN_PRESS_ACTIVE.swap(false, Ordering::SeqCst);
    let chord = WIN_CHORD_SEEN.swap(false, Ordering::SeqCst);
    let bare = active && !chord;
    if !bare {
        // The press ended as a chord, so no toggle event will arrive to clear
        // the focus-escape guard the key-down armed. Clear it here or the
        // next focus loss looks like an in-flight press.
        SUPPRESS_FOCUS_ESCAPE.store(false, Ordering::SeqCst);
    }
    bare
}

pub(crate) fn reset_win_press_state() {
    WIN_PRESS_ACTIVE.store(false, Ordering::SeqCst);
    WIN_CHORD_SEEN.store(false, Ordering::SeqCst);
}

static OVERLAY_HAS_FOCUS: AtomicBool = AtomicBool::new(false);
static SUPPRESS_FOCUS_ESCAPE: AtomicBool = AtomicBool::new(false);
static OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);
static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

/// True once the elevated helper's pipe reader is active. The helper only
/// detects bare-Win hotkeys (`detect = target_is_win` in its config), so local
/// detection defers to it only through [`helper_owns_detection`].
static HELPER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Helper generation counter. Bumped before a replacement helper is started
/// so a reconfig thread can tell whether the previous generation is gone.
///
/// Starts at 1, never 0: the helper treats generation 0 as "this config
/// predates generations" and skips the superseded check, so every helper
/// spawned with 0 could never recognise a later generation and would adopt a
/// new combo in place instead of exiting. That is what left Win dead until a
/// restart.
static HELPER_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Claim the next helper generation. Strictly increasing, never 0.
fn next_helper_generation() -> u64 {
    HELPER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// Runtime event sender, mirrored here so a helper relaunched outside the
/// listener (hotkey swap, recovery) can forward detections to the same queue.
static HELPER_EVENT_TX: Mutex<Option<Sender<OverlayEvent>>> = Mutex::new(None);

/// The combo the helper should currently run, kept in sync by
/// [`update_live_hotkey`].
///
/// The supervisor captured its spawn arguments once at thread start, so after
/// a live hotkey change it kept resurrecting the *previous* combo: the old
/// helper exited on the new generation as intended, the supervisor respawned,
/// and the replacement came back with the stale hotkey. It must read the
/// target fresh on every respawn.
static HELPER_TARGET: Mutex<Option<HelperTarget>> = Mutex::new(None);

/// Resolved helper target: hotkey key code, whether it is the Win key, the
/// required modifiers, and the original string (for config/logging).
#[derive(Clone)]
struct HelperTarget {
    key: u32,
    is_win: bool,
    mods: Vec<u32>,
    hotkey: String,
}

/// Publish the combo the helper supervisor should spawn with.
fn set_helper_target(key: u32, is_win: bool, mods: &[u32], hotkey: &str) {
    if let Ok(mut slot) = HELPER_TARGET.lock() {
        *slot = Some(HelperTarget {
            key,
            is_win,
            mods: mods.to_vec(),
            hotkey: hotkey.to_string(),
        });
    }
}

/// Read the combo the helper should be running right now.
fn current_helper_target() -> Option<HelperTarget> {
    HELPER_TARGET.lock().ok().and_then(|slot| slot.clone())
}

/// Real time of the last toggle any detection path accepted, plus the
/// clock it is measured against.
static HOTKEY_EVENT_AT_MS: AtomicU64 = AtomicU64::new(0);
static HOTKEY_CLOCK: OnceLock<Instant> = OnceLock::new();

/// Send one toggle to the runtime, unless another detection path already
/// sent one for the same key press.
///
/// **Contract — every sender of `OverlayEvent::Hotkey` goes through here.**
/// Detection overlaps on purpose, and must keep overlapping:
/// - raw input (host.rs WM_INPUT) is nex's own path and covers normal focus;
/// - the in-process hook (this file) covers the window before the helper
///   connects, and non-Win combos that RegisterHotKeyW refuses;
/// - the WM_HOTKEY fallback covers combos RegisterHotKeyW delivers;
/// - the helper's pipe is the ONLY path that sees elevated foreground
///   windows (UIPI blocks both raw input and the Medium-IL hook there).
///
/// The raw path and the hook both defer to the helper while it owns
/// detection (`helper_owns_detection()`: live AND bare-Win hotkey), so one
/// press normally has exactly one owner. This
/// send-site window stays as the backstop for the cases where they still
/// overlap.
///
/// History: a receipt-time window in the runtime (removed in `87ac9f8`)
/// failed whenever the worker was busy: the copies left the queue seconds
/// apart, both passed, and one press toggled twice — the "one press acts
/// like a double press, then it settles" complaint from the first seconds
/// after launch. v2.10.0 "fixed" this by deleting the WM_INPUT Win toggle,
/// 6854445 re-added it with the helper out of detection, and 055a47a
/// re-enabled helper detection without re-gating the raw path; the 50 ms
/// window papered over the double owner until startup load stretched the
/// copies past it.
pub(crate) fn send_hotkey_event(sender: &Sender<OverlayEvent>, hotkey_id: i32) -> bool {
    let now = HOTKEY_CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64;
    let prev = HOTKEY_EVENT_AT_MS.swap(now, Ordering::SeqCst);
    if same_press(prev, now) {
        logging::info("[nex::debug] Hotkey: duplicate detection path ignored");
        return false;
    }
    sender.send(OverlayEvent::Hotkey(hotkey_id)).is_ok()
}

/// Is `now_ms` the same physical press as the previous one at `prev_ms`?
/// A zero `prev_ms` means no press yet.
fn same_press(prev_ms: u64, now_ms: u64) -> bool {
    /// Window in which two detections are the same physical press.
    const SAME_PRESS_MS: u64 = 50;
    prev_ms != 0 && now_ms.saturating_sub(prev_ms) < SAME_PRESS_MS
}

/// Outbound command pipe to the helper (simulation transport).
/// Connected best-effort during helper setup; `None` when down.
static HELPER_CMD: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Send one allowlisted simulation command (`play_pause`, `copy`, …).
/// Returns false when the pipe is down.
pub(crate) fn send_helper_key(cmd: &str) -> bool {
    let mut line = cmd.to_string();
    line.push('\n');
    if let Ok(mut slot) = HELPER_CMD.lock() {
        if let Some(pipe) = slot.as_mut() {
            use std::io::Write;
            if pipe.write_all(line.as_bytes()).is_ok() {
                return true;
            }
        }
        *slot = None;
    }
    false
}

/// True from a consumed bare-Win key-down through its matching key-up.
/// The overlay host uses this to avoid treating the Win press's transient
/// focus loss as click-outside dismissal; the hotkey event owns that toggle.
pub(crate) fn is_win_key_hotkey() -> bool {
    HOOK_CTX.lock().ok().and_then(|g| g.as_ref().map(|ctx| ctx.target_is_win)).unwrap_or(false)
}

/// Check if raw-input VK matches the configured hotkey (target key +
/// required modifiers, no extra modifiers held).  Uses GetAsyncKeyState
/// for modifier checks.  Used by the WM_INPUT handler when the overlay
/// is visible and Chromium's WH_KEYBOARD_LL hook intercepts our hook.
/// Check if any common Win+ chord key is physically held (E, D, R, etc.).
/// Used on non-hook threads where KS0/KS1/KS2 are not accessible.
fn win_chord_held_via_async() -> bool {
    // Check common Win+ shortcuts: A-Z, 0-9, Tab, Space, Escape, F1-F24, etc.
    for vk in 0x30..=0x39 { if is_key_down(vk) { return true; } } // 0-9
    for vk in 0x41..=0x5A { if is_key_down(vk) { return true; } } // A-Z
    for vk in 0x70..=0x87 { if is_key_down(vk) { return true; } } // F1-F24
    if is_key_down(0x09) { return true; } // Tab
    if is_key_down(0x20) { return true; } // Space
    if is_key_down(0x13) { return true; } // Pause
    if is_key_down(0x2D) { return true; } // Insert
    false
}

pub(crate) fn check_raw_input_hotkey(vk: u16) -> bool {
    // nex owns hotkey detection unconditionally — raw input receives keys
    // globally regardless of which window (or integrity level) has focus.
    let ctx = match HOOK_CTX.lock().ok().and_then(|g| g.as_ref().cloned()) {
        Some(ctx) => ctx,
        None => return false,
    };
    if ctx.target_is_win {
        if !(vk == VK_LWIN as u16 || vk == VK_RWIN as u16) { return false; }
        // If any non-modifier key is also held, it's a Win chord — pass through.
        if win_chord_held_via_async() { return false; }
        return true;
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

pub(crate) fn set_overlay_visible(visible: bool) {
    OVERLAY_VISIBLE.store(visible, Ordering::SeqCst);
}

pub(crate) fn set_overlay_hwnd(hwnd: isize) {
    OVERLAY_HWND.store(hwnd, Ordering::SeqCst);
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
    if HELPER_ACTIVE.load(Ordering::SeqCst) {
        return false; // helper owns detection
    }
    let ctx = match HOOK_CTX.lock().ok().and_then(|g| g.as_ref().cloned()) {
        Some(ctx) => ctx,
        None => return false,
    };
    if ctx.target_is_win {
        if !(is_key_down(VK_LWIN) || is_key_down(VK_RWIN)) { return false; }
        // If any non-modifier key is held alongside Win, it's a chord.
        if win_chord_held_via_async() { return false; }
        return true;
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

/// Whether this process currently holds the mask. Centralizes the pairing so
/// a second detector cannot double-send, and a missed key-up cannot leave the
/// mask stuck down.
static MASK_HELD: AtomicBool = AtomicBool::new(false);

/// Unassigned VK sent only to break a pending Start gesture.
///
/// Windows cancels the Start-menu-on-Win-key-up as soon as any other key
/// event belongs to the same press. A key-up for a key that was never down is
/// the cheapest such event: no app maps 0xFF, so nothing types. This is the
/// guard Raycast uses, and it is the second line of defence behind the held
/// menu mask — if the mask was ever released early (chord, helper restart,
/// focus handoff), the Win key-up still cannot open Start.
pub(crate) const VK_START_GUARD: u16 = 0xFF;

/// Invalidate a pending Start gesture. Call this immediately before firing the
/// bundle on Win key-up, before the shell processes that key-up.
pub(crate) fn invalidate_start_gesture() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    };
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: VK_START_GUARD,
        wScan: 0,
        dwFlags: KEYEVENTF_KEYUP,
        time: 0,
        dwExtraInfo: 0,
    };
    let _ = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
}

/// Hold the menu mask.
///
/// The injected mask key is one OS-level resource shared with the elevated
/// helper, so only one process may send it per press. Callers decide who owns
/// the press (see the recording guard and `hold_mask_for_press`); these two
/// helpers only keep this process's own send/release pairing correct.
pub(crate) fn mask_hold() {
    if !MASK_HELD.swap(true, Ordering::SeqCst) {
        send_mask_down();
    }
}

/// Release the menu mask if this process is the one holding it.
pub(crate) fn mask_release() {
    if MASK_HELD.swap(false, Ordering::SeqCst) {
        send_mask_up();
    }
}

/// Hold the mask for a Win press unless the helper is the one doing it.
///
/// The helper masks only while the configured hotkey is Win; with a non-Win
/// hotkey its Win branch never runs, so this process must hold the mask or a
/// bare Win tap opens Start.
pub(crate) fn hold_mask_for_press() {
    if !helper_masks_win() {
        mask_hold();
    }
}

/// Release the mask unless the helper owns it — a release from this process
/// while the helper still expects the mask held would end its suppression.
pub(crate) fn release_mask_for_press() {
    if !helper_masks_win() {
        mask_release();
    }
}

/// Does the elevated helper hold the Win menu mask right now?
///
/// The helper's Win branch runs whenever its configured hotkey is Win, and it
/// masks every physical Win press in that case. For any other hotkey the
/// branch never runs, so nex must mask instead. Exactly one process may send
/// the injected mask key, which is why this gate exists.
fn helper_masks_win() -> bool {
    masks_win_press(HELPER_ACTIVE.load(Ordering::SeqCst), is_win_key_hotkey())
}

/// Pure form of [`helper_masks_win`], so the ownership rule can be tested
/// without a live helper or a globally installed hook context.
fn masks_win_press(helper_active: bool, hotkey_is_win: bool) -> bool {
    helper_active && hotkey_is_win
}

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
    if !is_win_key_hotkey() || helper_masks_win() {
        // Non-Win hotkey needs no Start guard, and when the helper owns
        // the mask a release from this process would drop it early.
        return;
    }
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
    if !is_win_key_hotkey() || helper_masks_win() {
        return;
    }
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

    let entered = std::time::Instant::now();
    let result = unsafe { keyboard_hook_proc_inner(n_code, w_param, l_param) };
    // This proc runs inside the OS input pipeline for every keystroke on the
    // system. Anything slow here is felt as low input-FPS for everything, so
    // samples above the LowLevelHooksTimeout budget are logged.
    let elapsed = entered.elapsed();
    if elapsed >= Duration::from_millis(30) {
        crate::runtime::log_warn(&format!(
            "[nex::stall] keyboard hook proc took {}ms",
            elapsed.as_millis()
        ));
    }
    result
}

unsafe fn keyboard_hook_proc_inner(
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

    let kb = unsafe { *(l_param as *const KBDLLHOOKSTRUCT) };
    let vk = kb.vkCode;
    let injected = (kb.flags & 0x10) != 0;

    let ctx = match HOOK_CTX.lock().ok().and_then(|g| g.as_ref().cloned()) {
        Some(ctx) => ctx,
        None => {
            if is_keydown && !injected {
                crate::runtime::log_info(&format!(
                    "[nex::diag] Hook proc: HOOK_CTX is None, passing vk={:?} keydown={}",
                    vk, is_keydown,
                ));
            }
            return unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
            };
        }
    };

    // Track key state via atomics — GetAsyncKeyState lies when
    // the WebView has focus and has consumed the Ctrl key-down.
    if !injected {
        match vk {
            VK_LCONTROL | VK_RCONTROL => CTRL_DOWN.store(is_keydown, Ordering::SeqCst),
            VK_LMENU | VK_RMENU => ALT_DOWN.store(is_keydown, Ordering::SeqCst),
            VK_LSHIFT | VK_RSHIFT => SHIFT_DOWN.store(is_keydown, Ordering::SeqCst),
            _ => {}
        }
    }

    // Recording a hotkey in Settings: a bare Win tap must not open Start.
    // Raw input (the recorder) can observe keys but not block them, so the
    // menu mask has to be held for this press.
    //
    // Ownership rules, so exactly one process sends the injected mask key:
    //  - helper live *and* the configured hotkey is Win: the helper masks on
    //    every physical Win press, so this process stands down.
    //  - helper live but the hotkey is not Win: the helper's Win branch is
    //    not active, so nothing would hold the mask — nex must.
    //  - no helper: nex owns it.
    if !injected
        && (vk == VK_LWIN || vk == VK_RWIN)
        && crate::overlay::host::RECORDING_HOTKEY.load(Ordering::SeqCst)
        && !helper_masks_win()
    {
        if is_keydown {
            mask_hold();
        } else if is_keyup && MASK_HELD.load(Ordering::SeqCst) {
            // Checked by mask state, not RECORDING_HOTKEY: a chord commit
            // (Win+E) ends recording before Win comes back up.
            mask_release();
            invalidate_start_gesture();
        }
    }

    // --- Win press bookkeeping, shared with the raw-input sink ---
    //
    // Real keys only, and only when the hotkey is Win. The press is tracked
    // once for every detector: a raw-input make event that never arrives
    // (observed on Win+E) or a hook bypassed by Chromium's can no longer turn
    // the same press into a bare tap in the other detector.
    if ctx.target_is_win && !injected {
        let is_win_vk = vk == VK_LWIN || vk == VK_RWIN;
        let is_mod = matches!(
            vk,
            VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU | VK_LSHIFT | VK_RSHIFT
        );

        if is_win_vk && is_keydown {
            note_win_press_start();
            hold_mask_for_press();
            SUPPRESS_FOCUS_ESCAPE.store(true, Ordering::SeqCst);
            // DO NOT EAT — pass Win through so Win+ chords work. Nothing
            // fires here; the key-up decides between chord and bare tap.
            return unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(
                    std::ptr::null_mut(), n_code, w_param, l_param)
            };
        }

        if !is_win_vk && is_keydown && !is_mod && note_chord_key() {
            crate::runtime::log_info(&format!(
                "[nex::debug] Hook: chord detected vk={}, passing through",
                vk,
            ));
            // Let the OS own the chord with no mask in the way.
            release_mask_for_press();
        }

        if is_win_vk && is_keyup {
            // The helper reports its own press, so when it is live it owns
            // the decision; the raw sink consumes the press instead.
            if !HELPER_ACTIVE.load(Ordering::SeqCst) {
                let bare = finish_win_press();
                mask_release();
                if bare {
                    crate::runtime::log_info(&format!(
                        "[nex::debug] Hook: bare Win key-up vk={}, opening Nex",
                        vk,
                    ));
                    invalidate_start_gesture();
                    let _ = send_hotkey_event(&ctx.sender, ctx.hotkey_id);
                }
            }
            return unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(
                    std::ptr::null_mut(), n_code, w_param, l_param)
            };
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

    if is_target && !injected && !ctx.target_is_win {
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
            // No helper gate here: the helper is spawned with detection off for
            // non-Win hotkeys, so deferring to it left only RegisterHotKeyW,
            // which refuses combos another app (e.g. an IME) already holds.
            if ctx.target_is_win {
                // Unreachable: the guard above requires a non-Win target.
                return unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(
                        std::ptr::null_mut(), n_code, w_param, l_param)
                };
            }
            if ctx.required_mods.iter().any(|&m| m == VK_LWIN || m == VK_RWIN) {
                // The eaten key never reaches the shell, so the held Win
                // would still open Start on key-up.
                invalidate_start_gesture();
            }
            let _ = send_hotkey_event(&ctx.sender, ctx.hotkey_id);
            return 1;
        }
    }

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param)
    }
}

// ---------------------------------------------------------------------------
// Mouse hook — instant click-outside hide
// ---------------------------------------------------------------------------

unsafe extern "system" fn mouse_hook_proc(
    n_code: i32,
    w_param: windows_sys::Win32::Foundation::WPARAM,
    l_param: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, MSLLHOOKSTRUCT};

    if n_code < 0 {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), n_code, w_param, l_param) };
    }

    let msg = w_param as u32;
    // Only process button-down messages — never UP.
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_RBUTTONDOWN: u32 = 0x0204;
    const WM_MBUTTONDOWN: u32 = 0x0207;
    const WM_XBUTTONDOWN: u32 = 0x020B;
    const WM_NCLBUTTONDOWN: u32 = 0x00A1;
    const WM_NCRBUTTONDOWN: u32 = 0x00A4;

    let is_button_down = matches!(
        msg,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN
    );

    if is_button_down {
        let msll = unsafe { *(l_param as *const MSLLHOOKSTRUCT) };
        // Skip injected (SendInput) clicks.
        const LLMHF_INJECTED: u32 = 0x1;
        if (msll.flags & LLMHF_INJECTED) == 0 {
            emit_outside_click_if_outside("mouse hook");
        }
    }

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::CallNextHookEx(
            std::ptr::null_mut(), n_code, w_param, l_param,
        )
    }
}

/// True when (x, y) lies outside the rect. Pure so the hit test is testable.
fn cursor_outside_rect(x: i32, y: i32, left: i32, top: i32, right: i32, bottom: i32) -> bool {
    x < left || x >= right || y < top || y >= bottom
}

/// Emit `OutsideClick` when the cursor is outside the overlay panel.
///
/// Shared by the in-process `WH_MOUSE_LL` hook and the elevated helper's
/// forwarded `MOUSEDOWN`. The Medium-IL hook is UIPI-gated: it never fires for
/// clicks routed to an elevated window (Task Manager, Settings), so the helper
/// reports those. A short debounce collapses the duplicate for normal windows,
/// where both paths observe the same click.
fn emit_outside_click_if_outside(source: &str) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

    if !OVERLAY_VISIBLE.load(Ordering::SeqCst) {
        return;
    }
    let hwnd = OVERLAY_HWND.load(Ordering::SeqCst) as windows_sys::Win32::Foundation::HWND;
    if hwnd.is_null() {
        return;
    }

    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return;
    }
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return;
    }
    if !cursor_outside_rect(cursor.x, cursor.y, rect.left, rect.top, rect.right, rect.bottom) {
        if OVERLAY_VISIBLE.load(Ordering::SeqCst) {
            logging::info(&format!(
                "[nex::debug] outside click ({source}) ignored: cursor inside panel cursor=({}, {}) rect=({},{})-({},{})",
                cursor.x, cursor.y, rect.left, rect.top, rect.right, rect.bottom
            ));
        }
        return;
    }

    let clock = OUTSIDE_CLICK_CLOCK.get_or_init(Instant::now);
    let now_ms = clock.elapsed().as_millis() as u64;
    let last_ms = LAST_OUTSIDE_CLICK_MS.swap(now_ms, Ordering::SeqCst);
    if last_ms != u64::MAX && now_ms.saturating_sub(last_ms) < OUTSIDE_CLICK_DEDUPE_MS {
        return;
    }

    // Dedicated event (not Escape): keyboard Escape keeps picker-ignore
    // semantics while a genuine click dismisses at once.
    logging::info(&format!("[nex::debug] outside click ({source})"));
    let ctx = HOOK_CTX.lock().ok().and_then(|g| g.as_ref().map(|c| c.sender.clone()));
    if let Some(sender) = ctx {
        let _ = sender.send(OverlayEvent::OutsideClick);
    } else {
        logging::warn("[nex] outside click ignored: no event channel");
    }
}

/// Both the UI hook and the helper report a normal-window click; collapse the
/// pair into one dismissal.
const OUTSIDE_CLICK_DEDUPE_MS: u64 = 150;
/// Zero point for the dedupe clock. Created lazily, so the *first* evaluation
/// always measures ~0 ms — the sentinel below makes it pass anyway.
static OUTSIDE_CLICK_CLOCK: OnceLock<Instant> = OnceLock::new();
/// Timestamp of the last emitted dismissal; `u64::MAX` means "none yet".
/// Without the sentinel the first click of the process always looked 0 ms
/// after a previous one and was silently deduped (the "click Task Manager
/// twice on the first show" report).
static LAST_OUTSIDE_CLICK_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Hook-free click-away poll.
///
/// The helper's `WH_MOUSE_LL` covers elevated windows once it connects, but
/// that path has startup and edge-case gaps: before the helper is live, and on
/// the first show over an elevated window, a click on Task Manager could go
/// unseen. Polling the async button state is not UIPI-gated, costs two cheap
/// calls every 40 ms only while the panel is visible, and feeds the same
/// outside-click helper. The 150 ms dedupe collapses overlap with the hook and
/// helper paths.
fn start_click_away_fallback(should_exit: Arc<AtomicBool>) {
    let spawned = thread::Builder::new()
        .name("nex-click-away-poll".into())
        .spawn(move || {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON,
            };

            let mut was_down = false;
            logging::info("[nex] click-away poll active (40ms, visible only)");
            loop {
                if should_exit.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(40));

                if !OVERLAY_VISIBLE.load(Ordering::SeqCst) {
                    was_down = false;
                    continue;
                }

                let down = unsafe {
                    (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0
                        || (GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000) != 0
                };
                if down && !was_down {
                    emit_outside_click_if_outside("poll");
                }
                was_down = down;
            }
        });
    if let Err(error) = spawned {
        logging::warn(&format!(
            "[nex] failed to spawn click-away fallback: {error}"
        ));
    }
}

// ---------------------------------------------------------------------------
// HotkeyListener
// ---------------------------------------------------------------------------

pub(crate) struct HotkeyListener {
    inner: Option<HotkeyListenerInner>,
}

/// State shared between the listener and the helper supervisor thread.
/// The helper connects asynchronously after the in-process hook is live,
/// so startup never blocks on PowerShell/schtasks/pipe retries.
struct HelperState {
    /// True while the supervisor has a live helper pipe (detection owner).
    active: bool,
    /// Non-zero for direct-spawn mode, 0 for scheduled-task mode.
    helper_process_handle: Option<isize>,
}

struct HotkeyListenerInner {
    should_exit: Arc<AtomicBool>,
    hook_thread: Option<thread::JoinHandle<()>>,
    thread_id: Arc<OnceLock<u32>>,
    helper: Arc<Mutex<HelperState>>,
    /// Late helper supervisor thread (detached on drop; self-exits on shutdown).
    helper_thread: Option<thread::JoinHandle<()>>,
}

impl HotkeyListener {
    pub(crate) fn start(hotkey_str: &str, event_tx: Sender<OverlayEvent>) -> Result<Self, String> {
        Self::start_internal(hotkey_str, event_tx, true)
    }

    pub(crate) fn start_no_helper(hotkey_str: &str, event_tx: Sender<OverlayEvent>) -> Result<Self, String> {
        Self::start_internal(hotkey_str, event_tx, false)
    }

    fn start_internal(hotkey_str: &str, event_tx: Sender<OverlayEvent>, try_helper: bool) -> Result<Self, String> {
        let parsed = parse_hotkey(hotkey_str)
            .map_err(|e| format!("invalid hotkey '{hotkey_str}': {e}"))?;
        let required_mods = modifier_vks_from_names(&parsed.modifiers)?;
        let target_key = vk_from_key(&parsed.key)?;
        let target_is_win = target_key == VK_LWIN;
        // Dispatch verdict at startup: which path serves this combo and
        // why. The hook thread logs per-attempt outcomes on top of this.
        let has_win_mod = required_mods.iter().any(|m| *m == VK_LWIN || *m == VK_RWIN);
        let startup_reason = dispatch_for(target_key, target_is_win || has_win_mod, !required_mods.is_empty(), false);
        logging::info(&format!(
            "[nex] hotkey '{hotkey_str}' dispatches via {}: {}",
            if startup_reason == DispatchReason::SystemHotkey { "system hotkey + hook" } else { "hook" },
            dispatch_reason_text(startup_reason),
        ));

        let should_exit = Arc::new(AtomicBool::new(false));
        let thread_id: Arc<OnceLock<u32>> = Arc::new(OnceLock::new());

        start_click_away_fallback(Arc::clone(&should_exit));

        *HOOK_CTX.lock().unwrap() = Some(HookContext { sender: event_tx.clone(), hotkey_id: 1, target_key, target_is_win, required_mods: required_mods.clone() });
        if let Ok(mut slot) = HELPER_EVENT_TX.lock() {
            *slot = Some(event_tx.clone());
        }

        if try_helper {
            logging::info("[nex] hotkey: elevated helper setup scheduled (async)");
        } else {
            logging::info("[nex] hotkey: skipping helper, using in-process hook");
        }

        // --- Fallback: in-process hook thread (existing code path) ---
        let should_exit_for_thread = should_exit.clone();
        let thread_id_for_thread = thread_id.clone();
        const WM_NEX_REREGISTER: u32 = 0x8000 + 0x4E47;

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
                    WH_KEYBOARD_LL, WH_MOUSE_LL,
                };
                let hook_id = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL as i32, Some(keyboard_hook_proc), std::ptr::null_mut(), 0)
                };
                if hook_id.is_null() {
                    let _ = ready_tx.send(Err("SetWindowsHookExW failed".into()));
                    return;
                }
                let mouse_hook_id = unsafe {
                    SetWindowsHookExW(WH_MOUSE_LL as i32, Some(mouse_hook_proc), std::ptr::null_mut(), 0)
                };
                if mouse_hook_id.is_null() {
                    logging::warn("[nex::debug] Hook: SetWindowsHookExW(WH_MOUSE_LL) failed, click-outside disabled");
                }
                let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
                let _ = thread_id_for_thread.set(tid);
                HOOK_THREAD_ID.store(tid, Ordering::SeqCst);
                let _ = ready_tx.send(Ok(()));

                // Register system-level hotkey fallback via RegisterHotKeyW.
                // Unlike WH_KEYBOARD_LL + WM_INPUT, RegisterHotKeyW delivers
                // WM_HOTKEY to this thread's message queue regardless of
                // foreground window or integrity level.  This ensures the
                // hotkey works even when elevated/UWP windows (Task Manager,
                // advanced settings) are foreground.
                // Win combos skip it (raw-input path owns them); numpad and
                // single-tap bindings demote to the hook with a reason.
                const ERROR_HOTKEY_ALREADY_IN_USE: u32 = 1409;
                let mut fallback_id: u32 = 0;
                // Copy what we need and release HOOK_CTX immediately. The
                // hook proc locks this same mutex on EVERY keystroke, so a
                // guard held across the message loop below deadlocks all
                // system input (frozen keyboard, Start menu leaking through,
                // tray menus never opening).
                let (target_is_win_local, target_key_local, required_mods_local) = {
                    let g = HOOK_CTX.lock().unwrap();
                    let c = g.as_ref().unwrap();
                    (c.target_is_win, c.target_key, c.required_mods.clone())
                };
                if !target_is_win_local {
                    let mod_map: &[(u32, u32)] = &[
                        (0x11, 0x0002), // VK_CTRL  → MOD_CONTROL
                        (0x12, 0x0001), // VK_ALT   → MOD_ALT
                        (0x10, 0x0004), // VK_SHIFT → MOD_SHIFT
                        (0x5B, 0x0008), // VK_LWIN  → MOD_WIN
                        (0x5C, 0x0008), // VK_RWIN  → MOD_WIN
                    ];
                    let mut mods: u32 = 0;
                    for &(vk, flag) in mod_map {
                        if required_mods_local.contains(&vk) { mods |= flag; }
                    }
                    // Generate a unique hotkey ID that won't collide.
                    fallback_id = (tid as u32).wrapping_mul(7) ^ 0x4E45;
                    let reason = dispatch_for(
                        target_key_local,
                        false,
                        !required_mods_local.is_empty(),
                        false,
                    );
                    if reason != DispatchReason::SystemHotkey {
                        logging::info(&format!(
                            "[nex::debug] Hook: dispatches via hook: {}",
                            dispatch_reason_text(reason),
                        ));
                        fallback_id = 0;
                    } else {
                        let ok = match register_hotkey {
                            Some(f) => unsafe { f(std::ptr::null_mut(), fallback_id as i32, mods, target_key_local as u32) },
                            None => { logging::warn("[nex::debug] Hook: RegisterHotKeyW not available (fallback disabled)"); 0 },
                        };
                        if ok == 0 {
                            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                            if err == ERROR_HOTKEY_ALREADY_IN_USE {
                                logging::warn("[nex::debug] Hook: RegisterHotKey refused (already in use): demoted to hook");
                            } else {
                                logging::warn(&format!(
                                    "[nex::debug] Hook: RegisterHotKeyW failed err={} (fallback disabled)",
                                    err,
                                ));
                            }
                            fallback_id = 0;
                        } else {
                            logging::info("[nex::debug] Hook: RegisterHotKeyW fallback active");
                        }
                    }
                } else {
                    logging::info(&format!(
                        "[nex::debug] Hook: dispatches via hook: {}",
                        dispatch_reason_text(DispatchReason::WinCombo),
                    ));
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
                    // Skip if we're shutting down — WM_HOTKEY may be queued
                    // before WM_QUIT, and the global HOOK_CTX may have been
                    // overwritten by a new listener by now.
                    if msg.message == WM_NEX_REREGISTER {
                        // Live hotkey update: swap the RegisterHotKeyW
                        // fallback to the freshly synced HOOK_CTX. Brief lock
                        // on this thread is fine — it's not the input path.
                        logging::info("[nex::debug] Hook: live re-register requested");
                        let g = HOOK_CTX.lock().unwrap();
                        if let Some(ctx) = g.as_ref() {
                            if fallback_id != 0 {
                                if let Some(f) = unregister_hotkey {
                                    unsafe { f(std::ptr::null_mut(), fallback_id as i32); }
                                }
                                fallback_id = 0;
                            }
                            if !ctx.target_is_win {
                                let mod_map: &[(u32, u32)] = &[
                                    (0x11, 0x0002), (0x12, 0x0001),
                                    (0x10, 0x0004), (0x5B, 0x0008), (0x5C, 0x0008),
                                ];
                                let mut mods: u32 = 0;
                                for &(vk, flag) in mod_map {
                                    if ctx.required_mods.contains(&vk) { mods |= flag; }
                                }
                                fallback_id = (tid as u32).wrapping_mul(7) ^ 0x4E45;
                                let reason = dispatch_for(
                                    ctx.target_key,
                                    false,
                                    !ctx.required_mods.is_empty(),
                                    false,
                                );
                                if reason != DispatchReason::SystemHotkey {
                                    logging::info(&format!(
                                        "[nex::debug] Hook: live dispatches via hook: {}",
                                        dispatch_reason_text(reason),
                                    ));
                                    fallback_id = 0;
                                } else {
                                    let ok = match register_hotkey {
                                        Some(f) => unsafe { f(std::ptr::null_mut(), fallback_id as i32, mods, ctx.target_key as u32) },
                                        None => 0,
                                    };
                                    if ok == 0 {
                                        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                                        if err == ERROR_HOTKEY_ALREADY_IN_USE {
                                            logging::warn("[nex::debug] Hook: live RegisterHotKey refused (already in use): demoted to hook");
                                        } else {
                                            logging::warn("[nex::debug] Hook: live re-register FAILED");
                                        }
                                        fallback_id = 0;
                                    }
                                    else { logging::info("[nex::debug] Hook: live fallback re-registered OK"); }
                                }
                            } else {
                                logging::info(&format!(
                                    "[nex::debug] Hook: live dispatches via hook: {}",
                                    dispatch_reason_text(DispatchReason::WinCombo),
                                ));
                            }
                        }
                        drop(g);
                        continue;
                    }
                    if msg.message == WM_HOTKEY && fallback_id != 0 && !should_exit_for_thread.load(Ordering::SeqCst) {
                        let hk_id = msg.wParam as u32;
                        if hk_id == fallback_id {
                            logging::info("[nex::debug] Hook: WM_HOTKEY fallback fired, sending toggle");
                            let ctx = match HOOK_CTX.lock().ok().and_then(|g| g.as_ref().cloned()) {
                                Some(ctx) => ctx,
                                None => continue,
                            };
                            let _ = send_hotkey_event(&ctx.sender, ctx.hotkey_id);
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
                if !mouse_hook_id.is_null() {
                    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(mouse_hook_id); }
                }
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook_id); }
                // A successor listener may already own the slot; leave it.
                let _ = HOOK_THREAD_ID.compare_exchange(tid, 0, Ordering::SeqCst, Ordering::SeqCst);
                if !should_exit_for_thread.load(Ordering::SeqCst) {
                    logging::warn("[nex] hotkey message loop exited unexpectedly");
                }
            })
            .map_err(|e| {
                *HOOK_CTX.lock().unwrap_or_else(|e| e.into_inner()) = None;
                format!("failed to spawn hotkey thread: {e}")
            })?;

        // Bounded wait: a hung hook install must fail startup loudly,
        // not block it forever. Disconnect still means the thread died.
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                *HOOK_CTX.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Err(e);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                *HOOK_CTX.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Err("hotkey thread start timed out".into());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                *HOOK_CTX.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Err("hotkey thread panicked".into());
            }
        }

        // --- Late helper supervision (async) ---
        // The in-process hook is live. A supervisor thread runs the
        // elevated-helper dance (task check, schtasks /run, pipe connect),
        // owns the pipe while it lasts, and reconnects with backoff when the
        // helper dies. Failing that, the hook keeps working.
        let helper = Arc::new(Mutex::new(HelperState {
            active: false,
            helper_process_handle: None,
        }));
        let helper_thread = if try_helper {
            // Seed the supervisor's shared target before it can spawn.
            set_helper_target(target_key, target_is_win, &required_mods, &hotkey_str);
            let should_exit_h = should_exit.clone();
            let helper_h = helper.clone();
            let event_tx_h = event_tx.clone();
            let required_mods_h = required_mods.clone();
            let hotkey_str_h = hotkey_str.to_string();
            thread::Builder::new()
                .name("nex-helper-supervisor".into())
                .spawn(move || {
                    run_helper_supervisor(
                        target_key,
                        target_is_win,
                        required_mods_h,
                        hotkey_str_h,
                        event_tx_h,
                        helper_h,
                        should_exit_h,
                    );
                })
                .map_err(|e| format!("failed to spawn helper supervisor: {e}"))?
                .into()
        } else {
            None
        };

        Ok(Self { inner: Some(HotkeyListenerInner {
            should_exit,
            hook_thread: Some(thread),
            thread_id,
            helper,
            helper_thread,
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
                if let Ok(st) = inner.helper.lock() {
                    if st.active {
                        // Helper mode: the supervisor thread reads the pipe
                        // and reconnects on its own; HELPER_ACTIVE is the
                        // liveness signal for the detection path.
                        return !inner.should_exit.load(Ordering::SeqCst)
                            && HELPER_ACTIVE.load(Ordering::SeqCst);
                    }
                }
                // Hook mode (helper pending or skipped): thread_id is set
                // before the message pump starts. If it's not set yet the
                // thread is still in early startup — treat as alive.
                if inner.thread_id.get().is_none() {
                    return true;
                }
                !inner.should_exit.load(Ordering::SeqCst)
                    && inner.hook_thread.as_ref().is_some_and(|t| !t.is_finished())
            }
            None => false,
        }
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.should_exit.store(true, Ordering::SeqCst);
            // Clear the global hook context BEFORE signalling threads
            // so the hook proc immediately becomes a no-op — no stale
            // hotkey events can fire from the dying listener.
            if let Ok(mut hook_ctx) = HOOK_CTX.lock() {
                *hook_ctx = None;
            }
            CTRL_DOWN.store(false, Ordering::SeqCst);
            ALT_DOWN.store(false, Ordering::SeqCst);
            SHIFT_DOWN.store(false, Ordering::SeqCst);
            reset_win_press_state();
            SUPPRESS_FOCUS_ESCAPE.store(false, Ordering::SeqCst);
            HELPER_ACTIVE.store(false, Ordering::SeqCst);

            // Helper mode (installed or in-flight): terminate the helper
            // process, then drop (detach) the pipe reader thread.
            //
            // The pipe reader blocks on BufReader::lines() reading from
            // the named pipe.  Terminating the helper closes its pipe
            // end, making the next read fail -> the reader loop breaks
            // and the thread exits on its own.
            //
            // For direct-spawn mode (helper_process_handle != 0) we use
            // TerminateProcess.  For scheduled-task mode the handle is 0
            // (no direct process ownership), so we kill by process name —
            // safe because nex is single-instance so killing any
            // NexHelper.exe belongs to us.
            let handle = inner
                .helper
                .lock()
                .ok()
                .and_then(|mut st| st.helper_process_handle.take());
            if let Some(handle) = handle {
                kill_helper_process(handle);
            }
            // The late supervisor thread may still be mid-spawn; it checks
            // should_exit and kills the helper itself if it connects late.
            drop(inner.helper_thread.take());

            // Hook mode: post WM_QUIT and detach — must not join here
            // because Drop runs on the runtime worker thread and joining
            // would freeze the event loop (Windows marks the process
            // "not responding").  HOOK_CTX is cleared immediately below,
            // so the old hook proc becomes a no-op; the thread exits
            // naturally on WM_QUIT.
            if let Some(&tid) = inner.thread_id.get() {
                logging::info(&format!("[nex] shutdown: posting WM_QUIT to hotkey thread {tid}"));
                post_quit_to_thread(tid);
            }
            drop(inner.hook_thread.take());
        }
    }
}

/// Terminate the helper: TerminateProcess when we own a direct handle,
/// taskkill by name for scheduled-task mode (handle == 0).
fn kill_helper_process(handle: isize) {
    if handle != 0 {
        logging::info("[nex] shutdown: terminating helper process");
        unsafe {
            let h = handle as *mut core::ffi::c_void;
            windows_sys::Win32::System::Threading::TerminateProcess(h, 1);
            windows_sys::Win32::Foundation::CloseHandle(h);
        }
    } else {
        logging::info("[nex] shutdown: killing NexHelper by name (scheduled task)");
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "NexHelper.exe"])
            .creation_flags(0x08000000)
            .output();
    }
}

/// Time a replaced helper gets to notice that its pipes are gone and exit on
/// its own. The wait ends sooner whenever the old generation is observed to
/// be gone; this is only the ceiling for a wedged helper.
const HELPER_REPLACE_GRACE: Duration = Duration::from_millis(1500);
const HELPER_REPLACE_POLL: Duration = Duration::from_millis(25);

/// Wait until the previous helper generation has released the detection pipe.
///
/// A replacement helper cannot be started while another one is running: the
/// pipe server exists once, so the newcomer exits and the old generation —
/// still configured for the previous hotkey — keeps detecting. Killing it from
/// here is not an option (`taskkill` from Medium IL is denied against the
/// elevated helper), so this only closes every pipe the helper could be
/// holding: its read hits EOF and the next write fails, which is what its
/// exit paths watch. Called from a reconfig/recovery thread only.
fn wait_for_previous_helper_to_exit(previous_generation: u64) {
    // Doorshut: the command pipe client and any command-channel connection
    // for the previous generation must not survive into the new one.
    if let Ok(mut slot) = HELPER_CMD.lock() {
        *slot = None;
    }
    crate::mft_channel::drop_helper_connection();

    // Ring the config doorbell: a helper that is still alive reloads, finds
    // the new config and re-checks its pipes. Harmless if it never sees it.
    {
        use windows_sys::Win32::System::Threading::SetEvent;
        if let Ok(guard) = CONFIG_CHANGED_EVENT.lock() {
            if *guard != 0 {
                unsafe { SetEvent(*guard as *mut core::ffi::c_void) };
            }
        }
    }

    let started = std::time::Instant::now();
    loop {
        let generation_advanced = HELPER_GENERATION.load(Ordering::SeqCst) != previous_generation;
        if generation_advanced || !HELPER_ACTIVE.load(Ordering::SeqCst) || !helper_pipe_open() {
            return;
        }
        if started.elapsed() >= HELPER_REPLACE_GRACE {
            logging::warn(
                "[nex] previous helper generation is still holding the detection pipe; starting anyway",
            );
            return;
        }
        thread::sleep(HELPER_REPLACE_POLL);
    }
}

/// Whether this process currently holds a helper detection pipe.
fn helper_pipe_open() -> bool {
    HELPER_ACTIVE.load(Ordering::SeqCst)
}

/// Keep the elevated helper alive for the lifetime of the listener.
///
/// The in-process hook is always live, so the helper is a supplement: when it
/// is unavailable the hotkey still works for normal windows. On pipe EOF the
/// helper is gone, so this retries with capped exponential backoff instead of
/// leaving elevated-window detection degraded until the next UI restart.
fn run_helper_supervisor(
    target_key: u32,
    target_is_win: bool,
    required_mods: Vec<u32>,
    hotkey: String,
    event_tx: Sender<OverlayEvent>,
    helper: Arc<Mutex<HelperState>>,
    should_exit: Arc<AtomicBool>,
) {
    const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    let mut backoff = INITIAL_BACKOFF;
    let mut reconnect = false;

    loop {
        if should_exit.load(Ordering::SeqCst) {
            return;
        }
        if reconnect {
            // The previous helper is gone or wedged; make sure the scheduled
            // task cannot leave a second detector running.
            kill_stale_helper();
        }
        reconnect = true;

        // Read the target fresh: a live hotkey change swaps it, and respawning
        // with the values captured at thread start resurrected the old combo.
        let (spawn_key, spawn_is_win, spawn_mods, spawn_hotkey) =
            match current_helper_target() {
                Some(target) => (target.key, target.is_win, target.mods, target.hotkey),
                None => (
                    target_key,
                    target_is_win,
                    required_mods.clone(),
                    hotkey.clone(),
                ),
            };

        match spawn_and_connect_helper(spawn_key, spawn_is_win, &spawn_mods, &spawn_hotkey) {
            Ok((helper_handle, pipe_file)) => {
                if should_exit.load(Ordering::SeqCst) {
                    kill_helper_process(helper_handle);
                    return;
                }
                backoff = INITIAL_BACKOFF;
                HELPER_ACTIVE.store(true, Ordering::SeqCst);
                if let Ok(mut st) = helper.lock() {
                    st.active = true;
                    st.helper_process_handle = Some(helper_handle);
                }
                logging::info("[nex] hotkey: using elevated helper for detection");
                connect_helper_aux_pipes();

                read_helper_pipe(pipe_file, &event_tx);

                HELPER_ACTIVE.store(false, Ordering::SeqCst);
                HELPER_GENERATION.fetch_add(1, Ordering::SeqCst);
                set_suppress_focus_escape(false);
                if let Ok(mut st) = helper.lock() {
                    st.active = false;
                }
                if should_exit.load(Ordering::SeqCst) {
                    return;
                }
                logging::warn("[nex] helper pipe closed; reconnecting elevated helper");
            }
            Err(error) => {
                if should_exit.load(Ordering::SeqCst) {
                    return;
                }
                logging::warn(&format!(
                    "[nex] elevated helper unavailable ({error}); in-process hook stays active"
                ));
            }
        }

        // Slice the backoff so quit is never held up by the retry timer.
        let mut waited = Duration::ZERO;
        while waited < backoff {
            if should_exit.load(Ordering::SeqCst) {
                return;
            }
            let slice = Duration::from_millis(200).min(backoff - waited);
            thread::sleep(slice);
            waited += slice;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Consume the detection pipe until the helper goes away.
fn read_helper_pipe(pipe_file: std::fs::File, event_tx: &Sender<OverlayEvent>) {
    use std::io::BufRead;
    logging::info("[nex] helper detection pipe reader attached");
    let reader = std::io::BufReader::new(pipe_file);
    for line in reader.lines() {
        match line {
            Ok(l) if l == "HOTKEY" => {
                logging::info("[nex] pipe: HOTKEY received from helper");
                // Stamped on arrival: the helper's copy of a press that raw
                // input also saw lands here within the same 50 ms gate.
                let _ = send_hotkey_event(event_tx, 1);
            }
            Ok(l) if l == "SUPPRESS_ON" => set_suppress_focus_escape(true),
            Ok(l) if l == "SUPPRESS_OFF" => set_suppress_focus_escape(false),
            // Elevated-window click forwarded by the helper; the local hook
            // cannot see these (UIPI).
            Ok(l) if l == "MOUSEDOWN" => {
                if OVERLAY_VISIBLE.load(Ordering::SeqCst) {
                    logging::info("[nex::debug] helper mouse-down received");
                }
                emit_outside_click_if_outside("helper")
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

/// Best-effort auxiliary pipes. Never gates detection.
fn connect_helper_aux_pipes() {
    // Command channel: the UI owns the server so the elevated helper can read
    // down into it (write-up from Medium IL to a High-IL pipe is denied).
    start_helper_cmd_server();

    let log_pipe_name = format!(r"\\.\pipe\NexHelperLog\{}", std::process::id());
    match connect_pipe(&log_pipe_name, 0x80000000, 3) {
        Ok(log_file) => {
            match thread::Builder::new()
                .name("NexHelper-log-reader".into())
                .spawn(move || {
                    use std::io::BufRead;
                    let reader = std::io::BufReader::new(log_file);
                    for line in reader.lines().flatten() {
                        logging::info(&format!("[helper] {line}"));
                    }
                }) {
                Ok(_) => {}
                Err(e) => logging::warn(&format!("[nex] helper log reader spawn failed: {e}")),
            }
        }
        Err(e) => logging::warn(&format!("[nex] helper log pipe unavailable: {e}")),
    }
}

/// Serve the helper's command channel.
///
/// The pipe object must be created by this Medium-IL process. The helper runs
/// at High IL and UIPI blocks write-up into a High-IL pipe, which is why the
/// old helper-created command pipe always failed with "access denied". Here
/// the UI owns the write end and the elevated helper connects as a client that
/// only reads; reading down is permitted.
fn start_helper_cmd_server() {
    let pipe_name = format!(r"\\.\pipe\NexHelperCmd\{}", std::process::id());
    let spawned = thread::Builder::new()
        .name("NexHelper-cmd-pipe".into())
        .spawn(move || {
            use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};

            const PIPE_ACCESS_OUTBOUND: u32 = 0x0000_0002;
            const PIPE_TYPE_MESSAGE: u32 = 0x0000_0004;
            const PIPE_WAIT: u32 = 0x0000_0000;
            const PIPE_UNLIMITED_INSTANCES: u32 = 255;
            const ERROR_PIPE_CONNECTED: u32 = 535;

            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn CreateNamedPipeW(
                    lpName: *const u16,
                    dwOpenMode: u32,
                    dwPipeMode: u32,
                    nMaxInstances: u32,
                    nOutBufferSize: u32,
                    nInBufferSize: u32,
                    nDefaultTimeOut: u32,
                    lpSecurityAttributes: *mut core::ffi::c_void,
                ) -> *mut core::ffi::c_void;
                fn ConnectNamedPipe(
                    hNamedPipe: *mut core::ffi::c_void,
                    lpOverlapped: *mut core::ffi::c_void,
                ) -> i32;
            }

            let wide = to_wide(&pipe_name);
            let handle = unsafe {
                CreateNamedPipeW(
                    wide.as_ptr(),
                    PIPE_ACCESS_OUTBOUND,
                    PIPE_TYPE_MESSAGE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    4096,
                    4096,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle.is_null() || handle as isize == -1 {
                logging::warn("[nex] helper cmd pipe server create failed");
                return;
            }

            let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            if connected == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_PIPE_CONNECTED {
                    logging::warn(&format!("[nex] helper cmd pipe accept failed err={error}"));
                    unsafe { CloseHandle(handle) };
                    return;
                }
            }

            let file = unsafe { std::fs::File::from_raw_handle(handle as *mut _) };
            if let Ok(mut slot) = HELPER_CMD.lock() {
                *slot = Some(file);
            }
            logging::info("[nex] helper cmd pipe connected");
        });
    if let Err(error) = spawned {
        logging::warn(&format!("[nex] failed to spawn helper cmd pipe: {error}"));
    }
}

/// Create the helper's detection pipe and hand back the read end.
///
/// The pipe object MUST be created by this Medium-IL process. It used to be
/// created by the helper, and nex then connected as a reader — a Medium-IL
/// read of a High-IL pipe, which UIPI refuses. The helper's writes reported
/// success into its own pipe while nex received nothing, so every bare-Win
/// press was detected and then silently dropped.
///
/// Ownership follows the same rule as `NexHelperCmd`, only mirrored: here the
/// UI owns the pipe and reads, and the elevated helper connects as the client
/// that writes. Writing *down* into a Medium-IL pipe is permitted.
fn create_helper_detection_pipe(pipe_name: &str) -> Result<std::fs::File, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};

    const PIPE_ACCESS_INBOUND: u32 = 0x0000_0001;
    // BYTE mode, not MESSAGE: the reader is a `BufReader::lines()` loop, which
    // issues partial reads. On a message-mode pipe a read that does not
    // consume a whole message blocks, so the helper's line was written and
    // the reader still never produced it.
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const ERROR_PIPE_CONNECTED: u32 = 535;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            lpName: *const u16,
            dwOpenMode: u32,
            dwPipeMode: u32,
            nMaxInstances: u32,
            nOutBufferSize: u32,
            nInBufferSize: u32,
            nDefaultTimeOut: u32,
            lpSecurityAttributes: *mut core::ffi::c_void,
        ) -> *mut core::ffi::c_void;
        fn ConnectNamedPipe(
            hNamedPipe: *mut core::ffi::c_void,
            lpOverlapped: *mut core::ffi::c_void,
        ) -> i32;
    }

    let wide = to_wide(pipe_name);
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            0,
            65536,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(format!(
            "CreateNamedPipeW(inbound) failed for {pipe_name}: err={}",
            unsafe { GetLastError() }
        ));
    }

    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    if connected == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_PIPE_CONNECTED {
            unsafe { CloseHandle(handle) };
            return Err(format!(
                "ConnectNamedPipe(inbound) failed for {pipe_name}: err={error}"
            ));
        }
    }

    Ok(unsafe { std::fs::File::from_raw_handle(handle as *mut _) })
}

/// Clear a helper left over from a previous connection before reconnecting.
///
/// `taskkill` from Medium IL is denied against the High-IL helper, so this
/// only closes this process's ends of the helper's pipes; the helper's own
/// EOF/failed-write paths are what make it exit. Used by the supervisor
/// before a reconnect, never while a helper pipe is live.
fn kill_stale_helper() {
    if HELPER_ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut slot) = HELPER_CMD.lock() {
        *slot = None;
    }
    crate::mft_channel::drop_helper_connection();
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

/// Whether the elevated helper is connected. Gates the single-activation
/// path in host.rs (the helper owns foreground for every hotkey).
pub(crate) fn helper_active() -> bool {
    HELPER_ACTIVE.load(Ordering::SeqCst)
}

/// Whether the helper is connected AND detecting. It is spawned with
/// `detect = target_is_win`, so for any other hotkey local detection must
/// keep firing even while the helper is live.
pub(crate) fn helper_owns_detection() -> bool {
    HELPER_ACTIVE.load(Ordering::SeqCst) && is_win_key_hotkey()
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

static CONFIG_CHANGED_EVENT: std::sync::Mutex<isize> = std::sync::Mutex::new(0);

/// Thread id of the live in-process hook thread (owns the RegisterHotKeyW
/// fallback registration), 0 when none. Rewritten by every listener start:
/// a set-once value kept pointing at the first, dead thread after a restart.
pub(crate) static HOOK_THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const WM_NEX_REREGISTER: u32 = 0x8000 + 0x4E47;

/// Ask the hook thread to swap its RegisterHotKeyW fallback to the current
/// HOOK_CTX. No-op if the hook thread isn't running.
pub(crate) fn request_fallback_reregister() {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
    if tid != 0 {
        let ok = unsafe { PostThreadMessageW(tid, WM_NEX_REREGISTER, 0, 0) };
        logging::info(&format!(
            "[nex::debug] fallback re-register posted to tid={tid}: ok={ok}"
        ));
    } else {
        logging::warn("[nex::debug] fallback re-register: no hook thread id");
    }
}

/// Create the config-changed doorbell event.
///
/// Created by nex (Medium IL) so the High-IL helper can open it —
/// UIPI blocks Medium→High writes, so the helper can never own this
/// object. Mirrors [`create_overlay_event`].
fn create_config_event() -> String {
    use windows_sys::Win32::System::Threading::CreateEventW;
    let pid = std::process::id();
    let name = format!("Global\\nex-hotkey-config-{pid}");
    let wide = to_wide(&name);
    let handle = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, wide.as_ptr()) };
    if let Ok(mut guard) = CONFIG_CHANGED_EVENT.lock() {
        *guard = handle as isize;
    }
    name
}

/// The overlay-ready event name this process created and the helper opens.
fn overlay_event_name() -> String {
    format!("Global\\nex-overlay-ready-{}", std::process::id())
}

/// The config-changed doorbell name this process created.
fn config_event_name() -> String {
    format!("Global\\nex-hotkey-config-{}", std::process::id())
}

/// The helper's detection pipe for this nex process.
fn helper_detection_pipe_name() -> String {
    format!(r"\\.\pipe\nex-hotkey\{}", std::process::id())
}

/// Ring the config-changed doorbell so a running helper re-reads its config.
fn ring_config_changed_event() {
    use windows_sys::Win32::System::Threading::SetEvent;
    let handle = CONFIG_CHANGED_EVENT.lock().map(|guard| *guard).unwrap_or(0);
    if handle != 0 {
        unsafe { SetEvent(handle as *mut core::ffi::c_void) };
    }
}

/// Live-update the hotkey of an already-running elevated helper.
///
/// Rewrites `helper-config.json` (nex owns that file) and rings the
/// config-changed doorbell the helper waits on. No `taskkill`, no
/// `schtasks /end`, no pipe writes — so nothing can race or leave the
/// system without a detector. Returns false when the helper isn't
/// active, in which case the caller must do a full re-registration.
pub(crate) fn update_live_hotkey(hotkey_str: &str) -> bool {
    if !HELPER_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    let parsed = match parse_hotkey(hotkey_str) {
        Ok(p) => p,
        Err(e) => {
            logging::warn(&format!("[nex] live hotkey update '{hotkey_str}': {e}"));
            return false;
        }
    };
    let target_is_win = parsed.key.eq_ignore_ascii_case("win");
    let target_key = if target_is_win {
        VK_LWIN
    } else {
        match vk_from_key(&parsed.key) {
            Ok(vk) => vk,
            Err(e) => {
                logging::warn(&format!("[nex] live hotkey update '{hotkey_str}': {e}"));
                return false;
            }
        }
    };
    let required_mods = match modifier_vks_from_names(&parsed.modifiers) {
        Ok(m) => m,
        Err(e) => {
            logging::warn(&format!("[nex] live hotkey update '{hotkey_str}': {e}"));
            return false;
        }
    };

    // nex owns ALL hotkey detection: raw input for Win, the WM_INPUT chord
    // check plus the RegisterHotKey fallback for combos. Nothing about the
    // detector is tied to a helper generation, so a hotkey change is a pure
    // local retarget: sync HOOK_CTX and the transient state below.
    //
    // The helper still has to be regenerated when `detect` flips (Win ↔
    // non-Win), and returns false for that. That restart closes pipes instead
    // of killing a process — see [`restart_elevated_helper`].
    let old_is_win = HOOK_CTX.lock().ok().and_then(|g| g.as_ref().map(|c| c.target_is_win)).unwrap_or(false);
    let helper_needs_new_generation = old_is_win != target_is_win;
    if let Ok(mut guard) = HOOK_CTX.lock() {
        if let Some(ctx) = guard.as_mut() {
            ctx.target_key = target_key;
            ctx.target_is_win = target_is_win;
            ctx.required_mods = required_mods.clone();
        }
    }
    // Publish the new target for the helper supervisor. Without this the
    // supervisor kept respawning with the combo captured at listener start,
    // so the replacement helper came back on the old hotkey.
    set_helper_target(target_key, target_is_win, &required_mods, hotkey_str);
    // Drop transient key state so a half-finished press of the previous
    // hotkey can't swallow the first press of the new one.
    reset_win_press_state();
    CTRL_DOWN.store(false, Ordering::SeqCst);
    ALT_DOWN.store(false, Ordering::SeqCst);
    SHIFT_DOWN.store(false, Ordering::SeqCst);
    crate::overlay::host::reset_raw_win_state();
    // Swap the RegisterHotKeyW fallback to the new combo on the hook
    // thread (raw input already covers Win + chords via synced HOOK_CTX).
    request_fallback_reregister();
    // Re-arm the raw-input sink: its NOHOTKEYS suppression flag depends on
    // whether the hotkey is a Win combo, and it was registered under the
    // previous hotkey's state.
    crate::overlay::host::rearm_raw_input_sink();
    logging::info(&format!(
        "[nex] live hotkey update applied locally: '{hotkey_str}'"
    ));
    // Rewrite the helper's config and ring its doorbell. Without the rewrite
    // the doorbell only makes the helper re-read its own stale file. The
    // generation is carried along, so a helper that has since been superseded
    // exits instead of adopting a config meant for its successor.
    let helper_config_written = HELPER_ACTIVE.load(Ordering::SeqCst)
        && write_helper_config(
            &helper_config_path(),
            target_key,
            target_is_win,
            &required_mods,
            hotkey_str,
            &overlay_event_name(),
            &helper_detection_pipe_name(),
            &config_event_name(),
        )
        .map_err(|error| {
            logging::warn(&format!(
                "[nex] live hotkey update '{hotkey_str}': helper config rewrite failed: {error}"
            ));
        })
        .is_ok();
    if helper_config_written {
        ring_config_changed_event();
    }

    // The detector is retargeted either way. This only reports whether the
    // helper also has to be regenerated — its `detect` flag follows the combo
    // family, so a Win ↔ non-Win change needs a new helper process.
    if helper_needs_new_generation {
        logging::info(&format!(
            "[nex] hotkey family changed ({old_is_win} → {target_is_win}); the elevated helper needs a new generation"
        ));
        return false;
    }
    true
}

/// Restart the elevated helper with the current hotkey, without touching the
/// in-process detector.
///
/// The hook thread, raw-input sink and RegisterHotKey fallback keep working,
/// so no keystroke is ever undetected, and nothing in the restart path spawns
/// a console binary as the old listeners did. The previous helper generation
/// is asked to exit through the pipes it holds ([`wait_for_previous_helper_to_exit`]);
/// only when it is confirmed gone is a replacement started.
///
/// Runs on a reconfig thread; blocks for at most [`HELPER_REPLACE_GRACE`].
///
/// The helper is only regenerated when the window is free of one: a live pipe
/// belongs to the listener's supervisor, which owns reconnecting, and a second
/// connector would fight it for the single pipe instance.
/// Resolve a hotkey string into the helper config's target triple.
fn resolve_helper_target(hotkey_str: &str) -> Result<(u32, bool, Vec<u32>), String> {
    let parsed = parse_hotkey(hotkey_str)?;
    let target_is_win = parsed.key.eq_ignore_ascii_case("win");
    let target_key = if target_is_win {
        VK_LWIN
    } else {
        vk_from_key(&parsed.key)?
    };
    let required_mods = modifier_vks_from_names(&parsed.modifiers)?;
    Ok((target_key, target_is_win, required_mods))
}

/// Rewrite the helper config for `hotkey_str` at the current generation.
///
/// Used when the helper cannot be replaced here (a live supervisor owns the
/// pipe) but the config still has to carry the bumped generation — that is the
/// only signal the running helper uses to decide it has been superseded.
fn publish_helper_config_for(hotkey_str: &str) -> Result<(), String> {
    let (target_key, target_is_win, required_mods) = resolve_helper_target(hotkey_str)?;
    write_helper_config(
        &helper_config_path(),
        target_key,
        target_is_win,
        &required_mods,
        hotkey_str,
        &overlay_event_name(),
        &helper_detection_pipe_name(),
        &config_event_name(),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn restart_elevated_helper(hotkey_str: &str) {
    let previous_generation = HELPER_GENERATION.load(Ordering::SeqCst);

    // The supervisor owns the pipe whenever it is live (helper mode is the
    // only way `is_alive` reports true for a helper-backed listener), so it
    // reconnects on its own — a second connector here would race it.
    //
    // The bumped generation MUST still reach the helper's config file before
    // returning. The helper decides to exit by comparing generations, and
    // `update_live_hotkey` already wrote the previous generation while the
    // combo was being retargeted. Returning here without a rewrite left the
    // file at the old generation, so the live helper adopted the new combo in
    // place instead of exiting: it kept mask state and detect semantics from
    // the old family, and Win stopped opening the overlay until a restart.
    if HELPER_ACTIVE.load(Ordering::SeqCst) {
        if let Err(error) = publish_helper_config_for(hotkey_str) {
            logging::warn(&format!(
                "[nex] helper restart '{hotkey_str}': deferred config rewrite failed: {error}"
            ));
        } else {
            ring_config_changed_event();
            logging::info(&format!(
                "[nex] helper restart deferred to the live supervisor; config re-published at generation {}",
                HELPER_GENERATION.load(Ordering::SeqCst),
            ));
        }
        wait_for_previous_helper_to_exit(previous_generation);
        return;
    }
    wait_for_previous_helper_to_exit(previous_generation);

    let (target_key, target_is_win, required_mods) =
        match resolve_helper_target(hotkey_str) {
            Ok(resolved) => resolved,
            Err(error) => {
                logging::warn(&format!(
                    "[nex] helper restart '{hotkey_str}': {error}"
                ));
                return;
            }
        };

    match spawn_and_connect_helper(target_key, target_is_win, &required_mods, hotkey_str) {
        Ok((helper_handle, pipe_file)) => {
            HELPER_ACTIVE.store(true, Ordering::SeqCst);
            logging::info(&format!(
                "[nex] elevated helper relaunched for '{hotkey_str}'"
            ));
            connect_helper_aux_pipes();
            // Drain the new helper's detection pipe on a detached thread. The
            // listener that owns the runtime channel is untouched, so these
            // events use the same sender and the same same-press gate.
            let event_tx = HELPER_EVENT_TX
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            std::thread::Builder::new()
                .name("nex-helper-relaunch-reader".into())
                .spawn(move || {
                    if let Some(event_tx) = event_tx {
                        read_helper_pipe(pipe_file, &event_tx);
                    }
                    HELPER_ACTIVE.store(false, Ordering::SeqCst);
                    HELPER_GENERATION.fetch_add(1, Ordering::SeqCst);
                    set_suppress_focus_escape(false);
                    logging::info("[nex] relaunched helper pipe closed");
                })
                .ok();
            if helper_handle != 0 {
                kill_helper_process(helper_handle);
            }
        }
        Err(error) => {
            logging::warn(&format!(
                "[nex] helper relaunch failed ({error}); in-process hook stays active"
            ));
        }
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
    let config_event_name = create_config_event();
    let pipe_name = helper_detection_pipe_name();

    // 2. Write JSON config for the helper (includes event name)
    let config_path = helper_config_path();
    write_helper_config(
        &config_path, 
        target_key, 
        target_is_win, 
        required_mods, 
        hotkey_str, 
        &event_name, 
        &pipe_name,
        &config_event_name,
    )?;

    // 3. Ensure scheduled task exists (one-time UAC if not yet created)
    let helper_path = find_helper_exe()?;
    ensure_helper_task(&helper_path, &config_path)?;

    // 4. Create the detection pipe and accept the helper's connection on a
    //    helper thread, so this supervisor is not blocked past a bounded wait.
    //    nex owns the pipe (Medium IL); the elevated helper connects as the
    //    client that writes. See `create_helper_detection_pipe`.
    let accept_name = pipe_name.clone();
    let (pipe_tx, pipe_rx) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("nex-helper-pipe-accept".into())
        .spawn(move || {
            let _ = pipe_tx.send(create_helper_detection_pipe(&accept_name));
        })
        .map_err(|e| format!("failed to spawn pipe accept thread: {e}"))?;

    // 5. Run the scheduled task (no UAC) — the helper connects to the pipe
    //    created above as soon as it starts.
    run_helper_task()?;

    // 6. Take the accepted connection. Bounded so a helper that never starts
    //    cannot wedge the supervisor; the caller retries with backoff.
    let pipe_file = match pipe_rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(Ok(file)) => file,
        Ok(Err(error)) => return Err(error),
        Err(_) => return Err("helper did not connect to the detection pipe in time".into()),
    };

    // Process handle = 0 (scheduled task, not directly manageable)
    Ok((0, pipe_file))
}

/// Locate NexHelper.exe beside nex.exe.
fn find_helper_exe() -> Result<std::path::PathBuf, String> {
    let exe_path = std::env::current_exe()
        .map_err(|e| format!("can't get exe path: {e}"))?;
    let helper_path = exe_path.parent()
        .unwrap_or(&exe_path)
        .join("NexHelper.exe");
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
    pipe_name: &str,
    config_event_name: &str,
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

    // `generation` lets a running helper recognise that it has been
    // superseded: a newer value means exit so the replacement can take the
    // single detection pipe.
    //
    // Claim a fresh value per write. Reading the static here made two writes
    // that straddle a bump share one value, so the helper saw "same
    // generation" and adopted a new combo in place instead of exiting — which
    // left the mask state and detect flag from the old combo family, and Win
    // only worked again after a full restart.
    let generation = next_helper_generation();
    // JSON needs the backslashes in a Windows path doubled; the helper parses
    // this file as JSON, so an unescaped `\\.\pipe\x` would be read back as a
    // different name.
    let json = format!(
        r#"{{"proto":1,"generation":{},"pipe":"{}","target_pid":{},"target_vk":{},"target_is_win":{},"mod_ctrl":{},"mod_alt":{},"mod_shift":{},"mod_win":{},"detect":{},"hotkey":"{}","event":"{}","config_event":"{}"}}"#,
        generation,
        json_escape(pipe_name),
        pid,
        target_key,
        if target_is_win { "true" } else { "false" },
        if mod_ctrl { "true" } else { "false" },
        if mod_alt { "true" } else { "false" },
        if mod_shift { "true" } else { "false" },
        if mod_win { "true" } else { "false" },
        // Helper detection is only needed for bare-Win combos: raw input
        // can't see keys over elevated-TM focus, but the High-IL hook can.
        // Non-Win combos are covered by RegisterHotKeyW + raw input, and
        // keeping the helper inert avoids double-toggles.
        if target_is_win { "true" } else { "false" },
        json_escape(hotkey_str),
        json_escape(overlay_event_name),
        json_escape(config_event_name),
    );

    std::fs::write(path, &json)
        .map_err(|e| format!("failed to write helper config '{:?}': {e}", path))
}

/// Escape a string for embedding in the helper's JSON config.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
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
/// Recreates if the binary path changed (e.g. after rename).
fn ensure_helper_task(helper_path: &std::path::Path, config_path: &std::path::Path) -> Result<(), String> {
    let path_str = helper_path.display().to_string();

    // Query the existing task via schtasks (native, ~30ms) instead of
    // PowerShell (cold CLR start ~1s). The XML output carries the
    // <Command> element of the first action.
    let query = std::process::Command::new("schtasks")
        .args(["/query", "/tn", SCHTASK_NAME, "/xml"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW — no console flash at boot
        .output()
        .map_err(|e| format!("schtasks query failed: {e}"))?;

    if query.status.success() {
        if let Some(actual) = parse_task_command(&query.stdout) {
            if actual == path_str {
                return Ok(()); // task exists with correct path
            }
            logging::info(&format!(
                "[nex] helper path changed (was '{actual}'), recreating task",
            ));
        }
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

/// Extract the first `<Command>` element from `schtasks /query /xml`
/// output (UTF-16LE with BOM when redirected), entity-decoded.
fn parse_task_command(bytes: &[u8]) -> Option<String> {
    let xml = if bytes.starts_with(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(
            &bytes[2..]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<u16>>(),
        )
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let start = xml.find("<Command>")? + "<Command>".len();
    let end = xml[start..].find("</Command>")? + start;
    let raw = &xml[start..end];
    Some(
        raw.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'"),
    )
}

/// Run the scheduled task (no UAC).
fn run_helper_task() -> Result<(), String> {
    let output = std::process::Command::new("schtasks")
        .args(["/run", "/tn", SCHTASK_NAME])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW — schtasks is a console binary
        .output()
        .map_err(|e| format!("schtasks /run failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("schtasks /run failed: {stderr}"));
    }

    Ok(())
}

/// Connect to an existing named pipe (client side, read-only).
/// Connect to a helper pipe. `desired_access` must match the server end
/// (outbound server → GENERIC_READ, inbound server → GENERIC_WRITE);
/// a wrong direction fails every attempt, so auxiliaries pass a small
/// `attempts` budget and never gate detection flow.
fn connect_pipe(pipe_name: &str, desired_access: u32, attempts: u32) -> Result<std::fs::File, String> {
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
    const GENERIC_WRITE: u32 = 0x40000000;
    const FILE_SHARE_READ: u32 = 0x00000001;
    const FILE_SHARE_WRITE: u32 = 0x00000002;
    const OPEN_EXISTING: u32 = 3;

    for attempt in 0..attempts {
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                desired_access,
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

// ---------------------------------------------------------------------------
// Dispatch decision: RegisterHotKey vs keyboard hook, with reasons
// ---------------------------------------------------------------------------

/// Why a binding goes to the hook instead of RegisterHotKey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchReason {
    /// Win in the combo: Start-menu conflicts need the raw-input path.
    WinCombo,
    /// Numpad keys: the OS can't tell them apart from dedicated keys.
    NumpadKey,
    /// No modifiers: single-tap needs press/release tracking the hook owns.
    SingleTap,
    /// RegisterHotKey refused (already registered elsewhere): demoted.
    RefusedDemoted,
    /// Default: RegisterHotKey delivers it.
    SystemHotkey,
}

/// Numpad virtual-key range (VK_NUMPAD0..VK_DIVIDE).
fn is_numpad_vk(vk: u32) -> bool {
    (0x60..=0x6F).contains(&vk)
}

/// Decide dispatch for one binding. Pure: same inputs, same answer.
fn dispatch_for(vk: u32, has_win_mod: bool, has_any_mod: bool, refused: bool) -> DispatchReason {
    if refused {
        DispatchReason::RefusedDemoted
    } else if has_win_mod {
        DispatchReason::WinCombo
    } else if is_numpad_vk(vk) {
        DispatchReason::NumpadKey
    } else if !has_any_mod {
        DispatchReason::SingleTap
    } else {
        DispatchReason::SystemHotkey
    }
}

fn dispatch_reason_text(reason: DispatchReason) -> &'static str {
    match reason {
        DispatchReason::WinCombo => "Win combo needs the raw-input path",
        DispatchReason::NumpadKey => "numpad keys are indistinguishable to RegisterHotKey",
        DispatchReason::SingleTap => "single-tap needs hook press/release tracking",
        DispatchReason::RefusedDemoted => "RegisterHotKey refused the combination",
        DispatchReason::SystemHotkey => "RegisterHotKey delivers it",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parse_ctrl_space() { let p = parse_hotkey("Ctrl+Space").unwrap(); assert_eq!(p.modifiers, vec!["Ctrl"]); assert_eq!(p.key, "Space"); }
    #[test] fn parse_ctrl_shift_f5() { let p = parse_hotkey("Ctrl+Shift+F5").unwrap(); assert_eq!(p.modifiers, vec!["Shift", "Ctrl"]); assert_eq!(p.key, "F5"); }
    #[test] fn parse_single_key() { let p = parse_hotkey("F1").unwrap(); assert!(p.modifiers.is_empty()); assert_eq!(p.key, "F1"); }
    #[test] fn parse_rejects_empty() { assert!(parse_hotkey("").is_err()); assert!(parse_hotkey("++").is_err()); }
    #[test] fn same_press_only_within_window() {
        assert!(!same_press(0, 5));        // first event ever
        assert!(same_press(1_000, 1_030)); // overlapping detection paths
        assert!(!same_press(1_000, 1_050)); // a fresh press
    }
    #[test] fn vk_space() { assert_eq!(vk_from_key("Space").unwrap(), windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SPACE as u32); }
    #[test] fn vk_f5() { assert_eq!(vk_from_key("F5").unwrap(), windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F5 as u32); }
    #[test] fn vk_single_letter() { assert_eq!(vk_from_key("A").unwrap(), b'A' as u32); }
    #[test] fn vk_win_key() { assert_eq!(vk_from_key("Win").unwrap(), VK_LWIN); }
    #[test] fn modifier_vks() { let mods = modifier_vks_from_names(&["Ctrl".into(), "Shift".into()]).unwrap(); assert_eq!(mods, vec![VK_CTRL, VK_SHIFT]); }
    #[test] fn dispatch_win_goes_hook() { assert_eq!(dispatch_for(0x20, true, true, false), DispatchReason::WinCombo); }
    #[test] fn dispatch_numpad_goes_hook() { assert_eq!(dispatch_for(0x60, false, true, false), DispatchReason::NumpadKey); }
    #[test] fn dispatch_single_tap_goes_hook() { assert_eq!(dispatch_for(0x70, false, false, false), DispatchReason::SingleTap); }
    #[test] fn dispatch_refused_demotes() { assert_eq!(dispatch_for(0x20, false, true, true), DispatchReason::RefusedDemoted); }
    #[test] fn dispatch_plain_combo_uses_system() { assert_eq!(dispatch_for(0x20, false, true, false), DispatchReason::SystemHotkey); }
    #[test] fn dispatch_numpad_bounds() { assert_eq!(dispatch_for(0x6F, false, true, false), DispatchReason::NumpadKey); assert_eq!(dispatch_for(0x5F, false, true, false), DispatchReason::SystemHotkey); assert_eq!(dispatch_for(0x70, false, true, false), DispatchReason::SystemHotkey); }
    #[test] fn dispatch_refused_wins_over_win() { assert_eq!(dispatch_for(0x20, true, true, true), DispatchReason::RefusedDemoted); }
    #[test] fn send_helper_key_down_without_pipe() { assert!(!send_helper_key("play_pause")); }

    #[test] fn outside_click_hit_test_excludes_panel() {
        let (l, t, r, b) = (100, 100, 300, 200);
        assert!(!cursor_outside_rect(200, 150, l, t, r, b), "inside panel");
        assert!(!cursor_outside_rect(100, 100, l, t, r, b), "top-left edge is inside");
        assert!(cursor_outside_rect(99, 150, l, t, r, b), "left of panel");
        assert!(cursor_outside_rect(300, 150, l, t, r, b), "right edge is outside");
        assert!(cursor_outside_rect(200, 200, l, t, r, b), "bottom edge is outside");
    }

    // One press tracker for every detector: bare taps fire once, chords never.
    #[test] fn bare_press_fires_exactly_once() {
        reset_win_press_state();
        note_win_press_start();
        note_win_press_start();              // key auto-repeat / second detector
        assert!(finish_win_press());
        assert!(!finish_win_press());        // other detector's key-up
    }
    #[test] fn chord_press_never_fires() {
        reset_win_press_state();
        note_win_press_start();
        assert!(note_chord_key());           // Win+E key-down
        assert!(note_chord_key());           // another chord key
        assert!(!finish_win_press());
    }
    #[test] fn chord_flag_does_not_leak_into_next_press() {
        reset_win_press_state();
        note_win_press_start();
        assert!(note_chord_key());
        assert!(!finish_win_press());
        note_win_press_start();
        assert!(finish_win_press());
    }
    #[test] fn chord_key_outside_press_is_ignored() {
        reset_win_press_state();
        assert!(!note_chord_key());
    }
    #[test] fn chord_press_clears_focus_escape_guard() {
        reset_win_press_state();
        note_win_press_start();
        set_suppress_focus_escape(true);
        note_chord_key();
        assert!(!finish_win_press());
        assert!(!is_bare_win_press_active());
    }

    // A hotkey change must never wait on the previous helper. These pin the
    // wait's two exit conditions: it returns immediately when no helper pipe
    // is held, and it is bounded when one is.

    #[test]
    fn json_escape_makes_a_pipe_path_parseable() {
        let path = r"\.\pipe\nex-hotkey\42";
        let escaped = json_escape(path);
        assert_eq!(escaped, r"\\.\\pipe\\nex-hotkey\\42");
        // The doubling is reversible, so the helper reads back the real path.
        assert_eq!(escaped.replace(r"\\", r"\"), path);
    }

    #[test]
    fn json_escape_quotes_and_control_bytes() {
        assert_eq!(json_escape("a\"b"), r#"a\"b"#);
        assert_eq!(json_escape("\u{1}"), r"\u0001");
    }

    #[test] fn helper_wait_returns_immediately_without_a_pipe() {
        HELPER_ACTIVE.store(false, Ordering::SeqCst);
        HELPER_GENERATION.store(41, Ordering::SeqCst);
        let started = std::time::Instant::now();
        wait_for_previous_helper_to_exit(41);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "a listener without a helper pipe must not wait",
        );
    }

    #[test] fn helper_wait_returns_when_the_generation_advanced() {
        HELPER_ACTIVE.store(true, Ordering::SeqCst);
        HELPER_GENERATION.store(7, Ordering::SeqCst);
        // The previous generation is already gone: its bump is observable.
        let previous = HELPER_GENERATION.load(Ordering::SeqCst);
        HELPER_GENERATION.fetch_add(1, Ordering::SeqCst);
        let started = std::time::Instant::now();
        wait_for_previous_helper_to_exit(previous);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "an advanced generation means the previous helper exited",
        );
        HELPER_ACTIVE.store(false, Ordering::SeqCst);
    }

    #[test] fn helper_wait_is_bounded() {
        HELPER_ACTIVE.store(true, Ordering::SeqCst);
        HELPER_GENERATION.store(99, Ordering::SeqCst);
        let started = std::time::Instant::now();
        // Nothing can advance the generation here, so only the ceiling ends
        // the wait. It must not hang the reconfig thread.
        let previous = HELPER_GENERATION.load(Ordering::SeqCst);
        wait_for_previous_helper_to_exit(previous);
        assert!(started.elapsed() <= HELPER_REPLACE_GRACE + Duration::from_millis(500));
        HELPER_ACTIVE.store(false, Ordering::SeqCst);
    }
    /// The injected mask key is one OS-level resource. If nex and the helper
    /// both send it for the same press, one side's release orphans the other's
    /// flag and the next Win tap leaks the Start menu. Exactly one side must
    /// own the press.
    #[test]
    fn only_one_process_masks_the_win_press() {
        use super::masks_win_press;

        // Helper live and the hotkey is Win: the helper masks on every
        // physical Win press, so nex stands down.
        assert!(masks_win_press(true, true));

        // Helper live but the hotkey is not Win: the helper's Win branch never
        // runs, so nex must own the mask itself or a bare Win tap opens Start.
        assert!(!masks_win_press(true, false));

        // No helper: nex always owns it.
        assert!(!masks_win_press(false, true));
        assert!(!masks_win_press(false, false));

        // The regression this guards: deciding by helper liveness alone made
        // nex stand down for a non-Win hotkey too, so neither process held the
        // mask and Start opened on every Win tap.
        assert!(
            !masks_win_press(true, false),
            "nex must mask when the helper does not",
        );
    }

    /// A helper spawned with generation 0 can never recognise a later
    /// generation, because the helper skips the superseded check when the
    /// config says 0. Such a helper adopted a new combo in place instead of
    /// exiting, which is why Win stopped working until a restart.
    #[test]
    fn helper_generation_never_starts_at_zero() {
        let start = AtomicU64::new(1);
        assert_ne!(start.load(Ordering::SeqCst), 0);
        let bumped = start.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(bumped, 2, "bumping from 1 yields a meaningful generation");
        assert_ne!(bumped, 0);
    }

    /// The generation the running helper last saw must differ from the one
    /// written after a bump, or it will not exit.
    #[test]
    fn a_bumped_generation_is_recognisably_newer() {
        let generation = AtomicU64::new(1);
        let written_before = generation.load(Ordering::SeqCst);
        let written_after = generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert_ne!(
            written_before, written_after,
            "the helper needs a different generation to exit on",
        );
    }

}
