//! Platform glue for the WebView2 overlay. Provides:
//!   * system theme detection (light vs dark) via the Windows
//!     `AppsUseLightTheme` registry key,
//!   * the legacy single-instance signal helpers that look up an
//!     existing `nex.exe` overlay by class name and post a custom
//!     `WM_APP+_` message.
//!
//! The hotkey registration, tray icon, and `RegisterHotKey`
//! subscription live in `shim.rs` because they are driven by the
//! runtime's overlay host, not the WebView itself.

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetWindowThreadProcessId, PostMessageW,
    RegisterWindowMessageW,
};

use crate::overlay::model::OverlayEvent;
use crate::overlay::model::Theme;

const CLASS_NAME: &str = "NexOverlayWindowClass";
const SIGNAL_SHOW_REGISTERED: &str = "Nex.ExternalShow.v1";
const SIGNAL_QUIT_REGISTERED: &str = "Nex.ExternalQuit.v1";

pub(crate) fn detect_system_theme() -> Theme {
    let key = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = to_wide("AppsUseLightTheme");
    let mut data: u32 = 0;
    let mut data_size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut c_void,
            &mut data_size,
        )
    };
    if status == 0 && data == 1 {
        Theme::Light
    } else {
        Theme::Dark
    }
}

/// Windows accent color as `#RRGGBB`.
///
/// Prefers `DwmGetColorizationColor` (always live — tracks both custom
/// picks and auto/wallpaper-derived accents) and falls back to the
/// `AccentColor` registry DWORD (ABGR). The registry key keeps the last
/// *custom* color and goes stale when the accent is set to Automatic,
/// which is why the DWM API takes priority.
pub(crate) fn detect_accent_color() -> Option<String> {
    let mut colorization: u32 = 0;
    let mut opaque: i32 = 0;
    let status = unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(
            &mut colorization,
            &mut opaque,
        )
    };
    if status == 0 {
        // ColorizationColor is 0xAARRGGBB — drop the alpha byte.
        let r = (colorization >> 16) & 0xFF;
        let g = (colorization >> 8) & 0xFF;
        let b = colorization & 0xFF;
        return Some(format!("#{r:02X}{g:02X}{b:02X}"));
    }
    detect_accent_color_registry()
}

/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor` (DWORD ABGR) as `#RRGGBB`.
fn detect_accent_color_registry() -> Option<String> {
    let key = to_wide("Software\\Microsoft\\Windows\\DWM");
    let value = to_wide("AccentColor");
    let mut data: u32 = 0;
    let mut data_size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut c_void,
            &mut data_size,
        )
    };
    if status != 0 {
        return None;
    }
    // ABGR → RRGGBB.
    let b = data & 0xFF;
    let g = (data >> 8) & 0xFF;
    let r = (data >> 16) & 0xFF;
    Some(format!("#{r:02X}{g:02X}{b:02X}"))
}

pub fn is_instance_window_present() -> bool {
    let class = to_wide(CLASS_NAME);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    !hwnd.is_null()
}

pub fn signal_existing_instance_show(target_pids: &[u32]) -> Result<bool, String> {
    let hwnd = if target_pids.is_empty() {
        let class = to_wide(CLASS_NAME);
        unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) }
    } else {
        find_hwnd_by_pids(target_pids)
    };
    if hwnd.is_null() {
        return Ok(false);
    }
    let msg_id = unsafe { RegisterWindowMessageW(to_wide(SIGNAL_SHOW_REGISTERED).as_ptr()) };
    if msg_id == 0 {
        return Err("RegisterWindowMessageW(show) failed".to_string());
    }
    let ok = unsafe { PostMessageW(hwnd, msg_id, 0, 0) };
    Ok(ok != 0)
}

pub fn signal_existing_instance_quit(target_pids: &[u32]) -> Result<bool, String> {
    let hwnd = if target_pids.is_empty() {
        let class = to_wide(CLASS_NAME);
        unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) }
    } else {
        find_hwnd_by_pids(target_pids)
    };
    if hwnd.is_null() {
        return Ok(false);
    }
    let msg_id = unsafe { RegisterWindowMessageW(to_wide(SIGNAL_QUIT_REGISTERED).as_ptr()) };
    if msg_id == 0 {
        return Err("RegisterWindowMessageW(quit) failed".to_string());
    }
    let ok = unsafe { PostMessageW(hwnd, msg_id, 0, 0) };
    Ok(ok != 0)
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    wide.push(0);
    wide
}

struct EnumCtx {
    class_wide: Vec<u16>,
    target_pids: Vec<u32>,
    found_hwnd: HWND,
}

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: isize) -> i32 {
    // SAFETY: lparam is always the *mut EnumCtx we passed from find_hwnd_by_pids.
    let ctx = unsafe { &mut *(lparam as *mut EnumCtx) };
    let mut class_buf = [0u16; 128];
    // SAFETY: hwnd is a valid top-level window handle from EnumWindows.
    let len = unsafe { GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    // Compare lengths first (class_wide includes null terminator, GetClassNameW does not)
    if len <= 0 || (len as usize) != ctx.class_wide.len() - 1 {
        return 1;
    }
    if class_buf[..len as usize] != ctx.class_wide[..len as usize] {
        return 1;
    }
    let mut pid: u32 = 0;
    // SAFETY: hwnd is valid; pid out-pointer is stack-allocated.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if ctx.target_pids.contains(&pid) {
        ctx.found_hwnd = hwnd;
        return 0; // stop enumeration
    }
    1
}

/// Find the first top-level window whose class matches `CLASS_NAME` and
/// whose owning process is in `target_pids`. Returns null HWND if none.
fn find_hwnd_by_pids(target_pids: &[u32]) -> HWND {
    let mut ctx = EnumCtx {
        class_wide: to_wide(CLASS_NAME),
        target_pids: target_pids.to_vec(),
        found_hwnd: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(Some(enum_windows_callback), &mut ctx as *mut EnumCtx as isize);
    }
    ctx.found_hwnd
}

/// Map a Win32 hotkey ID to the legacy `OverlayEvent` hotkey ID. The
/// legacy module used a single hard-coded `1` for the primary
/// `Ctrl+Space` hotkey, so we just return `1` here.
pub(crate) fn hotkey_id_for(_vk: u32) -> i32 {
    1
}

/// Suppress a lint about unused `OverlayEvent` and `HWND` — they are
/// imported because the next phase will add functions that need
/// them.
#[allow(dead_code)]
fn _phantom(_e: OverlayEvent, _h: HWND) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_system_theme_returns_a_theme() {
        let _ = detect_system_theme();
    }

    #[test]
    fn instance_signal_handles_absent_window() {
        assert!(!is_instance_window_present());
        let result = signal_existing_instance_show(&[]);
        assert!(matches!(result, Ok(false)));
        let result = signal_existing_instance_quit(&[]);
        assert!(matches!(result, Ok(false)));
    }
}
