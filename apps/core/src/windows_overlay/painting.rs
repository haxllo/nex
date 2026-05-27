use std::collections::HashSet;

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    FillRgn, FrameRgn, GetDC, GetTextExtentPoint32W, InvalidateRect, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, TextOutW, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, HDC, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetClientRect, GetCursorPos, GetWindowRect, GetWindowTextLengthW, HideCaret, KillTimer,
    SendMessageW, SetTimer, DI_NORMAL, LB_GETCOUNT, LB_GETITEMRECT, LB_GETTOPINDEX, LB_SETTOPINDEX,
    WM_SETREDRAW,
};

use std::time::Instant;

use crate::windows_overlay::animation::blend_color;
use crate::windows_overlay::layout::{
    apply_edit_text_rect, compute_input_text_rect, input_line_height_for_edit, visible_row_capacity,
};
use crate::windows_overlay::state::{state_for, OverlayShellState};
use crate::windows_overlay::types::*;
pub(crate) fn paint_edit_placeholder(edit_hwnd: HWND, state: &OverlayShellState) {
    let text_len = unsafe { GetWindowTextLengthW(edit_hwnd) };
    if text_len > 0 {
        return;
    }

    let mut text_rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        SendMessageW(
            edit_hwnd,
            EM_GETRECT,
            0,
            &mut text_rect as *mut RECT as LPARAM,
        );
    }
    if text_rect.right <= text_rect.left || text_rect.bottom <= text_rect.top {
        let mut client: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(edit_hwnd, &mut client);
        }
        let line_height = input_line_height_for_edit(edit_hwnd, state.input_font);
        text_rect = compute_input_text_rect(
            client.right - client.left,
            client.bottom - client.top,
            line_height,
            state.command_mode_input,
            state.command_uninstall_quick_mode,
        );
    }
    if text_rect.right <= text_rect.left {
        return;
    }

    let hdc = unsafe { GetDC(edit_hwnd) };
    if hdc.is_null() {
        return;
    }

    unsafe {
        let old_font = SelectObject(hdc, state.input_font as _);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, state.palette.text_secondary);
        let placeholder_text = if state.placeholder_hint.is_empty() {
            if state.command_mode_input {
                COMMAND_INPUT_PLACEHOLDER_TEXT
            } else {
                INPUT_PLACEHOLDER_TEXT
            }
        } else {
            state.placeholder_hint.as_str()
        };

        let placeholder = to_wide(placeholder_text);
        DrawTextW(
            hdc,
            placeholder.as_ptr(),
            -1,
            &mut text_rect,
            DT_LEFT | DT_SINGLELINE | DT_EDITCONTROL | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, old_font);
        ReleaseDC(edit_hwnd, hdc);
    }
}

pub(crate) fn paint_edit_search_icon(edit_hwnd: HWND, state: &OverlayShellState) {
    if state.command_mode_input {
        return;
    }

    let mut text_rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        SendMessageW(
            edit_hwnd,
            EM_GETRECT,
            0,
            &mut text_rect as *mut RECT as LPARAM,
        );
    }
    if text_rect.right <= text_rect.left || text_rect.bottom <= text_rect.top {
        let mut client: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(edit_hwnd, &mut client);
        }
        let line_height = input_line_height_for_edit(edit_hwnd, state.input_font);
        text_rect = compute_input_text_rect(
            client.right - client.left,
            client.bottom - client.top,
            line_height,
            false,
            false,
        );
    }
    if text_rect.right <= text_rect.left {
        return;
    }

    let hdc = unsafe { GetDC(edit_hwnd) };
    if hdc.is_null() {
        return;
    }

    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, state.palette.text_hint);
        let old = SelectObject(hdc, state.search_icon_font as _);
        let mut icon_rect = RECT {
            left: SEARCH_ICON_LEFT,
            top: text_rect.top,
            right: text_rect.left,
            bottom: text_rect.bottom,
        };
        let search_wide = to_wide(SEARCH_ICON_TEXT);
        DrawTextW(
            hdc,
            search_wide.as_ptr(),
            -1,
            &mut icon_rect,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        SelectObject(hdc, old);
        ReleaseDC(edit_hwnd, hdc);
    }
}

