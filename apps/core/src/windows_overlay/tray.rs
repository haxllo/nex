use windows_sys::Win32::Foundation::{GetLastError, HWND, POINT};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, PostMessageW,
    SetForegroundWindow, TrackPopupMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_COMMAND,
};

use crate::windows_overlay::state::OverlayShellState;
use crate::windows_overlay::types::*;
use crate::windows_overlay::window::path_to_wide;

fn tray_tooltip_text(game_mode_enabled: bool, hotkey_issue_active: bool) -> String {
    let base = if hotkey_issue_active {
        "Nex (hotkey issue)"
    } else if game_mode_enabled {
        "Nex (Game Mode)"
    } else {
        "Nex Launcher"
    };
    base.to_string()
}
fn copy_wide_text_into_buffer(buffer: &mut [u16], text: &str) {
    if buffer.is_empty() {
        return;
    }
    let wide = to_wide(text);
    let copy_len = wide
        .len()
        .saturating_sub(1)
        .min(buffer.len().saturating_sub(1));
    buffer[..copy_len].copy_from_slice(&wide[..copy_len]);
    buffer[copy_len] = 0;
}

fn build_tray_icon_data(hwnd: HWND, state: &OverlayShellState) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ICON_ID;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = NEX_WM_TRAY_ICON;
    data.hIcon = state.tray_icon_handle as _;
    copy_wide_text_into_buffer(
        &mut data.szTip,
        &tray_tooltip_text(state.game_mode_enabled, state.hotkey_issue_active),
    );
    data
}

pub(crate) fn load_tray_icon_handle() -> Result<isize, String> {
    let exe = std::env::current_exe().map_err(|error| format!("current_exe failed: {error}"))?;
    let wide = path_to_wide(&exe);
    let mut small_icon = std::ptr::null_mut();
    let extracted =
        unsafe { ExtractIconExW(wide.as_ptr(), 0, std::ptr::null_mut(), &mut small_icon, 1) };
    if extracted == 0 || small_icon.is_null() {
        return Err("ExtractIconExW did not return a small icon".to_string());
    }
    Ok(small_icon as isize)
}

pub(crate) fn add_tray_icon(hwnd: HWND, state: &mut OverlayShellState) -> Result<(), String> {
    let data = build_tray_icon_data(hwnd, state);
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data as *const NOTIFYICONDATAW) };
    if ok == 0 {
        return Err(format!(
            "Shell_NotifyIconW(NIM_ADD) failed with error {}",
            unsafe { GetLastError() }
        ));
    }
    state.tray_icon_added = true;
    Ok(())
}

pub(crate) fn update_tray_icon(hwnd: HWND, state: &OverlayShellState) -> Result<(), String> {
    if !state.tray_icon_added {
        return Ok(());
    }
    let data = build_tray_icon_data(hwnd, state);
    let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data as *const NOTIFYICONDATAW) };
    if ok == 0 {
        return Err(format!(
            "Shell_NotifyIconW(NIM_MODIFY) failed with error {}",
            unsafe { GetLastError() }
        ));
    }
    Ok(())
}

pub(crate) fn remove_tray_icon(hwnd: HWND, state: &mut OverlayShellState) {
    if state.tray_icon_added {
        let data = build_tray_icon_data(hwnd, state);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data as *const NOTIFYICONDATAW);
        }
        state.tray_icon_added = false;
    }
    if state.tray_icon_handle != 0 {
        unsafe {
            DestroyIcon(state.tray_icon_handle as _);
        }
        state.tray_icon_handle = 0;
    }
}

pub(crate) fn show_tray_context_menu(hwnd: HWND, state: &OverlayShellState) {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }

    let open_text = to_wide("Open Nex");
    let config_text = to_wide("Open Config");
    let updates_text = to_wide("Check for Updates");
    let game_mode_text = to_wide("Game Mode");
    let quit_text = to_wide("Quit");
    unsafe {
        AppendMenuW(menu, MF_STRING, TRAY_MENU_SHOW, open_text.as_ptr());
        AppendMenuW(menu, MF_STRING, TRAY_MENU_OPEN_CONFIG, config_text.as_ptr());
        AppendMenuW(
            menu,
            MF_STRING,
            TRAY_MENU_CHECK_UPDATES,
            updates_text.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(
            menu,
            MF_STRING
                | if state.game_mode_enabled {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            TRAY_MENU_GAME_MODE,
            game_mode_text.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, TRAY_MENU_QUIT, quit_text.as_ptr());
    }

    let mut cursor = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut cursor as *mut POINT);
        SetForegroundWindow(hwnd);
    }
    let selected = unsafe {
        TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            std::ptr::null(),
        )
    };
    if selected != 0 {
        unsafe {
            PostMessageW(hwnd, WM_COMMAND, selected as usize, 0);
        }
    }
    unsafe {
        DestroyMenu(menu);
    }
}
