#![cfg(target_os = "windows")]

use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, CREATESTRUCTW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, GetCursorPos, GetWindowLongPtrW, RegisterClassW,
    SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenu, GWLP_USERDATA, HWND_MESSAGE, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, SW_SHOW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_APP, WM_CREATE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
};

use crate::overlay::model::OverlayEvent;

const NEX_WM_TRAY_ICON: u32 = WM_APP + 18;
const TRAY_ICON_ID: u32 = 1;
const TRAY_MESSAGE_CLASS: &str = "NexTrayMessageWindow";

const TRAY_MENU_SHOW: u32 = 41001;
const TRAY_MENU_OPEN_CONFIG: u32 = 41002;
const TRAY_MENU_CHECK_UPDATES: u32 = 41003;
const TRAY_MENU_GAME_MODE: u32 = 41004;
const TRAY_MENU_QUIT: u32 = 41005;

fn to_wide(s: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    wide.push(0);
    wide
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

fn tooltip_text(game_mode: bool, hotkey_issue: bool) -> String {
    if hotkey_issue {
        "Nex (hotkey issue)"
    } else if game_mode {
        "Nex (Game Mode)"
    } else {
        "Nex Launcher"
    }
    .to_string()
}

struct TrayState {
    event_tx: Sender<OverlayEvent>,
    config_path: String,
    game_mode_enabled: bool,
    hotkey_issue_active: bool,
}

pub(crate) struct TrayIcon {
    message_hwnd: HWND,
    icon_handle: isize,
    icon_added: bool,
    state: Arc<Mutex<TrayState>>,
}

// SAFETY: HWND is a Windows handle that can be safely sent between
// threads. All mutable state is behind Arc<Mutex<>>.
unsafe impl Send for TrayIcon {}

impl TrayIcon {
    pub(crate) fn create(
        event_tx: Sender<OverlayEvent>,
        config_path: &str,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(TrayState {
            event_tx,
            config_path: config_path.to_string(),
            game_mode_enabled: false,
            hotkey_issue_active: false,
        }));

        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class_name = to_wide(TRAY_MESSAGE_CLASS);

        let mut wnd_class: WNDCLASSW = unsafe { std::mem::zeroed() };
        wnd_class.lpfnWndProc = Some(tray_wnd_proc);
        wnd_class.hInstance = instance;
        wnd_class.lpszClassName = class_name.as_ptr();

        let atom = unsafe { RegisterClassW(&wnd_class) };
        if atom == 0 {
            let error = unsafe { GetLastError() };
            if error != 1410 {
                return Err(format!("RegisterClassW(tray message) failed: {error}"));
            }
        }

        let state_ptr = Arc::as_ptr(&state) as *mut std::ffi::c_void;

        let msg_hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                instance,
                state_ptr,
            )
        };
        if msg_hwnd.is_null() {
            return Err(format!(
                "CreateWindowExW(HWND_MESSAGE) failed: {}",
                unsafe { GetLastError() }
            ));
        }

        let icon_handle = Self::load_icon()?;

        // NIM_ADD — only succeeds after the message window exists
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = msg_hwnd;
        data.uID = TRAY_ICON_ID;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = NEX_WM_TRAY_ICON;
        data.hIcon = icon_handle as _;
        copy_wide_text_into_buffer(&mut data.szTip, &tooltip_text(false, false));

        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data as *const NOTIFYICONDATAW) };
        if ok == 0 {
            // Destroy the message window but keep the icon handle in
            // the returned struct so Drop can destroy it.
            unsafe { DestroyWindow(msg_hwnd) };
            return Err(format!(
                "Shell_NotifyIconW(NIM_ADD) failed: {}",
                unsafe { GetLastError() }
            ));
        }

        Ok(Self {
            message_hwnd: msg_hwnd,
            icon_handle,
            icon_added: true,
            state,
        })
    }

    pub(crate) fn set_game_mode(&self, enabled: bool) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.game_mode_enabled = enabled;
        self.update_tray_tooltip(&guard);
    }

    pub(crate) fn set_hotkey_issue(&self, active: bool) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.hotkey_issue_active = active;
        self.update_tray_tooltip(&guard);
    }

    fn load_icon() -> Result<isize, String> {
        let exe =
            std::env::current_exe().map_err(|error| format!("current_exe failed: {error}"))?;
        let wide: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut small_icon = std::ptr::null_mut();
        let mut large_icon = std::ptr::null_mut();
        let extracted =
            unsafe { ExtractIconExW(wide.as_ptr(), 0, &mut large_icon, &mut small_icon, 1) };
        if !large_icon.is_null() {
            unsafe { DestroyIcon(large_icon) };
        }
        if extracted == 0 || small_icon.is_null() {
            return Err("ExtractIconExW did not return a small icon".to_string());
        }
        Ok(small_icon as isize)
    }

    fn update_tray_tooltip(&self, guard: &TrayState) {
        if !self.icon_added {
            return;
        }
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.message_hwnd;
        data.uID = TRAY_ICON_ID;
        data.uFlags = NIF_TIP;
        copy_wide_text_into_buffer(
            &mut data.szTip,
            &tooltip_text(guard.game_mode_enabled, guard.hotkey_issue_active),
        );
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &data as *const NOTIFYICONDATAW);
        }
    }

    fn remove_tray_icon(&mut self) {
        if self.icon_added {
            self.icon_added = false;
            let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
            data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = self.message_hwnd;
            data.uID = TRAY_ICON_ID;
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &data as *const NOTIFYICONDATAW);
            }
        }
        if self.icon_handle != 0 {
            unsafe {
                DestroyIcon(self.icon_handle as _);
            }
            self.icon_handle = 0;
        }
        if !self.message_hwnd.is_null() {
            let hwnd = self.message_hwnd;
            self.message_hwnd = std::ptr::null_mut();
            unsafe {
                DestroyWindow(hwnd);
            }
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove_tray_icon();
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let create_struct = lparam as *const CREATESTRUCTW;
        let state_ptr = unsafe { (*create_struct).lpCreateParams };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize) };
        return 0;
    }
    if msg == NEX_WM_TRAY_ICON {
        if hwnd.is_null() {
            return 0;
        }
        let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        if state_ptr == 0 {
            return 0;
        }
        let state: &Mutex<TrayState> = unsafe { &*(state_ptr as *const Mutex<TrayState>) };

        let lp = lparam as u32;
        if lp == WM_RBUTTONUP {
            // Snapshot what we need *before* TrackPopupMenu blocks,
            // so the mutex is not held across the modal menu loop.
            let snapshot = {
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                MenuSnapshot {
                    event_tx: guard.event_tx.clone(),
                    config_path: guard.config_path.clone(),
                    game_mode_enabled: guard.game_mode_enabled,
                }
            };
            show_context_menu(hwnd, &snapshot);
        } else if lp == WM_LBUTTONUP || lp == WM_LBUTTONDBLCLK {
            let _ = state.lock().unwrap_or_else(|e| e.into_inner()).event_tx.send(OverlayEvent::ExternalShow);
        }
        return 0;
    }
    if msg == WM_DESTROY {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        return 0;
    }
    // SAFETY: hwnd is the tray window handle, fall through to default proc
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