pub(crate) fn paint_edit_command_prefix(edit_hwnd: HWND, state: &OverlayShellState) {
    if !state.command_mode_input {
        return;
    }

    let mut text_rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        SendMessageW(
            edit_hwnd,
            EM_GETRECT,
            0,
            &mut text_rect as *mut RECT as LPARAM,
        );
    }
    if text_rect.right <= text_rect.left || text_rect.bottom <= text_rect.top {
        let mut client: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(edit_hwnd, &mut client);
        }
        let line_height = input_line_height_for_edit(edit_hwnd, state.input_font);
        text_rect = compute_input_text_rect(
            client.right - client.left,
            client.bottom - client.top,
            line_height,
            true,
            state.command_uninstall_quick_mode,
        );
    }
    if text_rect.right <= text_rect.left {
        return;
    }

    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetClientRect(edit_hwnd, &mut client);
    }
    let mut prefix_rect = text_rect;
    prefix_rect.top = client.top;
    prefix_rect.bottom = client.bottom;
    let reserved = COMMAND_PREFIX_RESERVED_WIDTH + COMMAND_PREFIX_GAP + COMMAND_PREFIX_LEFT_SHIFT;
    prefix_rect.left = (text_rect.left - reserved).max(0);
    prefix_rect.right = (prefix_rect.left + COMMAND_PREFIX_RESERVED_WIDTH)
        .min((text_rect.left - COMMAND_PREFIX_GAP).max(prefix_rect.left + 1));

    let hdc = unsafe { GetDC(edit_hwnd) };
    if hdc.is_null() {
        return;
    }

    unsafe {
        let prefix_font = if state.command_prefix_font != 0 {
            state.command_prefix_font
        } else {
            state.input_font
        };
        let old_font = SelectObject(hdc, prefix_font as _);
        SetBkMode(hdc, TRANSPARENT as i32);
        let prefix_color = blend_color(
            state.palette.input_bg,
            state.palette.text_hint,
            COMMAND_PREFIX_OPACITY,
        );
        let embolden_color = blend_color(
            state.palette.input_bg,
            state.palette.text_hint,
            COMMAND_PREFIX_EMBOLDEN_OPACITY,
        );
        let prefix = to_wide(COMMAND_PREFIX_TEXT);
        let mut left_pass_rect = prefix_rect;
        left_pass_rect.left -= COMMAND_PREFIX_EMBOLDEN_OFFSET_PX;
        left_pass_rect.right -= COMMAND_PREFIX_EMBOLDEN_OFFSET_PX;
        SetTextColor(hdc, embolden_color);
        DrawTextW(
            hdc,
            prefix.as_ptr(),
            -1,
            &mut left_pass_rect,
            DT_CENTER | DT_SINGLELINE | DT_EDITCONTROL | DT_VCENTER,
        );
        let mut right_pass_rect = prefix_rect;
        right_pass_rect.left += COMMAND_PREFIX_EMBOLDEN_OFFSET_PX;
        right_pass_rect.right += COMMAND_PREFIX_EMBOLDEN_OFFSET_PX;
        DrawTextW(
            hdc,
            prefix.as_ptr(),
            -1,
            &mut right_pass_rect,
            DT_CENTER | DT_SINGLELINE | DT_EDITCONTROL | DT_VCENTER,
        );
        SetTextColor(hdc, prefix_color);
        DrawTextW(
            hdc,
            prefix.as_ptr(),
            -1,
            &mut prefix_rect,
            DT_CENTER | DT_SINGLELINE | DT_EDITCONTROL | DT_VCENTER,
        );

        if state.command_uninstall_quick_mode {
            let progress = command_badge_progress(state);
            let opacity = (COMMAND_PREFIX_OPACITY * progress).clamp(0.0, 1.0);
            let badge_color = blend_color(state.palette.input_bg, state.palette.text_hint, opacity);
            let mut badge_rect = RECT {
                left: prefix_rect.right + COMMAND_BADGE_GAP_FROM_PREFIX,
                top: client.top,
                right: (text_rect.left - 2)
                    .max(prefix_rect.right + COMMAND_BADGE_GAP_FROM_PREFIX + 1),
                bottom: client.bottom,
            };
            let slide_px = ((1.0 - progress) * COMMAND_BADGE_SLIDE_PX as f32).round() as i32;
            badge_rect.left += slide_px;
            badge_rect.right += slide_px;
            let badge = to_wide(COMMAND_BADGE_TEXT);
            let badge_font = if state.command_badge_font != 0 {
                state.command_badge_font
            } else if state.input_font != 0 {
                state.input_font
            } else {
                prefix_font
            };
            let prev_badge_font = SelectObject(hdc, badge_font as _);
            SetTextColor(hdc, badge_color);
            DrawTextW(
                hdc,
                badge.as_ptr(),
                -1,
                &mut badge_rect,
                DT_LEFT | DT_SINGLELINE | DT_EDITCONTROL | DT_VCENTER,
            );
            SelectObject(hdc, prev_badge_font);
        }

        SelectObject(hdc, old_font);
        ReleaseDC(edit_hwnd, hdc);
    }
}

fn command_badge_progress(state: &OverlayShellState) -> f32 {
    let Some(start) = state.command_badge_anim_start else {
        return 1.0;
    };
    let elapsed_ms = start.elapsed().as_millis() as f32;
    (elapsed_ms / COMMAND_BADGE_ANIM_MS as f32).clamp(0.0, 1.0)
}

pub(crate) fn command_badge_animation_tick(state: &mut OverlayShellState) -> bool {
    if !state.command_uninstall_quick_mode {
        state.command_badge_anim_start = None;
        return false;
    }
    let Some(start) = state.command_badge_anim_start else {
        return false;
    };
    let elapsed_ms = start.elapsed().as_millis() as u32;
    unsafe {
        InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
    }
    if elapsed_ms >= COMMAND_BADGE_ANIM_MS {
        state.command_badge_anim_start = None;
        return false;
    }
    true
}

