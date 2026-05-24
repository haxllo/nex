use std::collections::HashSet;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateRoundRectRgn, DeleteObject, DrawTextW, EndPaint, FillRect, FillRgn, FrameRgn,
    GetDC, GetTextExtentPoint32W, GetTextMetricsW, InvalidateRect, ReleaseDC, RoundRect,
    SelectObject, SetBkMode, SetTextColor, TextOutW, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, HDC, PAINTSTRUCT, TEXTMETRICW, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursorPos, GetWindowRect, GetWindowTextLengthW, HideCaret, KillTimer,
    SendMessageW, SetTimer, LB_GETCOUNT, LB_GETITEMRECT, LB_GETTOPINDEX, LB_SETTOPINDEX,
    WM_SETREDRAW,
};

use std::time::Instant;

use crate::windows_overlay::animation::blend_color;
use crate::windows_overlay::icon_cache::{draw_action_icon, draw_row_icon};
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
            DT_LEFT | DT_SINGLELINE | DT_EDITCONTROL | DT_VCENTER | DT_END_ELLIPSIS,
        );
        SelectObject(hdc, old_font);
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
        if width > 0 && height > 0 {
            if state.dwm_rounded_enabled {
                // In DWM mode, let DWM own the rounded border and only fill the panel.
                FillRect(hdc, &client, state.panel_brush as _);
            } else {
                // Paint the rounded border and inner fill separately to keep edges clean.
                let outer_region =
                    CreateRoundRectRgn(0, 0, width + 1, height + 1, PANEL_RADIUS, PANEL_RADIUS);
                FillRgn(hdc, outer_region, state.border_brush as _);

                if width > 2 && height > 2 {
                    let inner_radius = (PANEL_RADIUS - 2).max(2);
                    let inner_region =
                        CreateRoundRectRgn(1, 1, width, height, inner_radius, inner_radius);
                    FillRgn(hdc, inner_region, state.panel_brush as _);
                    DeleteObject(inner_region as _);
                } else {
                    FillRgn(hdc, outer_region, state.panel_brush as _);
                }

                DeleteObject(outer_region as _);
            }

            draw_input_results_divider(hdc, width, state);
        }
        EndPaint(hwnd, &paint);
    }
}

