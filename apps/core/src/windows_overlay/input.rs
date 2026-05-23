use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetCursorPos, GetParent, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, HideCaret, KillTimer, LoadCursorW, PostMessageW, SendMessageW, SetCursor,
    SetTimer, SetWindowTextW, ShowWindow, GWLP_USERDATA, IDC_HAND, LB_GETCOUNT, LB_GETTOPINDEX,
    LB_ITEMFROMPOINT, LB_SETCURSEL, LB_SETTOPINDEX, SW_HIDE, SW_SHOW, WM_KEYDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETFOCUS, WM_SETREDRAW,
};

use crate::windows_overlay::layout::{apply_edit_text_rect, visible_row_capacity};
use crate::windows_overlay::painting::{
    help_hint_text, invalidate_list_row, paint_edit_command_prefix, paint_edit_placeholder,
    paint_footer_hint, paint_help_tip, row_is_selectable, set_uninstall_quick_mode,
};
use crate::windows_overlay::state::{state_for, OverlayShellState};
use crate::windows_overlay::types::*;
pub(crate) extern "system" fn control_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mut parent = unsafe { GetParent(hwnd) };
    if parent.is_null() {
        parent = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HWND };
    }
    if parent.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    if let Some(state) = state_for(parent) {
        if hwnd == state.help_tip_hwnd && message == WM_PAINT {
            paint_help_tip(hwnd, state);
            return 0;
        }
        if hwnd == state.footer_hint_hwnd && message == WM_PAINT {
            paint_footer_hint(hwnd, state);
            return 0;
        }
        if hwnd == state.edit_hwnd
            && (message == WM_SETFOCUS
                || message == WM_KEYDOWN
                || message == windows_sys::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN
                || message == WM_LBUTTONUP)
        {
            hide_input_caret(hwnd);
        }
        if message == WM_MOUSEMOVE {
            if hwnd == state.help_hwnd || hwnd == state.help_tip_hwnd {
                set_help_hover_state(parent, state, true);
            } else if state.help_hovered {
                sync_help_hover_with_cursor(parent, state);
            }
        }
        if message == WM_MOUSEWHEEL && (hwnd == state.edit_hwnd || hwnd == state.list_hwnd) {
            if !state.results_visible {
                return 0;
            }
            if hwnd == state.edit_hwnd && !is_cursor_over_window(state.list_hwnd) {
                return 0;
            }
            handle_wheel_input(state, wparam);
            return 0;
        }
        if message == WM_KEYDOWN
            && hwnd == state.edit_hwnd
            && wparam as u16 == VK_BACK
            && state.command_mode_input
        {
            let edit_empty = unsafe { GetWindowTextLengthW(state.edit_hwnd) } <= 0;
            if edit_empty {
                if state.command_uninstall_quick_mode {
                    set_uninstall_quick_mode(parent, state, false, false);
                } else {
                    state.command_mode_input = false;
                    apply_edit_text_rect(
                        state.edit_hwnd,
                        state.command_mode_input,
                        state.command_uninstall_quick_mode,
                    );
                }
                unsafe {
                    InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
                    PostMessageW(parent, NEX_WM_QUERY_CHANGED, 0, 0);
                }
                return 0;
            }
        }
        if message == windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR
            && (hwnd == state.edit_hwnd || hwnd == state.list_hwnd)
        {
            if hwnd == state.edit_hwnd && wparam as u32 == '>' as u32 {
                let edit_empty = unsafe { GetWindowTextLengthW(state.edit_hwnd) } <= 0;
                state.command_mode_input = !(state.command_mode_input && edit_empty);
                if state.command_mode_input && !state.placeholder_hint.is_empty() {
                    state.placeholder_hint.clear();
                }
                if !state.command_mode_input {
                    set_uninstall_quick_mode(parent, state, false, false);
                }
                apply_edit_text_rect(
                    state.edit_hwnd,
                    state.command_mode_input,
                    state.command_uninstall_quick_mode,
                );
                unsafe {
                    InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
                    PostMessageW(parent, NEX_WM_QUERY_CHANGED, 0, 0);
                }
                return 0;
            }
            if hwnd == state.edit_hwnd
                && state.command_mode_input
                && !state.command_uninstall_quick_mode
                && ((wparam as u32) == 'u' as u32 || (wparam as u32) == 'U' as u32)
            {
                let edit_empty = unsafe { GetWindowTextLengthW(state.edit_hwnd) } <= 0;
                if edit_empty {
                    set_uninstall_quick_mode(parent, state, true, true);
                    unsafe {
                        InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
                        PostMessageW(parent, NEX_WM_QUERY_CHANGED, 0, 0);
                    }
                    return 0;
                }
            }
            // Suppress default control beep for handled launcher keys.
            // Enter submits through WM_KEYDOWN -> NEX_WM_SUBMIT.
            match wparam as u32 {
                10 | 13 | 27 => return 0, // '\n', '\r', ESC
                _ => {}
            }
        }
        if message == windows_sys::Win32::UI::WindowsAndMessaging::WM_SETCURSOR
            && (hwnd == state.help_hwnd || hwnd == state.help_tip_hwnd)
        {
            unsafe {
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
            }
            return 1;
        }
        if message == WM_MOUSEMOVE && hwnd == state.list_hwnd {
            let x = (lparam as u32 & 0xFFFF) as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xFFFF) as i16 as i32;
            let packed = ((y as u32) << 16) | (x as u32 & 0xFFFF);
            let hit = unsafe { SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, packed as isize) };
            let row = (hit & 0xFFFF) as i32;
            let outside = ((hit >> 16) & 0xFFFF) != 0;
            let count = unsafe { SendMessageW(hwnd, LB_GETCOUNT, 0, 0) as i32 };
            let next_hover = if outside || count <= 0 || row < 0 || row >= count {
                -1
            } else if !row_is_selectable(state, row as usize) {
                -1
            } else {
                row
            };

            // During expand/collapse animation, ignore hover-driven selection sync to
            // avoid listbox auto-scroll side effects (top row can jump out of view).
            if state.window_anim.is_some() {
                if state.hover_index != -1 {
                    let previous_hover = state.hover_index;
                    state.hover_index = -1;
                    invalidate_list_row(hwnd, previous_hover);
                }
                return 0;
            }

            // Ignore one initial hover pulse after a fresh results refresh so a stationary
            // cursor does not immediately steal active row/scroll state from row 0.
            if state.suppress_next_hover_sync {
                state.suppress_next_hover_sync = false;
                if state.hover_index != -1 {
                    let previous_hover = state.hover_index;
                    state.hover_index = -1;
                    invalidate_list_row(hwnd, previous_hover);
                }
                return 0;
            }

            if next_hover != state.hover_index {
                let previous_hover = state.hover_index;
                state.hover_index = next_hover;
                invalidate_list_row(hwnd, previous_hover);
                invalidate_list_row(hwnd, next_hover);
            }
        }
        if message == WM_LBUTTONUP && hwnd == state.list_hwnd {
            let count = unsafe { SendMessageW(hwnd, LB_GETCOUNT, 0, 0) as i32 };
            if count > 0 {
                let x = (lparam as u32 & 0xFFFF) as i16 as i32;
                let y = ((lparam as u32 >> 16) & 0xFFFF) as i16 as i32;
                let packed = ((y as u32) << 16) | (x as u32 & 0xFFFF);
                let hit = unsafe { SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, packed as isize) };
                let row = (hit & 0xFFFF) as i32;
                let outside = ((hit >> 16) & 0xFFFF) != 0;
                if !outside && row >= 0 && row < count {
                    if !row_is_selectable(state, row as usize) {
                        unsafe {
                            SendMessageW(hwnd, LB_SETCURSEL, usize::MAX, 0);
                        }
                        return 0;
                    }
                    unsafe {
                        SendMessageW(hwnd, LB_SETCURSEL, row as usize, 0);
                        PostMessageW(parent, NEX_WM_SUBMIT, 0, 0);
                    }
                }
            }
            return 0;
        }
        if message == WM_LBUTTONUP && (hwnd == state.help_hwnd || hwnd == state.help_tip_hwnd) {
            // Click activation is handled once via the parent WM_COMMAND path.
            // Opening from both handlers causes the config file to launch twice.
            return 0;
        }
    }

    if message == WM_KEYDOWN {
        match wparam as u16 {
            VK_ESCAPE => {
                unsafe {
                    PostMessageW(parent, NEX_WM_ESCAPE, 0, 0);
                }
                return 0;
            }
            VK_UP => {
                unsafe {
                    PostMessageW(parent, NEX_WM_MOVE_UP, 0, 0);
                }
                return 0;
            }
            VK_DOWN => {
                unsafe {
                    PostMessageW(parent, NEX_WM_MOVE_DOWN, 0, 0);
                }
                return 0;
            }
            VK_RETURN => {
                unsafe {
                    PostMessageW(parent, NEX_WM_SUBMIT, 0, 0);
                }
                return 0;
            }
            _ => {}
        }
    }

    let Some(state) = state_for(parent) else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };

    let prev_ptr = if hwnd == state.edit_hwnd {
        state.edit_prev_proc
    } else if hwnd == state.list_hwnd {
        state.list_prev_proc
    } else if hwnd == state.help_hwnd {
        state.help_prev_proc
    } else if hwnd == state.help_tip_hwnd {
        state.help_tip_prev_proc
    } else if hwnd == state.footer_hint_hwnd {
        state.footer_hint_prev_proc
    } else {
        0
    };

    if prev_ptr == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    let prev_proc = unsafe {
        std::mem::transmute::<isize, windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC>(prev_ptr)
    };
    let result = unsafe { CallWindowProcW(prev_proc, hwnd, message, wparam, lparam) };
    if hwnd == state.edit_hwnd && message == WM_PAINT {
        paint_edit_placeholder(hwnd, state);
        paint_edit_command_prefix(hwnd, state);
    }
    result
}