pub(crate) fn set_uninstall_quick_mode(
    hwnd: HWND,
    state: &mut OverlayShellState,
    enabled: bool,
    animate: bool,
) {
    let enabled = enabled && state.command_mode_input;
    if enabled == state.command_uninstall_quick_mode {
        return;
    }

    state.command_uninstall_quick_mode = enabled;
    if enabled {
        state.command_badge_anim_start = if animate { Some(Instant::now()) } else { None };
        unsafe {
            if animate {
                SetTimer(hwnd, TIMER_COMMAND_BADGE_FADE, ANIM_FRAME_MS as u32, None);
            } else {
                KillTimer(hwnd, TIMER_COMMAND_BADGE_FADE);
            }
        }
    } else {
        state.command_badge_anim_start = None;
        unsafe {
            KillTimer(hwnd, TIMER_COMMAND_BADGE_FADE);
        }
    }

    apply_edit_text_rect(
        state.edit_hwnd,
        state.command_mode_input,
        state.command_uninstall_quick_mode,
    );
    unsafe {
        InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
    }
}

pub(crate) fn hide_input_caret(edit_hwnd: HWND) {
    unsafe {
        let _ = HideCaret(edit_hwnd);
    }
}

fn results_content_progress(state: &OverlayShellState) -> f32 {
    let Some(start) = state.results_content_anim_start else {
        return 1.0;
    };
    let elapsed_ms = start.elapsed().as_millis() as f32;
    (elapsed_ms / RESULTS_CONTENT_FADE_MS as f32).clamp(0.0, 1.0)
}

pub(crate) fn draw_panel_background(hwnd: HWND) {
    let Some(state) = state_for(hwnd) else {
        return;
    };

    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };

    // D2D path: use renderer for hardware-accelerated painting
    if state.d2d.is_some() {
        let width_f = (client.right - client.left).max(0) as f32;
        let height_f = (client.bottom - client.top).max(0) as f32;
        if width_f <= 0.0 || height_f <= 0.0 { return; }

        // Must call BeginPaint/EndPaint to validate the update region even in D2D path
        let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        let needs_end_paint = !hdc.is_null();

        let panel_bg = state.palette.panel_bg;
        let panel_border = state.palette.panel_border;
        let dwm = state.dwm_rounded_enabled;
        let draw_divider = state.results_visible;

        let renderer = state.d2d.as_mut().unwrap();
        if !renderer.begin_draw() {
            if needs_end_paint { unsafe { EndPaint(hwnd, &paint); } }
            return;
        }

        renderer.clear(panel_bg);

        if dwm {
            renderer.fill_rectangle(
                &D2D_RECT_F { left: 0.0, top: 0.0, right: width_f, bottom: height_f },
                panel_bg,
            );
        } else {
            let radius = PANEL_RADIUS as f32;
            renderer.fill_rounded_rectangle(
                &D2D_RECT_F { left: 0.0, top: 0.0, right: width_f, bottom: height_f },
                radius,
                panel_border,
            );
            renderer.fill_rounded_rectangle(
                &D2D_RECT_F { left: 1.0, top: 1.0, right: width_f - 1.0, bottom: height_f - 1.0 },
                (radius - 2.0).max(2.0),
                panel_bg,
            );
        }

        if draw_divider {
            let left = 1.0;
            let right = (width_f - 1.0).max(left + 1.0);
            let y = COMPACT_HEIGHT as f32 + DIVIDER_TOP_SPACING as f32;
            renderer.draw_line(left, y, right, y, panel_border, 1.0);
        }

        renderer.end_draw();
        if needs_end_paint { unsafe { EndPaint(hwnd, &paint); } }
        return;
    }
}

