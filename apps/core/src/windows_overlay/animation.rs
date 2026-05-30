use std::time::Instant;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    KillTimer, SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, HWND_TOPMOST,
    LWA_ALPHA, SWP_NOACTIVATE, SW_HIDE,
};

use crate::windows_overlay::state::{OverlayShellState, WindowAnimation};
use crate::windows_overlay::types::{ANIM_FRAME_MS, RESULTS_CONTENT_FADE_MS, TIMER_WINDOW_ANIM};

pub(crate) fn apply_window_state(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, alpha: u8) {
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
        SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
    }
}

pub(crate) fn cancel_window_animation(hwnd: HWND) {
    if let Some(state) = state_for(hwnd) {
        state.window_anim = None;
        unsafe {
            KillTimer(hwnd, TIMER_WINDOW_ANIM);
        }
    }
}

pub(crate) fn hide_overlay_immediate(hwnd: HWND) {
    cancel_window_animation(hwnd);
    apply_window_state(hwnd, 0, 0, 0, 0, 0);
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
}

pub(crate) fn start_window_animation(
    hwnd: HWND,
    from_left: i32,
    from_top: i32,
    from_width: i32,
    from_height: i32,
    to_left: i32,
    to_top: i32,
    to_width: i32,
    to_height: i32,
    from_alpha: u8,
    to_alpha: u8,
    duration_ms: u32,
    hide_on_complete: bool,
) {
    if let Some(state) = state_for(hwnd) {
        state.window_anim = Some(WindowAnimation {
            start: Instant::now(),
            duration_ms,
            from_left,
            from_top,
            from_width,
            from_height,
            to_left,
            to_top,
            to_width,
            to_height,
            from_alpha,
            to_alpha,
            hide_on_complete,
        });
        unsafe {
            SetTimer(hwnd, TIMER_WINDOW_ANIM, ANIM_FRAME_MS as u32, None);
        }
    }
}

pub(crate) fn window_animation_tick(hwnd: HWND, state: &OverlayShellState) -> bool {
    let Some(ref anim) = state.window_anim else {
        return false;
    };

    let elapsed = Instant::now().duration_since(anim.start).as_millis() as u64;
    if elapsed >= anim.duration_ms as u64 {
        apply_window_state(
            hwnd,
            anim.to_left,
            anim.to_top,
            anim.to_width,
            anim.to_height,
            anim.to_alpha,
        );
        if anim.hide_on_complete {
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
        if let Some(state) = state_for(hwnd) {
            state.window_anim = None;
        }
        return false;
    }

    let t = elapsed as f64 / anim.duration_ms as f64;
    let eased = ease_out(t);
    let x = lerp_i32(anim.from_left, anim.to_left, eased);
    let y = lerp_i32(anim.from_top, anim.to_top, eased);
    let w = lerp_i32(anim.from_width, anim.to_width, eased);
    let h = lerp_i32(anim.from_height, anim.to_height, eased);
    let alpha = lerp_i32(anim.from_alpha as i32, anim.to_alpha as i32, eased) as u8;

    apply_window_state(hwnd, x, y, w, h, alpha);
    true
}

pub(crate) fn complete_window_animation_if_running(hwnd: HWND, state: &OverlayShellState) {
    let Some(ref anim) = state.window_anim else {
        return;
    };
    apply_window_state(
        hwnd,
        anim.to_left,
        anim.to_top,
        anim.to_width,
        anim.to_height,
        anim.to_alpha,
    );
    if anim.hide_on_complete {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

pub(crate) fn results_content_animation_tick(_hwnd: HWND, state: &OverlayShellState) -> bool {
    let Some(start) = state.results_content_anim_start else {
        return false;
    };
    let elapsed = Instant::now().duration_since(start).as_millis() as u64;
    if elapsed >= RESULTS_CONTENT_FADE_MS as u64 {
        return false;
    }
    unsafe {
        // Only invalidate the listbox — the panel background does not change
        // during content-fade animation.  Invalidating the parent overlay here
        // would trigger a D2D BeginDraw → EndDraw → DXGI Present that races
        // ahead of GDI child-window painting on WS_EX_LAYERED windows, causing
        // a visible flash frame.
        if !state.list_hwnd.is_null() {
            InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
        }
    }
    true
}

pub(crate) fn ease_out(t: f64) -> f64 {
    t * (2.0 - t)
}

pub(crate) fn lerp_i32(a: i32, b: i32, t: f64) -> i32 {
    (a as f64 + (b - a) as f64 * t).round() as i32
}

pub(crate) fn blend_color(bg: u32, fg: u32, opacity: f32) -> u32 {
    let bg_r = (bg >> 0) & 0xFF;
    let bg_g = (bg >> 8) & 0xFF;
    let bg_b = (bg >> 16) & 0xFF;
    let fg_r = (fg >> 0) & 0xFF;
    let fg_g = (fg >> 8) & 0xFF;
    let fg_b = (fg >> 16) & 0xFF;
    let alpha = opacity.clamp(0.0, 1.0);
    let r = (bg_r as f32 * (1.0 - alpha) + fg_r as f32 * alpha) as u32;
    let g = (bg_g as f32 * (1.0 - alpha) + fg_g as f32 * alpha) as u32;
    let b = (bg_b as f32 * (1.0 - alpha) + fg_b as f32 * alpha) as u32;
    (b << 16) | (g << 8) | r
}

// Forward declaration for state_for used above - state_for is defined in window.rs
// We need to use the one from the window module
use crate::windows_overlay::state::state_for;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_returns_expected_values() {
        assert!((ease_out(0.0) - 0.0).abs() < 1e-10);
        assert!((ease_out(0.5) - 0.75).abs() < 1e-10);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-10);
        assert!(ease_out(0.25) > 0.25);
    }

    #[test]
    fn lerp_i32_interpolates_correctly() {
        assert_eq!(lerp_i32(0, 100, 0.0), 0);
        assert_eq!(lerp_i32(0, 100, 0.5), 50);
        assert_eq!(lerp_i32(0, 100, 1.0), 100);
        assert_eq!(lerp_i32(100, 200, 0.3), 130);
    }

    #[test]
    fn lerp_i32_extrapolates_beyond_zero_and_one() {
        // lerp_i32 does not clamp t, it extrapolates linearly.
        assert_eq!(lerp_i32(10, 20, -0.5), 5);
        assert_eq!(lerp_i32(10, 20, 1.5), 25);
    }

    #[test]
    fn blend_color_combines_opaque_fg() {
        let bg = 0x00_00_00_u32; // black (BGR)
        let fg = 0xFF_FF_FF_u32; // white (BGR)
        assert_eq!(blend_color(bg, fg, 1.0), fg);
    }

    #[test]
    fn blend_color_returns_bg_when_opacity_zero() {
        let bg = 0x12_34_56_u32;
        let fg = 0xAB_CD_EF_u32;
        assert_eq!(blend_color(bg, fg, 0.0), bg);
    }

    #[test]
    fn blend_color_clamps_opacity() {
        let bg = 0x00_00_00_u32;
        let fg = 0xFF_FF_FF_u32;
        assert_eq!(blend_color(bg, fg, -0.1), bg);
        assert_eq!(blend_color(bg, fg, 1.5), fg);
    }

    #[test]
    fn blend_color_partial_opacity() {
        let bg = 0x00_00_00_u32;
        let fg = 0xFF_FF_FF_u32;
        let result = blend_color(bg, fg, 0.5);
        // f32 as u32 truncates: 127.5 -> 127 per channel = 0x7F7F7F in BGR
        assert_eq!(result, 0x7F_7F_7F_u32);
    }
}
