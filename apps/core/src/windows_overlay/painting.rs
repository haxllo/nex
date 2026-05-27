use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint,
    GetDC, GetTextExtentPoint32W, InvalidateRect, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, TextOutW,
    DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, HDC, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursorPos, GetWindowRect, GetWindowTextLengthW, HideCaret, KillTimer,
    SendMessageW, SetTimer, LB_GETCOUNT, LB_GETITEMRECT, LB_GETTOPINDEX, LB_SETTOPINDEX,
    WM_SETREDRAW,
};

use std::time::Instant;

use crate::windows_overlay::animation::blend_color;
use crate::windows_overlay::gdiplus_rendering::{GdiplusContext, GpRectF, SMOOTHING_MODE_ANTI_ALIAS};
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
    let Some(state) = state_for(hwnd) else { return; };
    let Some(ref gdiplus) = state.gdiplus else { return; };

    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };

    let w = (client.right - client.left).max(0);
    let h = (client.bottom - client.top).max(0);
    if w <= 0 || h <= 0 { return; }

    let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
    if hdc.is_null() { return; }

    let Some(graphics) = gdiplus.create_graphics(hdc as isize) else {
        unsafe { EndPaint(hwnd, &paint); }
        return;
    };

    let panel_bg = GdiplusContext::gdi_color_to_argb(state.palette.panel_bg);
    let panel_border = GdiplusContext::gdi_color_to_argb(state.palette.panel_border);

    // Fill entire background
    gdiplus.fill_rect(graphics, 0, 0, w, h, panel_bg);

    if !state.dwm_rounded_enabled && PANEL_RADIUS > 0 {
        // Draw border (outer rounded rect filled with border color)
        gdiplus.fill_rounded_rect_on_graphics(
            graphics, 0, 0, w, h, PANEL_RADIUS, panel_border,
        );
        // Draw inner rect (slightly smaller, filled with bg color)
        gdiplus.fill_rounded_rect_on_graphics(
            graphics, 2, 2, w - 4, h - 4,
            (PANEL_RADIUS - 2).max(1), panel_bg,
        );
    }

    if state.results_visible {
        let y = COMPACT_HEIGHT + DIVIDER_TOP_SPACING;
        GdiplusContext::draw_line(graphics, 2, y, w - 2, y, panel_border, 1.0);
    }

    GdiplusContext::delete_graphics(graphics);
    unsafe { EndPaint(hwnd, &paint); }
}