struct MenuSnapshot {
    event_tx: Sender<OverlayEvent>,
    config_path: String,
    game_mode_enabled: bool,
}

fn show_context_menu(hwnd: HWND, s: &MenuSnapshot) {
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
        AppendMenuW(menu, MF_STRING, TRAY_MENU_SHOW as usize, open_text.as_ptr());
        AppendMenuW(
            menu,
            MF_STRING,
            TRAY_MENU_OPEN_CONFIG as usize,
            config_text.as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING,
            TRAY_MENU_CHECK_UPDATES as usize,
            updates_text.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(
            menu,
            MF_STRING
                | if s.game_mode_enabled {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
            TRAY_MENU_GAME_MODE as usize,
            game_mode_text.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(
            menu,
            MF_STRING,
            TRAY_MENU_QUIT as usize,
            quit_text.as_ptr(),
        );
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
    } as u32;

    // Required by Win32: post a benign message so the menu's
    // internal message loop exits cleanly. Without this the
    // menu can appear frozen / uninteractive.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, 0, 0, 0);
    }

    match selected {
        0 => {}
        TRAY_MENU_SHOW => {
            let _ = s.event_tx.send(OverlayEvent::ExternalShow);
        }
        TRAY_MENU_OPEN_CONFIG => {
            let config_path = to_wide(&s.config_path);
            unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    to_wide("open").as_ptr(),
                    config_path.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOW,
                );
            }
        }
        TRAY_MENU_CHECK_UPDATES => {
            let _ = s.event_tx.send(OverlayEvent::TrayCheckForUpdates);
        }
        TRAY_MENU_GAME_MODE => {
            let _ = s.event_tx.send(OverlayEvent::TrayToggleGameMode);
        }
        TRAY_MENU_QUIT => {
            let _ = s.event_tx.send(OverlayEvent::ExternalQuit);
        }
        _ => {}
    }

    unsafe {
        DestroyMenu(menu);
    }
}