pub(crate) fn draw_list_row(hwnd: HWND, dis: &mut DRAWITEMSTRUCT) {
    if dis.itemID == u32::MAX {
        return;
    }

    let Some(state) = state_for(hwnd) else {
        return;
    };

    let item_index = dis.itemID as i32;
    let row = state
        .rows
        .get(item_index as usize)
        .cloned()
        .unwrap_or_else(|| OverlayRow {
            role: OverlayRowRole::Item,
            result_index: -1,
            kind: "file".to_string(),
            title: String::new(),
            path: String::new(),
            icon_path: String::new(),
        });

    let content_progress = results_content_progress(state);
    let offset_y = ((1.0 - content_progress) * 4.0).round() as i32;
    let status_row = matches!(row.role, OverlayRowRole::Status);
    let section_row = matches!(row.role, OverlayRowRole::Header);
    let selected_flag = (dis.itemState & ODS_SELECTED as u32) != 0;
    let hovered = state.hover_index == item_index;
    let palette = state.palette;
    let selected_visible = !status_row
        && !section_row
        && if state.hover_index >= 0 {
            hovered
        } else {
            selected_flag
        };
    let has_meta = !row.path.trim().is_empty();
    let icon_container_size = state.icon_container_size;
    let _dwm_rounded = state.dwm_rounded_enabled;

    let gdi_bg_rect = RECT {
        left: dis.rcItem.left,
        top: dis.rcItem.top,
        right: dis.rcItem.right,
        bottom: dis.rcItem.bottom,
    };

    unsafe {
        FillRect(dis.hDC, &gdi_bg_rect, state.results_brush as _);
    }

    // --- Section row ---
    if section_row {
        let section_title = row.title.trim();
        let section_title = if section_title.is_empty() {
            "Section"
        } else {
            section_title
        };
        let sect_wide = to_wide_no_nul(section_title);
        let sect_left = dis.rcItem.left + ROW_INSET_X;
        let sect_top = dis.rcItem.top + ((ROW_HEIGHT - HEADER_ROW_LABEL_HEIGHT).max(0) / 2);
        let sect_right = dis.rcItem.right - ROW_INSET_X;

        let mut sect_rect = RECT {
            left: sect_left, top: sect_top,
            right: sect_right, bottom: sect_top + HEADER_ROW_LABEL_HEIGHT,
        };
        unsafe {
            let old = SelectObject(dis.hDC, state.header_font as _);
            SetBkMode(dis.hDC, TRANSPARENT as i32);
            SetTextColor(dis.hDC, palette.text_section);
            DrawTextW(dis.hDC, sect_wide.as_ptr(), sect_wide.len() as i32, &mut sect_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            SelectObject(dis.hDC, old);
        }

        // Draw separator line
        let sect_width = unsafe {
            let mut text_size: SIZE = std::mem::zeroed();
            let old_font = SelectObject(dis.hDC, state.header_font as _);
            let _ = GetTextExtentPoint32W(dis.hDC, sect_wide.as_ptr(), sect_wide.len() as i32, &mut text_size);
            SelectObject(dis.hDC, old_font);
            text_size.cx
        };
        if sect_width > 0 {
            let line_left = (sect_left + sect_width + HEADER_ROW_LINE_GAP).min(sect_right);
            if line_left < sect_right {
                let line_rect = RECT {
                    left: line_left,
                    top: sect_top + HEADER_ROW_LABEL_HEIGHT / 2,
                    right: sect_right,
                    bottom: sect_top + HEADER_ROW_LABEL_HEIGHT / 2 + HEADER_ROW_LINE_HEIGHT,
                };
                unsafe {
                    FillRect(dis.hDC, &line_rect, state.row_separator_brush as _);
                }
            }
        }
        return;
    }

    // --- Selection highlight (GDI-only pre-blended fill) ---
    // GDI+ antialiased rounded rect was removed because mixing GDI+ and GDI
    // on the same HDC for a WS_EX_LAYERED child window causes each GDI+ →
    // GDI flush to be immediately visible (DWM does not redirect/batch GDI
    // updates for child windows of layered parents), creating a screen-tear
    // effect during hover transitions.  A GDI-only FillRect with a pre-blended
    // color avoids the GDI+ rendering pipeline entirely.
    if !status_row && (selected_visible || hovered) {
        let sel_x = dis.rcItem.left + 3;
        let sel_y = dis.rcItem.top + ROW_VERTICAL_INSET + 1 + offset_y;
        let sel_w = dis.rcItem.right - 3 - sel_x;
        let sel_h = dis.rcItem.bottom - ROW_VERTICAL_INSET - 1 + offset_y - sel_y;

        if sel_w > 0 && sel_h > 0 {
            let is_dark = (palette.results_bg & 0xFF) < 128;
            let tint = if is_dark { 0xFFFFFF } else { 0x000000 };
            let highlight_color = blend_color(palette.results_bg, tint, 0.14);
            let sel_rect = RECT {
                left: sel_x,
                top: sel_y,
                right: sel_x + sel_w,
                bottom: sel_y + sel_h,
            };
            unsafe {
                let brush = CreateSolidBrush(highlight_color);
                if !brush.is_null() {
                    FillRect(dis.hDC, &sel_rect, brush as _);
                    DeleteObject(brush as _);
                }
            }
        }
    }

    // --- Icon ---
    if !status_row {
        let icon_key = crate::windows_overlay::icon_cache::icon_cache_key(&row);
        let icon_handle = state.icon_cache.get(&icon_key).copied().unwrap_or(0);
        if icon_handle != 0 {
            let icon_draw_size = state.icon_draw_size;
            let total_height = if has_meta {
                ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT
            } else {
                ROW_TITLE_BLOCK_HEIGHT
            };
            let text_top = dis.rcItem.top + ((ROW_HEIGHT - total_height).max(0) / 2) + offset_y;
            let icon_top = text_top + ((total_height as f32 - icon_container_size as f32) / 2.0) as i32;
            let icon_offset = ((icon_container_size as f32 - icon_draw_size as f32) / 2.0) as i32;
            unsafe {
                DrawIconEx(
                    dis.hDC,
                    dis.rcItem.left + ROW_INSET_X + icon_offset,
                    icon_top + icon_offset,
                    icon_handle as _,
                    icon_draw_size,
                    icon_draw_size,
                    0,
                    std::ptr::null_mut(),
                    DI_NORMAL,
                );
            }
        }
    }

    // --- GDI Text ---
    let text_left = if status_row {
        dis.rcItem.left + ROW_INSET_X
    } else {
        dis.rcItem.left + ROW_INSET_X + icon_container_size as i32 + ROW_ICON_GAP
    };
    let text_right = dis.rcItem.right - ROW_INSET_X;
    let total_height = if has_meta {
        ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT
    } else {
        ROW_TITLE_BLOCK_HEIGHT
    };
    let text_top = dis.rcItem.top + ((ROW_HEIGHT - total_height).max(0) / 2) + offset_y;

    if status_row {
        let mut title_rect = RECT {
            left: text_left, top: text_top,
            right: text_right, bottom: text_top + ROW_TITLE_BLOCK_HEIGHT,
        };
        let wide = to_wide(&row.title);
        unsafe {
            let old = SelectObject(dis.hDC, state.status_font as _);
            SetBkMode(dis.hDC, TRANSPARENT as i32);
            SetTextColor(dis.hDC, palette.text_secondary);
            DrawTextW(dis.hDC, wide.as_ptr(), -1, &mut title_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            SelectObject(dis.hDC, old);
        }
    } else {
        let mut title_rect = RECT {
            left: text_left, top: text_top,
            right: text_right, bottom: text_top + ROW_TITLE_BLOCK_HEIGHT,
        };
        let wide = to_wide(&row.title);
        unsafe {
            let old = SelectObject(dis.hDC, state.title_font as _);
            SetBkMode(dis.hDC, TRANSPARENT as i32);
            SetTextColor(dis.hDC, palette.text_primary);
            DrawTextW(dis.hDC, wide.as_ptr(), -1, &mut title_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            SelectObject(dis.hDC, old);
        }
    }

    if has_meta && !status_row {
        let mut path_rect = RECT {
            left: text_left,
            top: text_top + ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP,
            right: text_right,
            bottom: text_top + ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT,
        };
        let wide = to_wide(&row.path);
        unsafe {
            let old = SelectObject(dis.hDC, state.meta_font as _);
            SetBkMode(dis.hDC, TRANSPARENT as i32);
            SetTextColor(dis.hDC, palette.text_secondary);
            DrawTextW(dis.hDC, wide.as_ptr(), -1, &mut path_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            SelectObject(dis.hDC, old);
        }
    }
}

pub(crate) fn row_is_selectable(state: &OverlayShellState, index: usize) -> bool {
    state.rows.get(index).is_some_and(|row| {
        matches!(row.role, OverlayRowRole::Item | OverlayRowRole::TopHit) && row.result_index >= 0
    })
}

pub(crate) fn handle_wheel_input(state: &mut OverlayShellState, wparam: WPARAM) {
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

pub(crate) fn wheel_delta_from_wparam(wparam: WPARAM) -> i32 {
    ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32
}

pub(crate) fn scroll_list_by_wheel_delta(state: &mut OverlayShellState, wheel_delta: i32) {
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

pub(crate) fn set_list_top_index_no_anim(list_hwnd: HWND, target_top: i32) {
    unsafe {
        SendMessageW(list_hwnd, WM_SETREDRAW as u32, 0, 0);
        SendMessageW(list_hwnd, LB_SETTOPINDEX, target_top as usize, 0);
        SendMessageW(list_hwnd, WM_SETREDRAW as u32, 1, 0);
        InvalidateRect(list_hwnd, std::ptr::null(), 0);
    }
}

pub(crate) fn is_cursor_over_window(hwnd: HWND) -> bool {
    let mut cursor: POINT = unsafe { std::mem::zeroed() };
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut cursor);
        GetWindowRect(hwnd, &mut rect);
    }
    cursor.x >= rect.left && cursor.x < rect.right && cursor.y >= rect.top && cursor.y < rect.bottom
}

fn d2d_fit_text_with_ellipsis(
    renderer: &crate::windows_overlay::d2d_renderer::D2dRenderer,
    format: &IDWriteTextFormat,
    text: &str,
    max_width: f32,
) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    if renderer.measure_text_width(format, text) <= max_width {
        return text.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = renderer.measure_text_width(format, ellipsis);
    if ellipsis_width >= max_width {
        return String::new();
    }

    let mut output = String::new();
    for ch in text.chars() {
        let mut candidate = output.clone();
        candidate.push(ch);
        if renderer.measure_text_width(format, &candidate) + ellipsis_width > max_width {
            break;
        }
        output.push(ch);
    }
    output.push_str(ellipsis);
    output
}

fn d2d_draw_highlighted_title(
    renderer: &mut crate::windows_overlay::d2d_renderer::D2dRenderer,
    format: &IDWriteTextFormat,
    rect: &D2D_RECT_F,
    title: &str,
    query: &str,
    base_color: u32,
    highlight_color: u32,
) {
    let max_width = rect.right - rect.left;
    if max_width <= 0.0 || title.trim().is_empty() {
        return;
    }

    let display = d2d_fit_text_with_ellipsis(renderer, format, title, max_width);
    if display.is_empty() {
        return;
    }

    let highlighted = fuzzy_match_positions(&display, query);
    if highlighted.is_empty() {
        renderer.dc_draw_text(&display, rect, base_color, format);
        return;
    }

    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut current_hl = false;
    for (i, ch) in display.chars().enumerate() {
        let is_hl = highlighted.contains(&i);
        if current.is_empty() {
            current_hl = is_hl;
            current.push(ch);
        } else if is_hl == current_hl {
            current.push(ch);
        } else {
            runs.push((std::mem::take(&mut current), current_hl));
            current_hl = is_hl;
            current.push(ch);
        }
    }
    if !current.is_empty() {
        runs.push((current, current_hl));
    }

    let (_, text_height) = renderer.measure_text_size(format, "Wy");
    let y = rect.top + ((rect.bottom - rect.top - text_height).max(0.0) / 2.0);

    let mut measurements: Vec<(String, bool, f32)> = Vec::with_capacity(runs.len());
    for (text, is_hl) in &runs {
        let w = renderer.measure_text_width(format, text);
        measurements.push((text.clone(), *is_hl, w));
    }

    let mut x = rect.left;
    for (text, is_hl, w) in &measurements {
        if x >= rect.right {
            break;
        }
        let run_rect = D2D_RECT_F {
            left: x,
            top: y,
            right: (x + w).min(rect.right),
            bottom: y + text_height,
        };
        if run_rect.right > run_rect.left {
            renderer.dc_draw_text(
                text,
                &run_rect,
                if *is_hl { highlight_color } else { base_color },
                format,
            );
        }
        x += w;
    }
}

fn d2d_draw_icon_glyph(
    renderer: &mut crate::windows_overlay::d2d_renderer::D2dRenderer,
    row: &OverlayRow,
    icon_rect: &D2D_RECT_F,
    color: u32,
) -> bool {
    let is_action = row.kind.eq_ignore_ascii_case("action");
    let codepoint = if is_action {
        let lower = row.title.to_ascii_lowercase();
        if lower.contains("web") || lower.contains("search") {
            0xE721
        } else if lower.contains("uninstall") || lower.contains("remove") {
            0xE74D
        } else if lower.contains("clipboard") {
            0xE8C8
        } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer") {
            0xE713
        } else if lower.contains("diagnostic") || lower.contains("bundle") || lower.contains("support") {
            0xE8A5
        } else if lower.contains("log") {
            0xE8B7
        } else if lower.contains("rebuild") || lower.contains("index") || lower.contains("refresh") {
            0xE895
        } else {
            0xE756
        }
    } else {
        match row.kind.to_ascii_lowercase().as_str() {
            "app" => 0xE714,
            "folder" => 0xE8B7,
            "file" => 0xE8A5,
            _ => return false,
        }
    };

    let Some(ch) = char::from_u32(codepoint) else { return false };
    let glyph = ch.to_string();

    if let Some(icon_fmt) = renderer.icon_text_format(true).map(|f| f.clone()) {
        renderer.dc_draw_text(&glyph, icon_rect, color, &icon_fmt);
        return true;
    }
    if let Some(icon_fmt) = renderer.icon_text_format(false).map(|f| f.clone()) {
        renderer.dc_draw_text(&glyph, icon_rect, color, &icon_fmt);
        return true;
    }
    false
}

fn fuzzy_match_positions(text: &str, query: &str) -> HashSet<usize> {
    let query = query.trim();
    if query.is_empty() {
        return HashSet::new();
    }

    let mut matched = HashSet::new();
    let mut text_iter = text.chars().enumerate();

    for q in query.chars() {
        let q_lower = q.to_ascii_lowercase();
        let mut found = None;
        for (index, t) in text_iter.by_ref() {
            if t.to_ascii_lowercase() == q_lower {
                found = Some(index);
                break;
            }
        }
        if let Some(index) = found {
            matched.insert(index);
        } else {
            return HashSet::new();
        }
    }

    matched
}

fn measure_text_width(hdc: HDC, text: &str) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let wide = to_wide_no_nul(text);
    let mut size: SIZE = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size) };
    if ok == 0 {
        0
    } else {
        size.cx
    }
}