pub(crate) fn draw_list_row(hwnd: HWND, dis: &mut DRAWITEMSTRUCT) {
    if dis.itemID == u32::MAX { return; }
    let Some(state) = state_for(hwnd) else { return; };
    let Some(ref gdiplus) = state.gdiplus else { return; };

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
        && if state.hover_index >= 0 { hovered } else { selected_flag };
    let has_meta = !row.path.trim().is_empty();
    let icon_container_size = state.icon_container_size;

    let Some(graphics) = gdiplus.create_graphics(dis.hDC as isize) else { return; };
    GdiplusContext::set_smoothing_mode(graphics, SMOOTHING_MODE_ANTI_ALIAS);

    // Fill row background
    let bg_argb = GdiplusContext::gdi_color_to_argb(palette.results_bg);
    gdiplus.fill_rect(
        graphics,
        dis.rcItem.left, dis.rcItem.top,
        dis.rcItem.right - dis.rcItem.left,
        dis.rcItem.bottom - dis.rcItem.top,
        bg_argb,
    );

    // --- Section row ---
    if section_row {
        let section_title = row.title.trim();
        let section_title = if section_title.is_empty() { "Section" } else { section_title };
        let sect_wide = to_wide_no_nul(section_title);
        let sect_left = dis.rcItem.left + ROW_INSET_X;
        let sect_top = dis.rcItem.top + ((ROW_HEIGHT - HEADER_ROW_LABEL_HEIGHT).max(0) / 2);
        let sect_right = dis.rcItem.right - ROW_INSET_X;

        // Draw section title text
        if state.gdiplus_header_font != 0 {
            let label_h = HEADER_ROW_LABEL_HEIGHT as f32;
            let text_rect = GpRectF {
                x: sect_left as f32, y: sect_top as f32,
                width: (sect_right - sect_left) as f32, height: label_h,
            };
            gdiplus.draw_string(
                graphics, &sect_wide, state.gdiplus_header_font, &text_rect,
                GdiplusContext::gdi_color_to_argb(palette.text_section),
            );

            // Measure text for separator line position
            let measured = gdiplus.measure_string(graphics, &sect_wide, state.gdiplus_header_font, &text_rect);
            if let Some(bounds) = measured {
                let sect_width = bounds.width as i32;
                if sect_width > 0 {
                    let line_left = (sect_left + sect_width + HEADER_ROW_LINE_GAP).min(sect_right);
                    if line_left < sect_right {
                        let line_y = sect_top + HEADER_ROW_LABEL_HEIGHT / 2;
                        let line_color = GdiplusContext::gdi_color_to_argb(palette.text_section);
                        GdiplusContext::draw_line(
                            graphics, line_left, line_y, sect_right, line_y, line_color, 1.0,
                        );
                    }
                }
            }
        }
        GdiplusContext::delete_graphics(graphics);
        return;
    }

    // --- Selection highlight ---
    if !status_row && (selected_visible || hovered) {
        let sel_x = dis.rcItem.left + 3;
        let sel_y = dis.rcItem.top + ROW_VERTICAL_INSET + 1 + offset_y;
        let sel_w = dis.rcItem.right - 3 - sel_x;
        let sel_h = dis.rcItem.bottom - ROW_VERTICAL_INSET - 1 + offset_y - sel_y;

        if sel_w > 0 && sel_h > 0 {
            let is_dark = (palette.results_bg & 0xFF) < 128;
            let tint = if is_dark { 0xFFFFFF } else { 0x000000 };
            let highlight_color = blend_color(palette.results_bg, tint, 0.14);
            let gp_color = GdiplusContext::gdi_color_to_argb(highlight_color);
            gdiplus.fill_rounded_rect_on_graphics(graphics, sel_x, sel_y, sel_w, sel_h, 4, gp_color);
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
            GdiplusContext::draw_icon(
                graphics,
                icon_handle as isize,
                dis.rcItem.left + ROW_INSET_X + icon_offset,
                icon_top + icon_offset,
                icon_draw_size,
            );
        }
    }

    // --- Text ---
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
    let text_w = (text_right - text_left).max(0) as f32;

    if status_row {
        let text = to_wide_no_nul(&row.title);
        if state.gdiplus_status_font != 0 {
            let text_rect = GpRectF {
                x: text_left as f32, y: text_top as f32,
                width: text_w, height: ROW_TITLE_BLOCK_HEIGHT as f32,
            };
            gdiplus.draw_string(
                graphics, &text, state.gdiplus_status_font, &text_rect,
                GdiplusContext::gdi_color_to_argb(palette.text_secondary),
            );
        }
    } else {
        // Title
        let title_text = to_wide_no_nul(&row.title);
        if state.gdiplus_title_font != 0 {
            let text_rect = GpRectF {
                x: text_left as f32, y: text_top as f32,
                width: text_w, height: ROW_TITLE_BLOCK_HEIGHT as f32,
            };
            gdiplus.draw_string(
                graphics, &title_text, state.gdiplus_title_font, &text_rect,
                GdiplusContext::gdi_color_to_argb(palette.text_primary),
            );
        }

        // Path meta
        if has_meta && state.gdiplus_meta_font != 0 {
            let path_text = to_wide_no_nul(&row.path);
            let path_rect = GpRectF {
                x: text_left as f32,
                y: (text_top + ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP) as f32,
                width: text_w,
                height: ROW_META_BLOCK_HEIGHT as f32,
            };
            gdiplus.draw_string(
                graphics, &path_text, state.gdiplus_meta_font, &path_rect,
                GdiplusContext::gdi_color_to_argb(palette.text_secondary),
            );
        }
    }

    GdiplusContext::delete_graphics(graphics);
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
    let Some(ref gdiplus) = state.gdiplus else { return; };
    if state.gdiplus_help_tip_font == 0 {
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

        let Some(graphics) = gdiplus.create_graphics(hdc as isize) else {
            EndPaint(hwnd, &paint);
            return;
        };
        GdiplusContext::set_smoothing_mode(graphics, SMOOTHING_MODE_ANTI_ALIAS);

        let bg_color = GdiplusContext::gdi_color_to_argb(state.palette.help_tip_bg);
        let border_color = GdiplusContext::gdi_color_to_argb(state.palette.panel_border);
        let text_color = GdiplusContext::gdi_color_to_argb(state.palette.help_tip_text);

        gdiplus.fill_rounded_rect_on_graphics(graphics, 0, 0, width, height, HELP_TIP_RADIUS, bg_color);
        let border_rect = GpRectF { x: 0.5, y: 0.5, width: (width - 1) as f32, height: (height - 1) as f32 };
        gdiplus.draw_rounded_rect_border_on_graphics_f(graphics, &border_rect, HELP_TIP_RADIUS, border_color, 1.0);

        let text = help_hint_text(state);
        let text_w = (width - HELP_TIP_TEXT_PAD_X * 2).max(0);
        let layout_rect = GpRectF {
            x: HELP_TIP_TEXT_PAD_X as f32,
            y: 0.0,
            width: text_w as f32,
            height: height as f32,
        };
        let wide = to_wide(&text);
        gdiplus.draw_string(graphics, &wide, state.gdiplus_help_tip_font, &layout_rect, text_color);

        GdiplusContext::delete_graphics(graphics);
        EndPaint(hwnd, &paint);
    }
}

