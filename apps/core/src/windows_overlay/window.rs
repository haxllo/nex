use std::ffi::c_void;
use std::path::Path;
use std::time::Instant;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AddFontResourceExW, CreateFontW, CreateSolidBrush, DeleteObject, GetDC, GetTextFaceW,
    InvalidateRect, ReleaseDC, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, DEFAULT_CHARSET, FF_DONTCARE, FR_PRIVATE, OPAQUE, OUT_DEFAULT_PRECIS,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetForegroundWindow,
    GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, IsChild, KillTimer, LoadCursorW, PeekMessageW, PostMessageW, PostQuitMessage,
    RegisterClassW, SendMessageW, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, CREATESTRUCTW,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_MULTILINE, GWLP_USERDATA,
    GWLP_WNDPROC, HWND_TOPMOST, IDC_ARROW, LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY,
    LBS_OWNERDRAWFIXED, LB_ADDSTRING, LB_GETCOUNT, LB_GETCURSEL, LB_GETTOPINDEX, LB_RESETCONTENT,
    LB_SETCURSEL, LB_SETTOPINDEX, LWA_ALPHA, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WM_ACTIVATE, WM_CLOSE, WM_COMMAND, WM_CREATE,
    WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM, WM_HOTKEY,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_MEASUREITEM, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY,
    WM_PAINT, WM_RBUTTONUP, WM_SETFONT, WM_SETREDRAW, WM_SIZE, WM_TIMER, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
};

use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT};

use crate::windows_overlay::animation::{
    apply_window_state, blend_color, cancel_window_animation,
    complete_window_animation_if_running, hide_overlay_immediate, results_content_animation_tick,
    start_window_animation, window_animation_tick,
};
use crate::windows_overlay::icon_cache::{
    cancel_icon_cache_idle_cleanup, clear_icon_cache, configure_runtime_performance_tuning,
    insert_icon_cache_entry, log_memory_snapshot, schedule_icon_cache_idle_cleanup,
};
use crate::windows_overlay::icon_loader;
use crate::windows_overlay::input;
use crate::windows_overlay::input::sync_help_hover_with_cursor;
use crate::windows_overlay::layout::{
    apply_edit_text_rect, apply_rounded_corners_hwnd, cleanup_state_resources,
    initial_visible_row_count, layout_children, row_index_for_result_index, row_result_index,
    target_top_index_for_selection, try_enable_dwm_rounded_corners,
};
use crate::windows_overlay::painting::{
    command_badge_animation_tick, draw_list_row, draw_panel_background, handle_wheel_input,
    hide_input_caret, is_cursor_over_window, set_uninstall_quick_mode,
};
use crate::windows_overlay::state::OverlayShellState;
use crate::windows_overlay::tray::{
    add_tray_icon, load_tray_icon_handle, remove_tray_icon, show_tray_context_menu,
    update_tray_icon,
};
use crate::windows_overlay::types::*;

// Type/constant aliases not in windows-sys 0.59
type HMENU = *mut core::ffi::c_void;
const EM_SETSEL: u32 = 0x00B1;
const EN_CHANGE: u32 = 0x0300;
const LBN_DBLCLK: u32 = 0x0005;

// Helper to fetch mut state (forwarded from state module for convenience)
fn state_for(hwnd: HWND) -> Option<&'static mut OverlayShellState> {
    crate::windows_overlay::state::state_for(hwnd)
}

impl NativeOverlayShell {
    pub fn create() -> Result<Self, String> {
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class_name = class_name_wide();

        let mut class: WNDCLASSW = unsafe { std::mem::zeroed() };
        // Use only custom rounded region + custom stroke; class drop shadow can add
        // a rectangular outer contour that fights the panel shape.
        class.style = CS_HREDRAW | CS_VREDRAW;
        class.lpfnWndProc = Some(overlay_wnd_proc);
        class.hInstance = instance;
        class.hCursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
        class.hbrBackground = std::ptr::null_mut();
        class.lpszClassName = class_name.as_ptr();

        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            let error = unsafe { GetLastError() };
            if error != 1410 {
                return Err(format!("RegisterClassW failed with error {error}"));
            }
        }