fn icon_glyph_for_kind(kind: &str) -> &'static str {
    if kind.eq_ignore_ascii_case("app") {
        "A"
    } else if kind.eq_ignore_ascii_case("action") {
        ">"
    } else if kind.eq_ignore_ascii_case("clipboard") {
        "C"
    } else if kind.eq_ignore_ascii_case("folder") {
        "D"
    } else {
        "F"
    }
}

fn icon_glyph_for_row(row: &OverlayRow) -> &'static str {
    if !row.kind.eq_ignore_ascii_case("action") {
        return icon_glyph_for_kind(&row.kind);
    }
    let lower = row.title.to_ascii_lowercase();
    if lower.contains("web") || lower.contains("search") {
        "W"
    } else if lower.contains("clipboard") {
        "C"
    } else if lower.contains("config") || lower.contains("setting") {
        "G"
    } else if lower.contains("diagnostic") || lower.contains("bundle") {
        "D"
    } else if lower.contains("log") {
        "L"
    } else if lower.contains("rebuild") || lower.contains("index") {
        "R"
    } else {
        ">"
    }
}

// ==================== PAINT HELPERS (recovered) ====================

pub(crate) fn paint_help_tip(hwnd: HWND, state: &OverlayShellState) {
    if state.help_tip_brush == 0 || state.help_tip_border_brush == 0 {
        return;
    }
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.is_null() {
            return;
        }
        let width = paint.rcPaint.right - paint.rcPaint.left;
        let height = paint.rcPaint.bottom - paint.rcPaint.top;
        if width <= 0 || height <= 0 {
            EndPaint(hwnd, &paint);
            return;
        }
        let bg_region = CreateRoundRectRgn(
            0,
            0,
            width + 1,
            height + 1,
            HELP_TIP_RADIUS,
            HELP_TIP_RADIUS,
        );
        FillRgn(hdc, bg_region, state.help_tip_brush as _);
        DeleteObject(bg_region as _);
        let border_region = CreateRoundRectRgn(
            0,
            0,
            width + 1,
            height + 1,
            HELP_TIP_RADIUS,
            HELP_TIP_RADIUS,
        );
        FrameRgn(hdc, border_region, state.help_tip_border_brush as _, 1, 1);
        DeleteObject(border_region as _);
        let old_font = if state.help_tip_font != 0 {
            SelectObject(hdc, state.help_tip_font as _)
        } else {
            std::ptr::null_mut()
        };
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, state.palette.help_tip_text);
        let mut text_rect = RECT {
            left: HELP_TIP_TEXT_PAD_X,
            top: 0,
            right: width - HELP_TIP_TEXT_PAD_X,
            bottom: height,
        };
        let text = to_wide(&help_hint_text(state));
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut text_rect,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        if !old_font.is_null() {
            SelectObject(hdc, old_font);
        }
        EndPaint(hwnd, &paint);
    }
}