fn draw_input_results_divider(hdc: HDC, width: i32, state: &OverlayShellState) {
    if !state.results_visible || state.border_brush == 0 {
        return;
    }

    let left = 1;
    let right = (width - 1).max(left + 1);
    let y = COMPACT_HEIGHT + DIVIDER_TOP_SPACING;
    let divider_rect = RECT {
        left,
        top: y,
        right,
        bottom: y + DIVIDER_HEIGHT,
    };
    unsafe {
        FillRect(hdc, &divider_rect, state.border_brush as _);
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
    unsafe {
        FillRect(dis.hDC, &dis.rcItem, state.results_brush as _);
        if section_row {
            let section_title = row.title.trim();
            let section_title = if section_title.is_empty() {
                "Section"
            } else {
                section_title
            };
            let mut section_rect = RECT {
                left: dis.rcItem.left + ROW_INSET_X,
                top: dis.rcItem.top + ((ROW_HEIGHT - HEADER_ROW_LABEL_HEIGHT).max(0) / 2),
                right: dis.rcItem.right - ROW_INSET_X,
                bottom: dis.rcItem.top
                    + ((ROW_HEIGHT - HEADER_ROW_LABEL_HEIGHT).max(0) / 2)
                    + HEADER_ROW_LABEL_HEIGHT,
            };
            let old_font = SelectObject(dis.hDC, state.header_font as _);
            SetBkMode(dis.hDC, TRANSPARENT as i32);
            SetTextColor(
                dis.hDC,
                blend_color(palette.results_bg, palette.text_section, content_progress),
            );
            DrawTextW(
                dis.hDC,
                to_wide(section_title).as_ptr(),
                -1,
                &mut section_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            let section_text_width = measure_text_width(dis.hDC, section_title);
            if section_text_width > 0 {
                let line_left = (section_rect.left + section_text_width + HEADER_ROW_LINE_GAP)
                    .min(section_rect.right);
                if line_left < section_rect.right {
                    let line_top = section_rect.top + (HEADER_ROW_LABEL_HEIGHT / 2);
                    let line_rect = RECT {
                        left: line_left,
                        top: line_top,
                        right: section_rect.right,
                        bottom: line_top + HEADER_ROW_LINE_HEIGHT,
                    };
                    let line_color =
                        blend_color(palette.results_bg, palette.row_separator, content_progress);
                    let line_brush = state.gdi_cache.brush(line_color);
                    FillRect(dis.hDC, &line_rect, line_brush as _);
                }
            }
            SelectObject(dis.hDC, old_font);
            return;
        }

        if !status_row && (selected_visible || hovered) {
            let row_rect = RECT {
                left: dis.rcItem.left + 3,
                top: dis.rcItem.top + ROW_VERTICAL_INSET + 1 + offset_y,
                right: dis.rcItem.right - 3,
                bottom: dis.rcItem.bottom - ROW_VERTICAL_INSET - 1 + offset_y,
            };
            let hover_color = blend_color(palette.results_bg, palette.row_hover, content_progress);
            let fill_brush = state.gdi_cache.brush(hover_color);
            let fill_pen = state.gdi_cache.pen(hover_color);
            let old_brush = SelectObject(dis.hDC, fill_brush as _);
            let old_pen = SelectObject(dis.hDC, fill_pen as _);
            // RoundRect generally renders cleaner highlight corners than region fills on GDI list rows.
            RoundRect(
                dis.hDC,
                row_rect.left,
                row_rect.top,
                row_rect.right + 1,
                row_rect.bottom + 1,
                ROW_ACTIVE_RADIUS,
                ROW_ACTIVE_RADIUS,
            );
            SelectObject(dis.hDC, old_pen);
            SelectObject(dis.hDC, old_brush);
        }

        let old_font = SelectObject(dis.hDC, state.title_font as _);
        SetBkMode(dis.hDC, TRANSPARENT as i32);
        let primary_text = blend_color(palette.results_bg, palette.text_primary, content_progress);
        let secondary_text =
            blend_color(palette.results_bg, palette.text_secondary, content_progress);
        let highlight_text =
            blend_color(palette.results_bg, palette.text_highlight, content_progress);
        SetTextColor(dis.hDC, primary_text);

        let has_meta = !row.path.trim().is_empty();
        let text_right = dis.rcItem.right - ROW_INSET_X;
        let text_left = if status_row {
            dis.rcItem.left + ROW_INSET_X
        } else {
            let text_top = if has_meta {
                let total_height =
                    ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT;
                dis.rcItem.top + ((ROW_HEIGHT - total_height).max(0) / 2) + offset_y
            } else {
                dis.rcItem.top + ((ROW_HEIGHT - ROW_TITLE_BLOCK_HEIGHT).max(0) / 2) + offset_y
            };
            let content_height = if has_meta {
                ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT
            } else {
                ROW_TITLE_BLOCK_HEIGHT
            };
            let icon_top = text_top + (content_height - ROW_ICON_SIZE) / 2;
            let icon_rect = RECT {
                left: dis.rcItem.left + ROW_INSET_X,
                top: icon_top,
                right: dis.rcItem.left + ROW_INSET_X + ROW_ICON_SIZE,
                bottom: icon_top + ROW_ICON_SIZE,
            };
            let icon_drawn = draw_row_icon(dis.hDC, &icon_rect, &row, state);
            if !icon_drawn {
                FillRect(dis.hDC, &icon_rect, state.icon_brush as _);
                let icon_tint =
                    blend_color(palette.results_bg, palette.icon_text, content_progress);
                if !draw_action_icon(dis.hDC, &icon_rect, &row, state, icon_tint) {
                    let mut icon_text_rect = icon_rect;
                    SetTextColor(dis.hDC, icon_tint);
                    DrawTextW(
                        dis.hDC,
                        to_wide(icon_glyph_for_row(&row)).as_ptr(),
                        -1,
                        &mut icon_text_rect,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                    );
                }
            }
            SetTextColor(dis.hDC, primary_text);
            icon_rect.right + ROW_ICON_GAP
        };
        let text_top = if has_meta {
            let total_height = ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT;
            dis.rcItem.top + ((ROW_HEIGHT - total_height).max(0) / 2) + offset_y
        } else {
            dis.rcItem.top + ((ROW_HEIGHT - ROW_TITLE_BLOCK_HEIGHT).max(0) / 2) + offset_y
        };
        let mut title_rect = RECT {
            left: text_left,
            top: text_top,
            right: text_right,
            bottom: text_top + ROW_TITLE_BLOCK_HEIGHT,
        };
        if status_row {
            SetTextColor(dis.hDC, secondary_text);
            DrawTextW(
                dis.hDC,
                to_wide(&row.title).as_ptr(),
                -1,
                &mut title_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        } else {
            draw_highlighted_title(
                dis.hDC,
                &title_rect,
                &row.title,
                &state.active_query,
                primary_text,
                highlight_text,
            );
        }

        if has_meta && !status_row {
            SelectObject(dis.hDC, state.meta_font as _);
            SetTextColor(dis.hDC, secondary_text);
            let path_rect = RECT {
                left: text_left,
                top: title_rect.bottom + ROW_TEXT_LINE_GAP,
                right: text_right,
                bottom: title_rect.bottom + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT,
            };
            draw_plain_text(dis.hDC, &path_rect, &row.path, secondary_text);
        }

        SelectObject(dis.hDC, old_font);
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

fn draw_highlighted_title(
    hdc: HDC,
    rect: &RECT,
    title: &str,
    query: &str,
    base_color: u32,
    highlight_color: u32,
) {
    if rect.right <= rect.left || title.trim().is_empty() {
        return;
    }

    let max_width = rect.right - rect.left;
    if max_width <= 0 {
        return;
    }

    let display = fit_text_with_ellipsis(hdc, title, max_width);
    if display.is_empty() {
        return;
    }

    let highlighted = fuzzy_match_positions(&display, query);
    let text_height = current_text_height(hdc).max(1);
    let y = rect.top + ((rect.bottom - rect.top - text_height).max(0) / 2);
    let mut x = rect.left;

    for (index, ch) in display.chars().enumerate() {
        let s = ch.to_string();
        let width = measure_text_width(hdc, &s).max(1);
        if x + width > rect.right {
            break;
        }

        let wide = to_wide_no_nul(&s);
        unsafe {
            SetTextColor(
                hdc,
                if highlighted.contains(&index) {
                    highlight_color
                } else {
                    base_color
                },
            );
            TextOutW(hdc, x, y, wide.as_ptr(), wide.len() as i32);
        }
        x += width;
    }
}

fn draw_plain_text(hdc: HDC, rect: &RECT, text: &str, color: u32) {
    if rect.right <= rect.left {
        return;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let mut draw_rect = *rect;
    unsafe {
        SetTextColor(hdc, color);
        DrawTextW(
            hdc,
            to_wide(trimmed).as_ptr(),
            -1,
            &mut draw_rect,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
}

fn fit_text_with_ellipsis(hdc: HDC, text: &str, max_width: i32) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    if measure_text_width(hdc, text) <= max_width {
        return text.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = measure_text_width(hdc, ellipsis);
    if ellipsis_width >= max_width {
        return String::new();
    }

    let mut output = String::new();
    for ch in text.chars() {
        let mut candidate = output.clone();
        candidate.push(ch);
        if measure_text_width(hdc, &candidate) + ellipsis_width > max_width {
            break;
        }
        output.push(ch);
    }
    output.push_str(ellipsis);
    output
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

fn current_text_height(hdc: HDC) -> i32 {
    let mut tm: TEXTMETRICW = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetTextMetricsW(hdc, &mut tm) };
    if ok == 0 {
        14
    } else {
        tm.tmHeight as i32
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