        let state = Box::new(OverlayShellState::default());
        let state_ptr = Box::into_raw(state);

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                class_name.as_ptr(),
                to_wide(WINDOW_TITLE).as_ptr(),
                WS_POPUP | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WINDOW_WIDTH,
                COMPACT_HEIGHT,
                std::ptr::null_mut(),
                0 as HMENU,
                instance,
                state_ptr as *mut c_void,
            )
        };

        if hwnd.is_null() {
            unsafe {
                let _ = Box::from_raw(state_ptr);
            }
            let error = unsafe { GetLastError() };
            return Err(format!("CreateWindowExW failed with error {error}"));
        }

        let shell = Self { hwnd };
        shell.center_window();
        shell.apply_rounded_corners();
        shell.hide_immediate();
        if let Err(error) = shell.initialize_tray_icon() {
            crate::logging::warn(&format!("[nex] tray icon init warning: {error}"));
        }
        Ok(shell)
    }

    pub fn is_visible(&self) -> bool {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.hwnd) != 0 }
    }

    pub fn has_focus(&self) -> bool {
        let fg = unsafe { GetForegroundWindow() };
        if fg == self.hwnd {
            return true;
        }
        unsafe { IsChild(self.hwnd, fg) != 0 }
    }

    pub fn show_and_focus(&self) {
        cancel_icon_cache_idle_cleanup(self.hwnd);
        let running_anim = state_for(self.hwnd)
            .and_then(|s| s.window_anim.as_ref())
            .is_some();
        if running_anim {
            cancel_window_animation(self.hwnd);
            unsafe {
                SetLayeredWindowAttributes(self.hwnd, 0, OVERLAY_ALPHA_OPAQUE, LWA_ALPHA);
            }
        }
        self.center_window();
        self.ensure_compact_state();
        self.animate_show();
        unsafe {
            SetForegroundWindow(self.hwnd);
        }
        self.focus_input_and_select_all();
        log_memory_snapshot("overlay_show");
    }

    pub fn focus_input_and_select_all(&self) {
        if let Some(state) = state_for(self.hwnd) {
            unsafe {
                SetFocus(state.edit_hwnd);
                SendMessageW(state.edit_hwnd, EM_SETSEL, 0, -1);
            }
            hide_input_caret(state.edit_hwnd);
        }
    }

    pub fn hide(&self) {
        self.animate_hide();
        schedule_icon_cache_idle_cleanup(self.hwnd);
    }

    pub fn hide_now(&self) {
        self.animate_hide();
        schedule_icon_cache_idle_cleanup(self.hwnd);
    }

    pub fn query_text(&self) -> String {
        let Some(state) = state_for(self.hwnd) else {
            return String::new();
        };

        let length = unsafe { GetWindowTextLengthW(state.edit_hwnd) };
        let mut text = if length <= 0 {
            String::new()
        } else {
            let mut buffer = vec![0_u16; (length as usize) + 1];
            let copied = unsafe {
                GetWindowTextW(state.edit_hwnd, buffer.as_mut_ptr(), buffer.len() as i32)
            };
            String::from_utf16_lossy(&buffer[..(copied as usize)])
        };

        if state.command_mode_input {
            if state.command_uninstall_quick_mode {
                let suffix = text.trim_start();
                if suffix.is_empty() {
                    return ">u".to_string();
                }
                return format!(">u {suffix}");
            }
            text.insert(0, '>');
        }
        text
    }

    pub fn set_query_text(&self, query: &str) {
        let Some(state) = state_for(self.hwnd) else {
            return;
        };

        let raw = query;
        let (command_mode, mut edit_text) = if let Some(rest) = raw.strip_prefix('>') {
            (true, rest.to_string())
        } else {
            (false, raw.to_string())
        };
        let mut uninstall_quick_mode = false;
        if command_mode {
            let trimmed = edit_text.trim_start();
            if let Some(after_prefix) = trimmed
                .strip_prefix('u')
                .or_else(|| trimmed.strip_prefix('U'))
            {
                let boundary_ok = after_prefix.is_empty()
                    || after_prefix
                        .chars()
                        .next()
                        .map(|ch| ch.is_whitespace())
                        .unwrap_or(false);
                if boundary_ok {
                    uninstall_quick_mode = true;
                    edit_text = after_prefix.trim_start().to_string();
                }
            }
        }

        state.command_mode_input = command_mode;
        unsafe {
            SetWindowTextW(state.edit_hwnd, to_wide(&edit_text).as_ptr());
        }
        set_uninstall_quick_mode(self.hwnd, state, uninstall_quick_mode, true);
        apply_edit_text_rect(
            state.edit_hwnd,
            state.command_mode_input,
            state.command_uninstall_quick_mode,
        );

        let caret = edit_text.encode_utf16().count() as isize;
        unsafe {
            SendMessageW(state.edit_hwnd, EM_SETSEL, caret as usize, caret);
            InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
        }
    }

    pub fn set_status_text(&self, message: &str) {
        if let Some(state) = state_for(self.hwnd) {
            let trimmed = message.trim();
            let status_text = trimmed;
            let was_no_results = state.no_results_mode;
            state.status_is_error =
                !trimmed.is_empty() && trimmed.to_ascii_lowercase().contains("error");
            state.no_results_mode = trimmed.eq_ignore_ascii_case(NO_RESULTS_STATUS_TEXT);
            if state.no_results_mode && !was_no_results {
                state.no_results_anim_pending = true;
            } else if !state.no_results_mode {
                state.no_results_anim_pending = false;
            }
            state.help_tip_visible = false;
            unsafe {
                ShowWindow(state.help_tip_hwnd, SW_HIDE);
            }
            if trimmed.is_empty() {
                state.help_hovered = false;
                state.no_results_mode = false;
                state.no_results_anim_pending = false;
            }
            let wide = to_wide(status_text);
            unsafe {
                SetWindowTextW(state.status_hwnd, wide.as_ptr());
                InvalidateRect(state.status_hwnd, std::ptr::null(), 1);
            }
            layout_children(self.hwnd, state);
            unsafe {
                InvalidateRect(self.hwnd, std::ptr::null(), 1);
            }
        }
    }

    pub fn set_hotkey_hint(&self, _hotkey: &str) {
        self.set_status_text("");
    }

    pub fn set_performance_tuning(&self, idle_cache_trim_ms: u32, active_memory_target_mb: u16) {
        configure_runtime_performance_tuning(idle_cache_trim_ms, active_memory_target_mb);
    }

    pub fn set_game_mode_enabled(&self, enabled: bool) {
        if let Some(state) = state_for(self.hwnd) {
            state.game_mode_enabled = enabled;
            let _ = update_tray_icon(self.hwnd, state);
        }
    }

    pub fn set_hotkey_issue_active(&self, active: bool) {
        if let Some(state) = state_for(self.hwnd) {
            state.hotkey_issue_active = active;
            let _ = update_tray_icon(self.hwnd, state);
        }
    }

    pub fn set_everything_active(&self, active: bool) {
        if let Some(state) = state_for(self.hwnd) {
            state.everything_active = active;
            if active {
                let wide = to_wide(EVERYTHING_INDICATOR_TEXT);
                unsafe {
                    SetWindowTextW(state.everything_hwnd, wide.as_ptr());
                    ShowWindow(state.everything_hwnd, SW_SHOW);
                }
            } else {
                unsafe {
                    ShowWindow(state.everything_hwnd, SW_HIDE);
                }
            }
            layout_children(self.hwnd, state);
        }
    }

    pub fn trim_runtime_memory(&self) {
        if let Some(state) = state_for(self.hwnd) {
            clear_icon_cache(state);
            log_memory_snapshot("manual_trim");
            unsafe {
                InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
            }
        }
    }

    pub fn set_mode_strip_text(&self, text: &str) {
        if let Some(state) = state_for(self.hwnd) {
            let resolved = if text.trim().is_empty() {
                MODE_STRIP_DEFAULT_TEXT.to_string()
            } else {
                text.trim().to_string()
            };
            if state.mode_strip_text == resolved {
                return;
            }
            state.mode_strip_text = resolved.clone();
            let wide = to_wide(&resolved);
            unsafe {
                SetWindowTextW(state.mode_strip_hwnd, wide.as_ptr());
                ShowWindow(state.mode_strip_hwnd, SW_HIDE);
            }
        }
    }

    pub fn set_help_config_path(&self, path: &str) {
        if let Some(state) = state_for(self.hwnd) {
            state.help_config_path = path.to_string();
        }
    }

    pub fn show_placeholder_hint(&self, message: &str) {
        if let Some(state) = state_for(self.hwnd) {
            state.placeholder_hint = message.trim().to_string();
            unsafe {
                InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
            }
        }
    }

    pub fn clear_placeholder_hint(&self) {
        if let Some(state) = state_for(self.hwnd) {
            let had_hint = !state.placeholder_hint.is_empty();
            state.placeholder_hint.clear();
            if had_hint {
                unsafe {
                    InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
                }
            }
        }
    }

    pub fn clear_query_text(&self) {
        if let Some(state) = state_for(self.hwnd) {
            state.command_mode_input = false;
            unsafe {
                SetWindowTextW(state.edit_hwnd, to_wide("").as_ptr());
            }
            set_uninstall_quick_mode(self.hwnd, state, false, false);
            apply_edit_text_rect(
                state.edit_hwnd,
                state.command_mode_input,
                state.command_uninstall_quick_mode,
            );
        }
    }

    pub fn set_results(&self, rows: &[OverlayRow], selected_index: usize) {
        if let Some(state) = state_for(self.hwnd) {
            if state
                .window_anim
                .as_ref()
                .map(|anim| !anim.hide_on_complete)
                .unwrap_or(false)
            {
                complete_window_animation_if_running(self.hwnd, state);
            }
            state.active_query = self
                .query_text()
                .trim()
                .trim_start_matches('>')
                .trim()
                .to_string();
            state.hover_index = -1;
            state.wheel_delta_remainder = 0;
            state.pending_wheel_delta = 0;
            let had_rows = !state.rows.is_empty();

            // Clear stale pending icon loads from previous results.
            state.pending_icon_loads.clear();

            if rows.is_empty() {
                schedule_icon_cache_idle_cleanup(self.hwnd);
                state.results_content_anim_start = None;
                unsafe {
                    KillTimer(self.hwnd, TIMER_RESULTS_CONTENT_FADE);
                }
                if state.results_visible && !state.rows.is_empty() {
                    self.collapse_results();
                    layout_children(self.hwnd, state);
                    return;
                }

                state.rows.clear();
                unsafe {
                    SendMessageW(state.list_hwnd, LB_RESETCONTENT, 0, 0);
                    SendMessageW(state.list_hwnd, LB_SETTOPINDEX, 0, 0);
                }

                self.collapse_results();
                state.hover_index = -1;
                state.expanded_rows = 0;
                state.suppress_next_hover_sync = false;
                if !state.status_is_error {
                    let wide = to_wide("");
                    unsafe {
                        SetWindowTextW(state.status_hwnd, wide.as_ptr());
                    }
                }
                layout_children(self.hwnd, state);
                return;
            }

            cancel_icon_cache_idle_cleanup(self.hwnd);
            let _ = had_rows;
            let should_animate_content = !had_rows || !state.results_visible;

            state.rows.clear();
            state.rows.extend_from_slice(rows);
            unsafe {
                // Batch first-render list updates so the first query does not flash
                // an intermediate frame while rows are being rebuilt.
                SendMessageW(state.list_hwnd, WM_SETREDRAW as u32, 0, 0);
                SendMessageW(state.list_hwnd, LB_RESETCONTENT, 0, 0);
                SendMessageW(state.list_hwnd, LB_SETTOPINDEX, 0, 0);
            }

            for row in rows {
                // Keep listbox item text lightweight; owner-draw uses state.rows.
                let wide = to_wide(&row.title);
                unsafe {
                    SendMessageW(state.list_hwnd, LB_ADDSTRING, 0, wide.as_ptr() as LPARAM);
                }
            }

            let visible_rows = initial_visible_row_count(rows);
            self.expand_results(visible_rows);
            state.status_is_error = false;
            state.no_results_mode = false;
            state.no_results_anim_pending = false;
            state.suppress_next_hover_sync = true;
            let wide = to_wide("");
            unsafe {
                SetWindowTextW(state.status_hwnd, wide.as_ptr());
                InvalidateRect(state.status_hwnd, std::ptr::null(), 1);
            }
            layout_children(self.hwnd, state);
            let status_only_row = rows.len() == 1 && matches!(rows[0].role, OverlayRowRole::Status);
            if status_only_row {
                unsafe {
                    SendMessageW(state.list_hwnd, LB_SETCURSEL, usize::MAX, 0);
                }
            } else {
                self.set_selected_index_internal(selected_index);
            }
            if selected_index == 0 || status_only_row {
                unsafe {
                    SendMessageW(state.list_hwnd, LB_SETTOPINDEX, 0, 0);
                }
            }
            if should_animate_content {
                state.results_content_anim_start = Some(Instant::now());
                unsafe {
                    SetTimer(
                        self.hwnd,
                        TIMER_RESULTS_CONTENT_FADE,
                        ANIM_FRAME_MS as u32,
                        None,
                    );
                }
            } else {
                state.results_content_anim_start = None;
                unsafe {
                    KillTimer(self.hwnd, TIMER_RESULTS_CONTENT_FADE);
                }
            }
            unsafe {
                SendMessageW(state.list_hwnd, WM_SETREDRAW as u32, 1, 0);
                InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
                // Parent overlay (self.hwnd) not invalidated here — the window
                // class has CS_HREDRAW | CS_VREDRAW which already invalidates
                // the full client area on any size change from expand_results.
                // Redundant invalidation would trigger a D2D EndDraw → DXGI
                // Present before the listbox's GDI content has painted,
                // causing a D2D/GDI desync flash on WS_EX_LAYERED windows.
            }
        }
    }

    pub fn set_selected_index(&self, selected_index: usize) {
        self.set_selected_index_internal(selected_index);
    }

    fn set_selected_index_internal(&self, selected_index: usize) {
        let Some(state) = state_for(self.hwnd) else {
            return;
        };

        let count = unsafe { SendMessageW(state.list_hwnd, LB_GETCOUNT, 0, 0) };
        if count <= 0 {
            return;
        }

        let Some(clamped) = row_index_for_result_index(state, selected_index) else {
            state.hover_index = -1;
            unsafe {
                SendMessageW(state.list_hwnd, LB_SETCURSEL, usize::MAX, 0);
                InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
            }
            return;
        };
        let current_top = unsafe { SendMessageW(state.list_hwnd, LB_GETTOPINDEX, 0, 0) as i32 };
        let target_top = target_top_index_for_selection(
            state.list_hwnd,
            clamped as i32,
            count as i32,
            current_top,
        );
        state.hover_index = clamped as i32;
        unsafe {
            // Avoid default listbox "scroll into view" animation on keyboard selection changes.
            SendMessageW(state.list_hwnd, WM_SETREDRAW as u32, 0, 0);
            if target_top != current_top {
                SendMessageW(state.list_hwnd, LB_SETTOPINDEX, target_top as usize, 0);
            }
            SendMessageW(state.list_hwnd, LB_SETCURSEL, clamped, 0);
            SendMessageW(state.list_hwnd, WM_SETREDRAW as u32, 1, 0);
            InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
        }
    }

    pub fn selected_index(&self) -> Option<usize> {
        let state = state_for(self.hwnd)?;
        let index = unsafe { SendMessageW(state.list_hwnd, LB_GETCURSEL, 0, 0) };
        if index < 0 {
            None
        } else {
            row_result_index(state, index as usize)
        }
    }

    pub fn run_message_loop_with_events<F>(&self, mut on_event: F) -> Result<(), String>
    where
        F: FnMut(OverlayEvent),
    {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if status == -1 {
                let err = unsafe { GetLastError() };
                return Err(format!("GetMessageW failed with error {err}"));
            }
            if status == 0 {
                return Ok(());
            }

            if msg.message == NEX_WM_QUERY_CHANGED {
                // Coalesce bursts of EN_CHANGE notifications into one query update.
                let mut drain: MSG = unsafe { std::mem::zeroed() };
                loop {
                    let removed = unsafe {
                        PeekMessageW(
                            &mut drain,
                            std::ptr::null_mut(),
                            NEX_WM_QUERY_CHANGED,
                            NEX_WM_QUERY_CHANGED,
                            PM_REMOVE,
                        )
                    };
                    if removed == 0 {
                        break;
                    }
                }
                on_event(OverlayEvent::QueryChanged(self.query_text()));
                continue;
            }
            if msg.message == NEX_WM_SEARCH_RESULTS_READY {
                on_event(OverlayEvent::SearchResultsReady);
                continue;
            }
            match msg.message {
                WM_HOTKEY => on_event(OverlayEvent::Hotkey(msg.wParam as i32)),
                NEX_WM_MOVE_UP => on_event(OverlayEvent::MoveSelection(-1)),
                NEX_WM_MOVE_DOWN => on_event(OverlayEvent::MoveSelection(1)),
                NEX_WM_SUBMIT => on_event(OverlayEvent::Submit),
                NEX_WM_TRAY_TOGGLE_GAME_MODE => on_event(OverlayEvent::TrayToggleGameMode),
                NEX_WM_TRAY_CHECK_UPDATES => on_event(OverlayEvent::TrayCheckForUpdates),
                NEX_WM_ESCAPE => on_event(OverlayEvent::Escape),
                NEX_WM_EXTERNAL_SHOW => on_event(OverlayEvent::ExternalShow),
                NEX_WM_EXTERNAL_QUIT => on_event(OverlayEvent::ExternalQuit),
                _ => {}
            }

            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn initialize_tray_icon(&self) -> Result<(), String> {
        let Some(state) = state_for(self.hwnd) else {
            return Err("overlay state unavailable for tray init".to_string());
        };
        if state.tray_icon_added {
            return Ok(());
        }
        state.tray_icon_handle = load_tray_icon_handle()?;
        add_tray_icon(self.hwnd, state)?;
        Ok(())
    }

    fn center_window(&self) {
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let x = (screen_width - WINDOW_WIDTH).max(0) / 2;
        let y = ((screen_height - COMPACT_HEIGHT).max(0) / 4 + WINDOW_OFFSET_Y).max(0);

        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                WINDOW_WIDTH,
                COMPACT_HEIGHT,
                SWP_NOACTIVATE,
            );
        }
    }

    fn apply_rounded_corners(&self) {
        apply_rounded_corners_hwnd(self.hwnd);
    }

    fn hide_immediate(&self) {
        unsafe {
            SetLayeredWindowAttributes(self.hwnd, 0, OVERLAY_ALPHA_OPAQUE, LWA_ALPHA);
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn ensure_compact_state(&self) {
        self.animate_results_height(COMPACT_HEIGHT, 0);
        if let Some(state) = state_for(self.hwnd) {
            state.results_visible = false;
            state.expanded_rows = 0;
            state.hover_index = -1;
            state.suppress_next_hover_sync = false;
            state.results_content_anim_start = None;
            unsafe {
                ShowWindow(state.footer_hint_hwnd, SW_HIDE);
                ShowWindow(state.mode_strip_hwnd, SW_HIDE);
                ShowWindow(state.everything_hwnd, SW_HIDE);
                SendMessageW(state.list_hwnd, LB_SETTOPINDEX, 0, 0);
                SendMessageW(state.list_hwnd, LB_RESETCONTENT, 0, 0);
                KillTimer(self.hwnd, TIMER_RESULTS_CONTENT_FADE);
            }
            state.rows.clear();
        }
    }

    fn expand_results(&self, visible_row_count: usize) {
        let rows = visible_row_count.max(1) as i32;
        let animate = RESULTS_ANIM_MS;
        let list_top = COMPACT_HEIGHT + INPUT_TO_LIST_GAP;
        // Keep enough vertical space for list rows plus bottom breathing room.
        // This must mirror layout_children() non-inline list bottom reserve.
        let list_bottom_reserve = PANEL_MARGIN_X + FOOTER_HINT_HEIGHT + 4;
        if let Some(state) = state_for(self.hwnd) {
            state.expanded_rows = rows;
            state.results_visible = true;
            unsafe {
                ShowWindow(state.list_hwnd, SW_SHOW);
            }
        }

        let target_height = list_top + rows * ROW_HEIGHT + list_bottom_reserve;
        self.animate_results_height(target_height, animate);
    }

    fn collapse_results(&self) {
        self.animate_results_height(COMPACT_HEIGHT, RESULTS_ANIM_MS);
        if let Some(state) = state_for(self.hwnd) {
            state.results_visible = false;
            state.expanded_rows = 0;
            state.suppress_next_hover_sync = false;
        }
    }

    fn animate_results_height(&self, target_height: i32, duration_ms: u32) {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(self.hwnd, &mut rect);
        }
        let current_height = rect.bottom - rect.top;

        if current_height == target_height {
            return;
        }

        if duration_ms == 0 {
            apply_window_state(
                self.hwnd,
                rect.left,
                rect.top,
                rect.right - rect.left,
                target_height,
                OVERLAY_ALPHA_OPAQUE,
            );
            return;
        }

        start_window_animation(
            self.hwnd,
            rect.left,
            rect.top,
            rect.right - rect.left,
            current_height,
            rect.left,
            rect.top,
            rect.right - rect.left,
            target_height,
            OVERLAY_ALPHA_OPAQUE,
            OVERLAY_ALPHA_OPAQUE,
            duration_ms,
            false,
        );
    }

    fn animate_show(&self) {
        if self.is_visible() {
            unsafe {
                ShowWindow(self.hwnd, SW_SHOW);
            }
            return;
        }

        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(self.hwnd, &mut rect);
        }
        let final_left = rect.left;
        let final_top = rect.top;
        let final_width = rect.right - rect.left;
        let final_height = rect.bottom - rect.top;

        let start_width = ((final_width as f32) * 0.96_f32) as i32;
        let start_height = ((final_height as f32) * 0.96_f32) as i32;
        let start_left = final_left + (final_width - start_width) / 2;
        let start_top = final_top + (final_height - start_height) / 2;

        apply_window_state(
            self.hwnd,
            start_left,
            start_top,
            start_width,
            start_height,
            0,
        );
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
        }
        start_window_animation(
            self.hwnd,
            start_left,
            start_top,
            start_width,
            start_height,
            final_left,
            final_top,
            final_width,
            final_height,
            0,
            OVERLAY_ALPHA_OPAQUE,
            OVERLAY_ANIM_MS,
            false,
        );
    }

    fn animate_hide(&self) {
        if !self.is_visible() {
            return;
        }

        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(self.hwnd, &mut rect);
        }
        let current_left = rect.left;
        let current_top = rect.top;
        let current_width = rect.right - rect.left;
        let current_height = rect.bottom - rect.top;

        let end_width = ((current_width as f32) * 0.96_f32) as i32;
        let end_height = ((current_height as f32) * 0.96_f32) as i32;
        let end_left = current_left + (current_width - end_width) / 2;
        let end_top = current_top + (current_height - end_height) / 2;

        start_window_animation(
            self.hwnd,
            current_left,
            current_top,
            current_width,
            current_height,
            end_left,
            end_top,
            end_width,
            end_height,
            OVERLAY_ALPHA_OPAQUE,
            0,
            OVERLAY_ANIM_MS,
            true,
        );
    }
}

extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let create = lparam as *const CREATESTRUCTW;
            if create.is_null() {
                return 0;
            }
            let state_ptr = unsafe { (*create).lpCreateParams as *mut OverlayShellState };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            }
            1
        }
        WM_CREATE => {
            if let Some(state) = state_for(hwnd) {
                state.overlay_hwnd = hwnd;
                unsafe {
                    state.dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
                }
                if state.dpi < 96 {
                    state.dpi = 96;
                }
                state.icon_draw_size = ((ROW_ICON_DRAW_SIZE * state.dpi as i32) + 48) / 96;
                state.icon_container_size = ((ROW_ICON_SIZE * state.dpi as i32) + 48) / 96;
                state.theme = detect_system_theme();
                state.palette = palette_for_theme(state.theme);
                state.dwm_rounded_enabled = try_enable_dwm_rounded_corners(hwnd);

                // Initialize GDI+ for all rendering (hard requirement)
                state.gdiplus = crate::windows_overlay::gdiplus_rendering::GdiplusContext::new();
                if state.gdiplus.is_none() {
                    crate::logging::error("[nex] GDI+ initialization failed, overlay disabled");
                    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                    return 0;
                }

                // Initialize tiny-skia renderer for panel backgrounds
                state.skia = crate::windows_overlay::skia_renderer::SkiaRenderer::new(
                    WINDOW_WIDTH as u32,
                    COMPACT_HEIGHT as u32,
                );
                if state.skia.is_none() {
                    crate::logging::warn("[nex] Skia renderer init failed, panel may not draw");
                }

                state.panel_brush = unsafe { CreateSolidBrush(state.palette.panel_bg) } as isize;
                state.border_brush =
                    unsafe { CreateSolidBrush(state.palette.panel_border) } as isize;
                state.input_brush = unsafe { CreateSolidBrush(state.palette.input_bg) } as isize;
                state.results_brush =
                    unsafe { CreateSolidBrush(state.palette.results_bg) } as isize;
                state.selection_brush =
                    unsafe { CreateSolidBrush(state.palette.selection) } as isize;
                state.selection_border_brush =
                    unsafe { CreateSolidBrush(state.palette.selection_border) } as isize;
                state.row_hover_brush =
                    unsafe { CreateSolidBrush(state.palette.row_hover) } as isize;
                state.row_separator_brush =
                    unsafe { CreateSolidBrush(state.palette.row_separator) } as isize;
                state.selection_accent_brush =
                    unsafe { CreateSolidBrush(state.palette.selection_accent) } as isize;
                state.icon_brush = unsafe { CreateSolidBrush(state.palette.icon_bg) } as isize;
                crate::logging::info(&format!(
                    "[nex] overlay_theme mode={}",
                    match state.theme {
                        OverlayTheme::Dark => "dark",
                        OverlayTheme::Light => "light",
                    }
                ));

                state.input_font = create_font(FONT_INPUT_HEIGHT, FONT_WEIGHT_INPUT);
                state.title_font = create_font(FONT_TITLE_HEIGHT, FONT_WEIGHT_TITLE);
                state.meta_font = create_font(FONT_META_HEIGHT, FONT_WEIGHT_META);
                state.status_font = create_font(FONT_STATUS_HEIGHT, FONT_WEIGHT_STATUS);
                state.header_font = create_font(FONT_HEADER_HEIGHT, FONT_WEIGHT_HEADER);
                state.top_hit_font = create_font(FONT_TOP_HIT_HEIGHT, FONT_WEIGHT_TOP_HIT);
                state.hint_font = create_font(FONT_HINT_HEIGHT, FONT_WEIGHT_HINT);
                state.help_tip_font = create_font(FONT_HELP_TIP_HEIGHT, FONT_WEIGHT_HELP_TIP);
                state.help_icon_font = create_font_with_family(
                    FONT_HELP_ICON_HEIGHT,
                    FONT_WEIGHT_HELP_ICON,
                    icon_font_family_primary_wide(),
                );
                state.search_icon_font = create_font_with_family(
                    FONT_INPUT_HEIGHT,
                    FONT_WEIGHT_INPUT,
                    icon_font_family_primary_wide(),
                );
                state.footer_font = create_font(FONT_FOOTER_HEIGHT, FONT_WEIGHT_FOOTER);
                state.command_prefix_font = create_font_with_family(
                    FONT_COMMAND_PREFIX_HEIGHT,
                    FONT_WEIGHT_COMMAND_PREFIX,
                    command_prefix_font_family_wide(),
                );
                state.command_badge_font =
                    create_font(FONT_COMMAND_BADGE_HEIGHT, FONT_WEIGHT_COMMAND_BADGE);
                state.command_icon_font = create_font_with_family(
                    -((-FONT_COMMAND_ICON_HEIGHT * state.dpi as i32 + 48) / 96),
                    FONT_WEIGHT_COMMAND_ICON,
                    icon_font_family_primary_wide(),
                );
                state.command_icon_fallback_font = create_font_with_family(
                    -((-FONT_COMMAND_ICON_HEIGHT * state.dpi as i32 + 48) / 96),
                    FONT_WEIGHT_COMMAND_ICON,
                    icon_font_family_fallback_wide(),
                );

                // Pre-create GDI+ font handles from GDI fonts (avoids per-row
                // GdipCreateFontFromDC + GdipDeleteFont in draw_list_row).
                if state.gdiplus.is_some() {
                    let temp_dc = unsafe { windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut()) };
                    if !temp_dc.is_null() {
                        let pairs: [(isize, &mut isize); 7] = [
                            (state.title_font, &mut state.gdiplus_title_font),
                            (state.meta_font, &mut state.gdiplus_meta_font),
                            (state.status_font, &mut state.gdiplus_status_font),
                            (state.header_font, &mut state.gdiplus_header_font),
                            (state.help_tip_font, &mut state.gdiplus_help_tip_font),
                            (state.footer_font, &mut state.gdiplus_footer_font),
                            (state.hint_font, &mut state.gdiplus_hint_font),
                        ];
                        for (gdi_font, gp_dest) in pairs {
                            if gdi_font != 0 {
                                let old = unsafe { windows_sys::Win32::Graphics::Gdi::SelectObject(temp_dc, gdi_font as _) };
                                *gp_dest = crate::windows_overlay::gdiplus_rendering::GdiplusContext::create_font_from_hdc(temp_dc as isize).unwrap_or(0);
                                unsafe { windows_sys::Win32::Graphics::Gdi::SelectObject(temp_dc, old); }
                            }
                        }
                        unsafe { windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), temp_dc); }
                    }
                }

                state.edit_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(INPUT_CLASS).as_ptr(),
                        to_wide("").as_ptr(),
                        WS_CHILD
                            | WS_VISIBLE
                            | WS_TABSTOP
                            | ES_AUTOHSCROLL as u32
                            | ES_MULTILINE as u32,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_INPUT as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };

                state.list_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(LIST_CLASS).as_ptr(),
                        std::ptr::null(),
                        WS_CHILD
                            | WS_TABSTOP
                            | LBS_NOTIFY as u32
                            | LBS_OWNERDRAWFIXED as u32
                            | LBS_HASSTRINGS as u32
                            | LBS_NOINTEGRALHEIGHT as u32,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_LIST as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };

                state.status_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(STATUS_CLASS).as_ptr(),
                        to_wide("").as_ptr(),
                        WS_CHILD | WS_VISIBLE | STATIC_RIGHT_STYLE,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_STATUS as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                state.help_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(STATUS_CLASS).as_ptr(),
                        to_wide(HELP_ICON_TEXT).as_ptr(),
                        WS_CHILD | WS_VISIBLE | STATIC_NOTIFY_STYLE,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_HELP as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                state.help_tip_hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_TOOLWINDOW | EX_NOACTIVATE_STYLE,
                        to_wide(STATUS_CLASS).as_ptr(),
                        to_wide(HOTKEY_HELP_TEXT_FALLBACK).as_ptr(),
                        WS_POPUP | STATIC_NOTIFY_STYLE,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                state.footer_hint_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(STATUS_CLASS).as_ptr(),
                        to_wide(FOOTER_HINT_TEXT).as_ptr(),
                        WS_CHILD | STATIC_CENTER_STYLE,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_FOOTER_HINT as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                state.mode_strip_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(STATUS_CLASS).as_ptr(),
                        to_wide(MODE_STRIP_DEFAULT_TEXT).as_ptr(),
                        WS_CHILD | STATIC_CENTER_STYLE,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_MODE_STRIP as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                state.everything_hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        to_wide(STATUS_CLASS).as_ptr(),
                        to_wide("").as_ptr(),
                        WS_CHILD | STATIC_LEFT_STYLE,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        CONTROL_ID_EVERYTHING as HMENU,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };

                unsafe {
                    SendMessageW(state.edit_hwnd, WM_SETFONT, state.input_font as usize, 1);
                    SendMessageW(state.list_hwnd, WM_SETFONT, state.meta_font as usize, 1);
                    SendMessageW(state.status_hwnd, WM_SETFONT, state.status_font as usize, 1);
                    SendMessageW(
                        state.help_hwnd,
                        WM_SETFONT,
                        if state.help_icon_font != 0 {
                            state.help_icon_font as usize
                        } else {
                            state.status_font as usize
                        },
                        1,
                    );
                    SendMessageW(
                        state.footer_hint_hwnd,
                        WM_SETFONT,
                        state.hint_font as usize,
                        1,
                    );
                    SendMessageW(
                        state.mode_strip_hwnd,
                        WM_SETFONT,
                        state.hint_font as usize,
                        1,
                    );
                    SendMessageW(
                        state.everything_hwnd,
                        WM_SETFONT,
                        state.status_font as usize,
                        1,
                    );
                    SendMessageW(
                        state.help_tip_hwnd,
                        WM_SETFONT,
                        state.help_tip_font as usize,
                        1,
                    );
                    state.edit_prev_proc = SetWindowLongPtrW(
                        state.edit_hwnd,
                        GWLP_WNDPROC,
                        input::control_subclass_proc as *const () as isize,
                    );
                    state.list_prev_proc = SetWindowLongPtrW(
                        state.list_hwnd,
                        GWLP_WNDPROC,
                        input::control_subclass_proc as *const () as isize,
                    );
                    state.help_prev_proc = SetWindowLongPtrW(
                        state.help_hwnd,
                        GWLP_WNDPROC,
                        input::control_subclass_proc as *const () as isize,
                    );
                    state.help_tip_prev_proc = SetWindowLongPtrW(
                        state.help_tip_hwnd,
                        GWLP_WNDPROC,
                        input::control_subclass_proc as *const () as isize,
                    );
                    state.footer_hint_prev_proc = SetWindowLongPtrW(
                        state.footer_hint_hwnd,
                        GWLP_WNDPROC,
                        input::control_subclass_proc as *const () as isize,
                    );
                    SetWindowLongPtrW(state.help_tip_hwnd, GWLP_USERDATA, hwnd as isize);

                    ShowWindow(state.list_hwnd, SW_HIDE);
                    ShowWindow(state.help_tip_hwnd, SW_HIDE);
                    ShowWindow(state.footer_hint_hwnd, SW_HIDE);
                    ShowWindow(state.mode_strip_hwnd, SW_HIDE);
                    ShowWindow(state.everything_hwnd, SW_HIDE);
                }

                state.results_visible = false;
                state.hover_index = -1;
                layout_children(hwnd, state);

                // Spawn the background icon loader thread.
                let (load_thread, load_sender, load_receiver) =
                    icon_loader::spawn_icon_loader_thread();
                state.icon_load_thread = Some(load_thread);
                state.icon_load_sender = Some(load_sender);
                state.icon_load_receiver = Some(load_receiver);
            }
            0
        }
        WM_MEASUREITEM => {
            let measure = lparam as *mut MEASUREITEMSTRUCT;
            if !measure.is_null() {
                unsafe {
                    if (*measure).CtlID as usize == CONTROL_ID_LIST {
                        (*measure).itemHeight = ROW_HEIGHT as u32;
                        return 1;
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_DRAWITEM => {
            let draw = lparam as *mut DRAWITEMSTRUCT;
            if draw.is_null() {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }

            let dis = unsafe { &mut *draw };
            if dis.CtlID as usize != CONTROL_ID_LIST {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }

            draw_list_row(hwnd, dis);
            1
        }
        WM_COMMAND => {
            let control_id = wparam & 0xffff;
            let notification = (wparam >> 16) & 0xffff;
            if notification == 0 {
                match control_id {
                    TRAY_MENU_SHOW => {
                        unsafe {
                            PostMessageW(hwnd, NEX_WM_EXTERNAL_SHOW, 0, 0);
                        }
                        return 0;
                    }
                    TRAY_MENU_OPEN_CONFIG => {
                        if let Some(state) = state_for(hwnd) {
                            let _ = open_help_config_file(state);
                        }
                        return 0;
                    }
                    TRAY_MENU_CHECK_UPDATES => {
                        unsafe {
                            PostMessageW(hwnd, NEX_WM_TRAY_CHECK_UPDATES, 0, 0);
                        }
                        return 0;
                    }
                    TRAY_MENU_GAME_MODE => {
                        unsafe {
                            PostMessageW(hwnd, NEX_WM_TRAY_TOGGLE_GAME_MODE, 0, 0);
                        }
                        return 0;
                    }
                    TRAY_MENU_QUIT => {
                        unsafe {
                            PostMessageW(hwnd, NEX_WM_EXTERNAL_QUIT, 0, 0);
                        }
                        return 0;
                    }
                    _ => {}
                }
            }
            if control_id == CONTROL_ID_INPUT && notification as u32 == EN_CHANGE as u32 {
                if let Some(state) = state_for(hwnd) {
                    if !state.placeholder_hint.is_empty() {
                        state.placeholder_hint.clear();
                        unsafe {
                            InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
                        }
                    }
                }
                unsafe {
                    PostMessageW(hwnd, NEX_WM_QUERY_CHANGED, 0, 0);
                }
                return 0;
            }
            if control_id == CONTROL_ID_LIST && notification as u32 == LBN_DBLCLK as u32 {
                unsafe {
                    PostMessageW(hwnd, NEX_WM_SUBMIT, 0, 0);
                }
                return 0;
            }
            if (control_id == CONTROL_ID_HELP || control_id == CONTROL_ID_HELP_TIP)
                && notification == 0
            {
                if let Some(state) = state_for(hwnd) {
                    if let Err(error) = open_help_config_file(state) {
                        state.status_is_error = true;
                        state.help_tip_visible = false;
                        let wide = to_wide(&format!("Help open error: {error}"));
                        unsafe {
                            SetWindowTextW(state.status_hwnd, wide.as_ptr());
                            InvalidateRect(state.status_hwnd, std::ptr::null(), 1);
                        }
                        layout_children(hwnd, state);
                    }
                }
                return 0;
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CTLCOLORSTATIC => {
            if let Some(state) = state_for(hwnd) {
                let target = lparam as HWND;
                if target == state.help_hwnd {
                    let base_help_color = blend_color(
                        state.palette.input_bg,
                        state.palette.text_hint,
                        COMMAND_PREFIX_OPACITY,
                    );
                    let hover_help_color = blend_color(
                        state.palette.input_bg,
                        state.palette.text_primary,
                        COMMAND_PREFIX_OPACITY,
                    );
                    unsafe {
                        SetTextColor(
                            wparam as _,
                            if state.help_hovered {
                                hover_help_color
                            } else {
                                base_help_color
                            },
                        );
                        SetBkMode(wparam as _, TRANSPARENT as i32);
                    }
                    return state.panel_brush;
                }
                if target == state.footer_hint_hwnd {
                    unsafe {
                        SetTextColor(wparam as _, state.palette.text_hint_footer);
                        SetBkMode(wparam as _, TRANSPARENT as i32);
                    }
                    return state.panel_brush;
                }
                if target == state.mode_strip_hwnd {
                    unsafe {
                        SetTextColor(wparam as _, state.palette.text_mode_strip);
                        SetBkMode(wparam as _, TRANSPARENT as i32);
                    }
                    return state.panel_brush;
                }
                if target == state.everything_hwnd {
                    unsafe {
                        SetTextColor(wparam as _, state.palette.text_hint_footer);
                        SetBkMode(wparam as _, TRANSPARENT as i32);
                    }
                    return state.panel_brush;
                }
                if target == state.status_hwnd {
                    let color = if state.status_is_error {
                        state.palette.text_error
                    } else {
                        state.palette.text_hint
                    };
                    unsafe {
                        SetTextColor(wparam as _, color);
                        SetBkMode(wparam as _, TRANSPARENT as i32);
                    }
                    return state.panel_brush;
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CTLCOLOREDIT => {
            if let Some(state) = state_for(hwnd) {
                let target = lparam as HWND;
                if target == state.edit_hwnd {
                    unsafe {
                        SetTextColor(wparam as _, state.palette.text_primary);
                        SetBkColor(wparam as _, state.palette.input_bg);
                        SetBkMode(wparam as _, OPAQUE as i32);
                    }
                    return state.input_brush;
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CTLCOLORLISTBOX => {
            if let Some(state) = state_for(hwnd) {
                let target = lparam as HWND;
                if target == state.list_hwnd {
                    unsafe {
                        SetTextColor(wparam as _, state.palette.text_primary);
                        SetBkColor(wparam as _, state.palette.results_bg);
                    }
                    return state.results_brush;
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_SIZE => {
            if let Some(state) = state_for(hwnd) {
                layout_children(hwnd, state);
            }
            apply_rounded_corners_hwnd(hwnd);
            0
        }
        WM_ACTIVATE => {
            let activation = (wparam & 0xFFFF) as u32;
            if activation == 0 {
                let activated_hwnd = lparam as HWND;
                if let Some(state) = state_for(hwnd) {
                    // The help tip is a no-activate popup owned by this overlay.
                    // Ignore this activation change so hovering/clicking "?" does not close the launcher.
                    if activated_hwnd == state.help_tip_hwnd {
                        return 0;
                    }
                }

                // Ignore transient/internal focus churn while the overlay still owns focus.
                let foreground = unsafe { GetForegroundWindow() };
                if foreground == hwnd || unsafe { IsChild(hwnd, foreground) } != 0 {
                    return 0;
                }
                unsafe {
                    PostMessageW(hwnd, NEX_WM_ESCAPE, 0, 0);
                }
                hide_overlay_immediate(hwnd);
            }
            0
        }
        WM_PAINT => {
            draw_panel_background(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            if let Some(state) = state_for(hwnd) {
                if !state.results_visible {
                    return 0;
                }
                if is_cursor_over_window(state.list_hwnd) {
                    handle_wheel_input(state, wparam);
                }
                return 0;
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_TIMER => {
            if wparam == TIMER_WINDOW_ANIM {
                if let Some(state) = state_for(hwnd) {
                    let running = window_animation_tick(hwnd, state);
                    if !running {
                        unsafe {
                            KillTimer(hwnd, TIMER_WINDOW_ANIM);
                        }
                    }
                }
            }
            if wparam == TIMER_HELP_HOVER {
                if let Some(state) = state_for(hwnd) {
                    sync_help_hover_with_cursor(hwnd, state);
                }
            }
            if wparam == TIMER_ICON_CACHE_IDLE {
                if let Some(state) = state_for(hwnd) {
                    if state.results_visible || state.help_hovered {
                        schedule_icon_cache_idle_cleanup(hwnd);
                    } else {
                        clear_icon_cache(state);
                        log_memory_snapshot("icon_cache_trim");
                        unsafe {
                            KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
                        }
                    }
                } else {
                    unsafe {
                        KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
                    }
                }
            }
            if wparam == TIMER_RESULTS_CONTENT_FADE {
                if let Some(state) = state_for(hwnd) {
                    let running = results_content_animation_tick(hwnd, state);
                    if !running {
                        state.results_content_anim_start = None;
                        unsafe {
                            KillTimer(hwnd, TIMER_RESULTS_CONTENT_FADE);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        layout_children(hwnd, state);
                    }
                } else {
                    unsafe {
                        KillTimer(hwnd, TIMER_RESULTS_CONTENT_FADE);
                    }
                }
            }
            if wparam == TIMER_COMMAND_BADGE_FADE {
                if let Some(state) = state_for(hwnd) {
                    let running = command_badge_animation_tick(state);
                    if !running {
                        unsafe {
                            KillTimer(hwnd, TIMER_COMMAND_BADGE_FADE);
                        }
                    }
                } else {
                    unsafe {
                        KillTimer(hwnd, TIMER_COMMAND_BADGE_FADE);
                    }
                }
            }
            0
        }
        NEX_WM_ICON_LOADED => {
            if let Some(state) = state_for(hwnd) {
                let results = state
                    .icon_load_receiver
                    .as_ref()
                    .map(|r| {
                        let mut v = Vec::new();
                        while let Ok(result) = r.try_recv() {
                            v.push(result);
                        }
                        v
                    })
                    .unwrap_or_default();
                if !results.is_empty() {
                    for r in results {
                        state.pending_icon_loads.remove(&r.key);
                        insert_icon_cache_entry(state, r.key, r.handle);
                    }
                    unsafe {
                        InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
                    }
                }
            }
            0
        }
        NEX_WM_TRAY_ICON => {
            if let Some(state) = state_for(hwnd) {
                match lparam as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => unsafe {
                        PostMessageW(hwnd, NEX_WM_EXTERNAL_SHOW, 0, 0);
                    },
                    WM_RBUTTONUP => {
                        show_tray_context_menu(hwnd, state);
                    }
                    _ => {}
                }
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_DESTROY => {
            if let Some(state) = state_for(hwnd) {
                remove_tray_icon(hwnd, state);
            }
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        WM_NCDESTROY => {
            unsafe {
                KillTimer(hwnd, TIMER_HELP_HOVER);
                KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
                KillTimer(hwnd, TIMER_RESULTS_CONTENT_FADE);
                KillTimer(hwnd, TIMER_COMMAND_BADGE_FADE);
            }
            let state_ptr =
                unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayShellState };
            if !state_ptr.is_null() {
                unsafe {
                    cleanup_state_resources(&mut *state_ptr);
                    let _ = Box::from_raw(state_ptr);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
            }
            0
        }
        NEX_WM_ESCAPE
        | NEX_WM_QUERY_CHANGED
        | NEX_WM_MOVE_UP
        | NEX_WM_MOVE_DOWN
        | NEX_WM_SUBMIT
        | NEX_WM_SEARCH_RESULTS_READY => 0,
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

// ==================== FONT HELPERS (recovered) ====================

use std::path::PathBuf;
use std::sync::OnceLock;

fn class_name_wide() -> &'static [u16] {
    static CLASS_NAME_WIDE: OnceLock<Vec<u16>> = OnceLock::new();
    CLASS_NAME_WIDE
        .get_or_init(|| to_wide(CLASS_NAME))
        .as_slice()
}

fn font_family_wide() -> &'static [u16] {
    static FONT_FAMILY_WIDE: OnceLock<Vec<u16>> = OnceLock::new();
    FONT_FAMILY_WIDE
        .get_or_init(|| {
            let family = resolve_font_family(
                std::env::var("NEX_FONT_FAMILY")
                    .or_else(|_| std::env::var("SWIFTFIND_FONT_FAMILY"))
                    .ok()
                    .as_deref(),
                register_private_fonts(),
            );
            to_wide(&family)
        })
        .as_slice()
}

fn icon_font_family_primary_wide() -> &'static [u16] {
    static ICON_FONT_PRIMARY_WIDE: OnceLock<Vec<u16>> = OnceLock::new();
    ICON_FONT_PRIMARY_WIDE
        .get_or_init(|| to_wide(ICON_FONT_FAMILY_PRIMARY))
        .as_slice()
}

fn icon_font_family_fallback_wide() -> &'static [u16] {
    static ICON_FONT_FALLBACK_WIDE: OnceLock<Vec<u16>> = OnceLock::new();
    ICON_FONT_FALLBACK_WIDE
        .get_or_init(|| to_wide(ICON_FONT_FAMILY_FALLBACK))
        .as_slice()
}

fn command_prefix_font_family_wide() -> &'static [u16] {
    static COMMAND_PREFIX_FONT_WIDE: OnceLock<Vec<u16>> = OnceLock::new();
    COMMAND_PREFIX_FONT_WIDE
        .get_or_init(|| to_wide(COMMAND_PREFIX_FONT_FAMILY))
        .as_slice()
}

fn resolve_font_family(font_env: Option<&str>, primary_loaded: bool) -> String {
    if let Some(value) = font_env.map(|v| v.trim()).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    if primary_loaded {
        return PRIMARY_FONT_FAMILY.to_string();
    }
    for &family in FALLBACK_FONT_CHAIN {
        if font_is_available(family) {
            return family.to_string();
        }
    }
    "Segoe UI".to_string()
}

fn font_is_available(family_name: &str) -> bool {
    let wide = to_wide(family_name);
    let hfont = unsafe {
        CreateFontW(
            0, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, 0,
            CLEARTYPE_QUALITY as u32, FF_DONTCARE as u32, wide.as_ptr(),
        )
    };
    if hfont.is_null() {
        return false;
    }
    let hdc = unsafe { GetDC(std::ptr::null_mut()) };
    if hdc.is_null() {
        unsafe { DeleteObject(hfont as _); }
        return false;
    }
    let old = unsafe { SelectObject(hdc, hfont as _) };
    let mut buf = [0u16; 256];
    let len = unsafe { GetTextFaceW(hdc, 256, buf.as_mut_ptr()) };
    unsafe { SelectObject(hdc, old); ReleaseDC(std::ptr::null_mut(), hdc); DeleteObject(hfont as _); }
    if len == 0 {
        return false;
    }
    let actual = String::from_utf16_lossy(&buf[..len as usize - 1]);
    actual.eq_ignore_ascii_case(family_name)
}

fn register_private_fonts() -> bool {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    *REGISTERED.get_or_init(|| {
        let mut candidates = Vec::new();
        if let Ok(dir) =
            std::env::var("NEX_FONT_DIR").or_else(|_| std::env::var("SWIFTFIND_FONT_DIR"))
        {
            let trimmed = dir.trim();
            if !trimmed.is_empty() {
                candidates.push(PathBuf::from(trimmed));
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("apps/assets/fonts/Inter/ttf"));
            candidates.push(cwd.join("fonts/Inter/ttf"));
            candidates.push(cwd.join("assets/fonts/Inter/ttf"));
            candidates.push(cwd.join("apps/assets/fonts/Inter"));
            candidates.push(cwd.join("fonts/Inter"));
            candidates.push(cwd.join("assets/fonts/Inter"));
        }
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                candidates.push(exe_dir.join("..").join("assets/fonts/Inter/ttf"));
                candidates.push(exe_dir.join("assets/fonts/Inter/ttf"));
                candidates.push(exe_dir.join("..").join("assets/fonts/Inter"));
                candidates.push(exe_dir.join("assets/fonts/Inter"));
            }
        }

        let files = [
    "Inter-Regular.ttf",
    "Inter-Bold.ttf",
        ];

        for base_dir in candidates {
            if !base_dir.is_dir() {
                continue;
            }
            let mut loaded_any = false;
            for file_name in files {
                let font_path = base_dir.join(file_name);
                if !font_path.is_file() {
                    continue;
                }
                let font_wide = path_to_wide(&font_path);
                let added =
                    unsafe { AddFontResourceExW(font_wide.as_ptr(), FR_PRIVATE, std::ptr::null()) };
                if added > 0 {
                    loaded_any = true;
                }
            }
            if loaded_any {
                return true;
            }
        }
        false
    })
}

pub(crate) fn path_to_wide(path: &Path) -> Vec<u16> {
    path.to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

fn create_font(height: i32, weight: i32) -> isize {
    create_font_with_family_quality(height, weight, font_family_wide(), CLEARTYPE_QUALITY as u32)
}

fn create_font_with_family(height: i32, weight: i32, family_wide: &[u16]) -> isize {
    create_font_with_family_quality(height, weight, family_wide, CLEARTYPE_QUALITY as u32)
}

fn create_font_with_family_quality(
    height: i32,
    weight: i32,
    family_wide: &[u16],
    quality: u32,
) -> isize {
    (unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            0,
            quality,
            FF_DONTCARE as u32,
            family_wide.as_ptr(),
        )
    }) as isize
}

// ==================== INSTANCE HELPERS (recovered) ====================

pub fn is_instance_window_present() -> bool {
    let hwnd = unsafe { FindWindowW(class_name_wide().as_ptr(), std::ptr::null()) };
    !hwnd.is_null()
}

pub fn signal_existing_instance_show() -> Result<bool, String> {
    let hwnd = unsafe { FindWindowW(class_name_wide().as_ptr(), std::ptr::null()) };
    if hwnd.is_null() {
        return Ok(false);
    }
    let ok = unsafe { PostMessageW(hwnd, NEX_WM_EXTERNAL_SHOW, 0, 0) };
    if ok == 0 {
        let error = unsafe { GetLastError() };
        return Err(format!("PostMessageW(show) failed with error {error}"));
    }
    Ok(true)
}

pub fn signal_existing_instance_quit() -> Result<bool, String> {
    let hwnd = unsafe { FindWindowW(class_name_wide().as_ptr(), std::ptr::null()) };
    if hwnd.is_null() {
        return Ok(false);
    }
    let ok = unsafe { PostMessageW(hwnd, NEX_WM_EXTERNAL_QUIT, 0, 0) };
    if ok == 0 {
        let error = unsafe { GetLastError() };
        return Err(format!("PostMessageW(quit) failed with error {error}"));
    }
    Ok(true)
}

// ==================== WINDOW HELPERS (recovered) ====================

fn open_help_config_file(state: &mut OverlayShellState) -> Result<(), String> {
    let cfg_path = state.help_config_path.trim().to_string();
    let target = if cfg_path.is_empty() {
        crate::config::stable_config_path()
            .to_string_lossy()
            .into_owned()
    } else {
        cfg_path
    };
    let wide = to_wide(&target);
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            to_wide("open").as_ptr(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            5,
        )
    };
    if (result as isize) <= 32 {
        Err(format!("ShellExecuteW returned {}", result as isize))
    } else {
        Ok(())
    }
}