pub(crate) fn paint_footer_hint(hwnd: HWND, state: &mut OverlayShellState) {
    if state.panel_brush == 0 {
        return;
    }
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.is_null() {
            return;
        }
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let width = client.right - client.left;
        let height = client.bottom - client.top;
        if width <= 0 || height <= 0 {
            EndPaint(hwnd, &paint);
            return;
        }
        FillRect(hdc, &client, state.panel_brush as _);
        let separator_brush = state.gdi_cache.brush(state.palette.panel_border);
        let separator_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: FOOTER_SEPARATOR_HEIGHT,
        };
        FillRect(hdc, &separator_rect, separator_brush as _);

        let footer_font = if state.footer_font != 0 {
            state.footer_font
        } else if state.meta_font != 0 {
            state.meta_font
        } else {
            state.hint_font
        };
        let old_hint_font = if footer_font != 0 {
            SelectObject(hdc, footer_font as _)
        } else {
            std::ptr::null_mut()
        };
        SetBkMode(hdc, TRANSPARENT as i32);

        let content_top = (FOOTER_SEPARATOR_HEIGHT + FOOTER_SEPARATOR_TO_CONTENT_GAP).min(height);
        let content_bottom = (height - FOOTER_CONTENT_PAD_Y).max(content_top + 1);
        draw_footer_hints_centered(hdc, state, width, content_top, content_bottom);

        if !old_hint_font.is_null() {
            SelectObject(hdc, old_hint_font);
        }
        EndPaint(hwnd, &paint);
    }
}