fn handle_wheel_input(state: &mut OverlayShellState, wparam: WPARAM) {
    let wheel_delta = wheel_delta_from_wparam(wparam);
    if wheel_delta == 0 {
        return;
    }

    if let Some(anim) = state.window_anim.as_ref() {
        if anim.hide_on_complete {
            return;
        }
        state.pending_wheel_delta = (state.pending_wheel_delta + wheel_delta)
            .clamp(-MAX_PENDING_WHEEL_DELTA, MAX_PENDING_WHEEL_DELTA);
        return;
    }

    scroll_list_by_wheel_delta(state, wheel_delta);
}

fn wheel_delta_from_wparam(wparam: WPARAM) -> i32 {
    ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32
}

fn scroll_list_by_wheel_delta(state: &mut OverlayShellState, wheel_delta: i32) {
    let list_hwnd = state.list_hwnd;
    let count = unsafe { SendMessageW(list_hwnd, LB_GETCOUNT, 0, 0) as i32 };
    if count <= 0 {
        return;
    }

    let current_top = unsafe { SendMessageW(list_hwnd, LB_GETTOPINDEX, 0, 0) as i32 };
    let visible_rows = visible_row_capacity(list_hwnd);
    let max_top = (count - visible_rows).max(0);
    state.wheel_delta_remainder += wheel_delta;
    let notches = state.wheel_delta_remainder / 120;
    if notches == 0 {
        return;
    }
    state.wheel_delta_remainder -= notches * 120;

    let target_top = (current_top - notches * WHEEL_LINES_PER_NOTCH).clamp(0, max_top);
    if target_top == current_top {
        return;
    }
    set_list_top_index_no_anim(list_hwnd, target_top);
}