pub(crate) fn paint_footer_hint(hwnd: HWND, state: &mut OverlayShellState) {
    let Some(ref gdiplus) = state.gdiplus else { return; };
    if state.gdiplus_footer_font == 0 && state.gdiplus_hint_font == 0 {
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

        let Some(graphics) = gdiplus.create_graphics(hdc as isize) else {
            EndPaint(hwnd, &paint);
            return;
        };

        let panel_bg = GdiplusContext::gdi_color_to_argb(state.palette.panel_bg);
        let panel_border = GdiplusContext::gdi_color_to_argb(state.palette.panel_border);
        gdiplus.fill_rect(graphics, 0, 0, width, height, panel_bg);
        gdiplus.fill_rect(graphics, 0, 0, width, FOOTER_SEPARATOR_HEIGHT, panel_border);

        // Keep font selected in DC for measurement functions
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

        let content_top = (FOOTER_SEPARATOR_HEIGHT + FOOTER_SEPARATOR_TO_CONTENT_GAP).min(height);
        let content_bottom = (height - FOOTER_CONTENT_PAD_Y).max(content_top + 1);
        draw_footer_hints_centered(hdc, state, width, content_top, content_bottom);

        if !old_hint_font.is_null() {
            SelectObject(hdc, old_hint_font);
        }
        GdiplusContext::delete_graphics(graphics);
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
            hdc, state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_CLOSE,
            &[FOOTER_KEY_ESC],
        );
        right_cursor -= FOOTER_HINT_GROUP_GAP;
        right_cursor = draw_footer_hint_group_right(
            hdc, state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_MOVE,
            &[FOOTER_KEY_UP, FOOTER_KEY_DOWN],
        );
        right_cursor -= FOOTER_HINT_GROUP_GAP;
        let _ = draw_footer_hint_group_right(
            hdc, state,
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
            hdc, state,
            right_cursor,
            content_top,
            content_bottom,
            FOOTER_HINT_LABEL_CLOSE,
            &[FOOTER_KEY_ESC],
        );
        right_cursor -= FOOTER_HINT_GROUP_GAP;
        let _ = draw_footer_hint_group_right(
            hdc, state,
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
            hdc, state,
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
        hdc, cursor,
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

    let key_font = if state.hint_font != 0 {
        state.hint_font
    } else {
        state.footer_font
    };
    if key_font == 0 {
        return left;
    }

    unsafe {
        let old_font = SelectObject(hdc, key_font as _);
        let text_color = blend_color(state.palette.results_bg, state.palette.text_primary, 0.94);
        SetTextColor(hdc, text_color);
        SetBkMode(hdc, TRANSPARENT as i32);
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
        SelectObject(hdc, old_font);
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
        SetBkMode(hdc, TRANSPARENT as i32);
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