fn draw_footer_hints_centered(
    hdc: HDC,
    state: &OverlayShellState,
    width: i32,
    content_top: i32,
    content_bottom: i32,
) {
    let full_width = footer_group_width(hdc, FOOTER_HINT_LABEL_OPEN, &[FOOTER_KEY_ENTER])
        + FOOTER_HINT_GROUP_GAP
        + footer_group_width(
            hdc,
            FOOTER_HINT_LABEL_MOVE,
            &[FOOTER_KEY_UP, FOOTER_KEY_DOWN],
        )
        + FOOTER_HINT_GROUP_GAP
        + footer_group_width(hdc, FOOTER_HINT_LABEL_CLOSE, &[FOOTER_KEY_ESC]);
    let medium_width = footer_group_width(hdc, FOOTER_HINT_LABEL_OPEN, &[FOOTER_KEY_ENTER])
        + FOOTER_HINT_GROUP_GAP
        + footer_group_width(hdc, FOOTER_HINT_LABEL_CLOSE, &[FOOTER_KEY_ESC]);
    let min_width = footer_group_width(hdc, FOOTER_HINT_LABEL_OPEN, &[FOOTER_KEY_ENTER]);
    let available_left = FOOTER_CONTENT_PAD_X.min(width.max(0));
    let available_right = (width - FOOTER_CONTENT_PAD_X).max(available_left);
    let available_width = (available_right - available_left).max(0);

    if available_width >= full_width {
        let block_left = available_left + ((available_width - full_width) / 2);
        let mut right_cursor = block_left + full_width;
        right_cursor = draw_footer_hint_group_right(
            hdc,
            state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_CLOSE,
            &[FOOTER_KEY_ESC],
        );
        right_cursor -= FOOTER_HINT_GROUP_GAP;
        right_cursor = draw_footer_hint_group_right(
            hdc,
            state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_MOVE,
            &[FOOTER_KEY_UP, FOOTER_KEY_DOWN],
        );
        right_cursor -= FOOTER_HINT_GROUP_GAP;
        let _ = draw_footer_hint_group_right(
            hdc,
            state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_OPEN,
            &[FOOTER_KEY_ENTER],
        );
    } else if available_width >= medium_width {
        let block_left = available_left + ((available_width - medium_width) / 2);
        let mut right_cursor = block_left + medium_width;
        right_cursor = draw_footer_hint_group_right(
            hdc,
            state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_CLOSE,
            &[FOOTER_KEY_ESC],
        );
        right_cursor -= FOOTER_HINT_GROUP_GAP;
        let _ = draw_footer_hint_group_right(
            hdc,
            state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_OPEN,
            &[FOOTER_KEY_ENTER],
        );
    } else if available_width >= min_width {
        let block_left = available_left + ((available_width - min_width) / 2);
        let right_cursor = block_left + min_width;
        let _ = draw_footer_hint_group_right(
            hdc,
            state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_OPEN,
            &[FOOTER_KEY_ENTER],
        );
    }
}