fn set_list_top_index_no_anim(list_hwnd: HWND, target_top: i32) {
    unsafe {
        SendMessageW(list_hwnd, WM_SETREDRAW as u32, 0, 0);
        SendMessageW(list_hwnd, LB_SETTOPINDEX, target_top as usize, 0);
        SendMessageW(list_hwnd, WM_SETREDRAW as u32, 1, 0);
        InvalidateRect(list_hwnd, std::ptr::null(), 0);
    }
}

fn is_cursor_over_window(hwnd: HWND) -> bool {
    let mut cursor: POINT = unsafe { std::mem::zeroed() };
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut cursor);
        GetWindowRect(hwnd, &mut rect);
    }
    cursor.x >= rect.left && cursor.x < rect.right && cursor.y >= rect.top && cursor.y < rect.bottom
}

fn hide_input_caret(edit_hwnd: HWND) {
    unsafe {
        let _ = HideCaret(edit_hwnd);
    }
}

// ==================== INPUT/WINDOW HELPERS (recovered) ====================

fn set_help_hover_state(hwnd: HWND, state: &mut OverlayShellState, hovered: bool) {
    if state.help_hovered == hovered {
        return;
    }
    state.help_hovered = hovered;
    unsafe {
        InvalidateRect(state.help_hwnd, std::ptr::null(), 0);
    }
    if hovered {
        state.help_tip_visible = true;
        let wide = to_wide(&help_hint_text(state));
        unsafe {
            SetWindowTextW(state.help_tip_hwnd, wide.as_ptr());
            SetTimer(hwnd, TIMER_HELP_HOVER, HELP_HOVER_POLL_MS, None);
            crate::windows_overlay::layout::position_help_tip_popup(state);
            ShowWindow(state.help_tip_hwnd, SW_SHOW);
        }
        unsafe {
            InvalidateRect(state.help_tip_hwnd, std::ptr::null(), 1);
        }
        return;
    }
    if state.help_tip_visible {
        state.help_tip_visible = false;
        unsafe {
            KillTimer(hwnd, TIMER_HELP_HOVER);
            ShowWindow(state.help_tip_hwnd, SW_HIDE);
        }
    } else {
        unsafe {
            KillTimer(hwnd, TIMER_HELP_HOVER);
        }
    }
}

pub(crate) fn sync_help_hover_with_cursor(hwnd: HWND, state: &mut OverlayShellState) {
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut cursor);
    }
    let mut help_rect: RECT = unsafe { std::mem::zeroed() };
    let mut tip_rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetWindowRect(state.help_hwnd, &mut help_rect);
        GetWindowRect(state.help_tip_hwnd, &mut tip_rect);
    }
    let over_help = point_in_rect(&help_rect, cursor);
    let over_tip = state.help_tip_visible && point_in_rect(&tip_rect, cursor);
    set_help_hover_state(hwnd, state, over_help || over_tip);
}

fn point_in_rect(rect: &RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}
