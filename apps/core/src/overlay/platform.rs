//! Platform glue for the Iced overlay. Provides:
//!   * system theme detection (light vs dark) via the Windows
//!     `AppsUseLightTheme` registry key,
//!   * the legacy single-instance signal helpers that look up an
//!     existing `nex.exe` overlay by class name and post a custom
//!     `WM_APP+_` message.
//!
//! The hotkey registration, tray icon, and `RegisterHotKey`
//! subscription live in `shim.rs` because they are driven by the
//! Iced runtime.

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, RegisterWindowMessageW,
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

pub fn is_instance_window_present() -> bool {
    let class = to_wide(CLASS_NAME);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    !hwnd.is_null()
}

pub fn signal_existing_instance_show() -> Result<bool, String> {
    let class = to_wide(CLASS_NAME);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if hwnd.is_null() {
        return Ok(false);
    }
    let msg_id = unsafe { RegisterWindowMessageW(to_wide(SIGNAL_SHOW_REGISTERED).as_ptr()) };
    if msg_id == 0 {
        return Err("RegisterWindowMessageW(show) failed".to_string());
    }
    let ok = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, msg_id, 0, 0) };
    Ok(ok != 0)
}

pub fn signal_existing_instance_quit() -> Result<bool, String> {
    let class = to_wide(CLASS_NAME);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if hwnd.is_null() {
        return Ok(false);
    }
    let msg_id = unsafe { RegisterWindowMessageW(to_wide(SIGNAL_QUIT_REGISTERED).as_ptr()) };
    if msg_id == 0 {
        return Err("RegisterWindowMessageW(quit) failed".to_string());
    }
    let ok = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, msg_id, 0, 0) };
    Ok(ok != 0)
}

fn to_wide(s: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    wide.push(0);
    wide
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
        let result = signal_existing_instance_show();
        assert!(matches!(result, Ok(false)));
        let result = signal_existing_instance_quit();
        assert!(matches!(result, Ok(false)));
    }
}