fn footer_group_width(hdc: HDC, label: &str, keys: &[&str]) -> i32 {
    if keys.is_empty() {
        return measure_text_width(hdc, label).max(1);
    }

    let mut total = measure_text_width(hdc, label).max(1) + FOOTER_HINT_LABEL_GAP;
    for (index, key) in keys.iter().enumerate() {
        total += footer_keycap_width(hdc, key);
        if index + 1 < keys.len() {
            total += FOOTER_KEYCAP_GAP;
        }
    }
    total
}

fn draw_footer_hint_group_right(
    hdc: HDC,
    state: &OverlayShellState,
    right: i32,
    content_top: i32,
    content_bottom: i32,
    label: &str,
    keys: &[&str],
) -> i32 {
    let mut cursor = right;
    for (index, key) in keys.iter().rev().enumerate() {
        cursor = draw_footer_keycap_right(hdc, state, cursor, content_top, content_bottom, key);
        if index + 1 < keys.len() {
            cursor -= FOOTER_KEYCAP_GAP;
        }
    }
    cursor -= FOOTER_HINT_LABEL_GAP;
    draw_footer_label_right(
        hdc,
        cursor,
        content_top,
        content_bottom,
        label,
        state.palette.text_hint_footer,
    )
}

fn footer_keycap_width(hdc: HDC, text: &str) -> i32 {
    measure_text_width(hdc, text).max(1)
}

fn draw_footer_keycap_right(
    hdc: HDC,
    state: &OverlayShellState,
    right: i32,
    content_top: i32,
    content_bottom: i32,
    text: &str,
) -> i32 {
    let text_width = footer_keycap_width(hdc, text);
    let left = (right - text_width).max(0);
    let content_height = (content_bottom - content_top).max(1);

    unsafe {
        let key_font = if state.hint_font != 0 {
            state.hint_font
        } else {
            state.footer_font
        };
        let old_font = if key_font != 0 {
            SelectObject(hdc, key_font as _)
        } else {
            std::ptr::null_mut()
        };
        let text_color = blend_color(state.palette.results_bg, state.palette.text_primary, 0.94);
        SetTextColor(hdc, text_color);
        let text_wide = to_wide_no_nul(text);
        let text_size = measure_text_size(hdc, text);
        let text_y =
            content_top + ((content_height - text_size.cy).max(0) / 2) + FOOTER_KEY_TEXT_SHIFT_Y;
        TextOutW(
            hdc,
            left,
            text_y,
            text_wide.as_ptr(),
            text_wide.len() as i32,
        );

        if !old_font.is_null() {
            SelectObject(hdc, old_font);
        }
    }

    left
}

fn draw_footer_label_right(
    hdc: HDC,
    right: i32,
    content_top: i32,
    content_bottom: i32,
    text: &str,
    color: u32,
) -> i32 {
    let text_width = measure_text_width(hdc, text).max(1);
    let left = (right - text_width).max(0);
    unsafe {
        SetTextColor(hdc, color);
        let text_wide = to_wide_no_nul(text);
        let text_size = measure_text_size(hdc, text);
        let content_height = (content_bottom - content_top).max(1);
        let text_y = content_top + ((content_height - text_size.cy).max(0) / 2);
        TextOutW(
            hdc,
            left,
            text_y,
            text_wide.as_ptr(),
            text_wide.len() as i32,
        );
    }
    left
}

fn measure_text_size(hdc: HDC, text: &str) -> SIZE {
    if text.is_empty() {
        return SIZE { cx: 0, cy: 0 };
    }
    let wide = to_wide_no_nul(text);
    let mut size: SIZE = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size) };
    if ok == 0 {
        SIZE { cx: 0, cy: 0 }
    } else {
        size
    }
}

pub(crate) fn help_hint_text(state: &OverlayShellState) -> String {
    if state.help_config_path.trim().is_empty() {
        HOTKEY_HELP_TEXT_FALLBACK.to_string()
    } else {
        "Click to edit hotkey".to_string()
    }
}

pub(crate) fn invalidate_list_row(list_hwnd: HWND, row: i32) {
    if row < 0 {
        return;
    }
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        let ok = SendMessageW(
            list_hwnd,
            LB_GETITEMRECT,
            row as usize,
            (&mut rect as *mut RECT) as LPARAM,
        );
        if ok != 0 {
            InvalidateRect(list_hwnd, &rect, 0);
        }
    }
}
