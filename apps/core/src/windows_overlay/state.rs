use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Instant;

use crate::windows_overlay::gdiplus_rendering::GdiplusContext;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{CreatePen, CreateSolidBrush, PS_SOLID};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW;
use windows_sys::Win32::UI::WindowsAndMessaging::GWLP_USERDATA;

// ==================== ASYNC ICON LOADER TYPES ====================

/// A request to load a shell icon on the background thread.
/// NOTE: `hwnd` is stored as `isize` because `HWND` (`*mut c_void`) is not `Send`.
pub(crate) struct IconLoadRequest {
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) icon_path: String,
    pub(crate) hwnd: isize,
}

/// A completed icon load delivered back to the UI thread.
pub(crate) struct IconLoadResult {
    pub(crate) key: String,
    pub(crate) handle: isize,
}

// ==================== GDI OBJECT CACHE ====================

/// Holds cached GDI brushes and pens indexed by their BGR color value.
/// Created on demand during painting and bulk-destroyed during cleanup.
pub(crate) struct GdiObjectCache {
    pub(crate) brushes: HashMap<u32, isize>,
    pub(crate) pens: HashMap<u32, isize>,
}

impl GdiObjectCache {
    pub(crate) fn new() -> Self {
        Self {
            brushes: HashMap::new(),
            pens: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn brush(&mut self, color: u32) -> isize {
        let entry = self
            .brushes
            .entry(color)
            .or_insert_with(|| unsafe { CreateSolidBrush(color) as isize });
        *entry
    }

    #[allow(dead_code)]
    pub(crate) fn pen(&mut self, color: u32) -> isize {
        let entry = self
            .pens
            .entry(color)
            .or_insert_with(|| unsafe { CreatePen(PS_SOLID, 1, color) as isize });
        *entry
    }

    pub(crate) fn clear(&mut self) {
        for (_, h) in self.brushes.drain() {
            if h != 0 {
                unsafe {
                    windows_sys::Win32::Graphics::Gdi::DeleteObject(h as _);
                }
            }
        }
        for (_, h) in self.pens.drain() {
            if h != 0 {
                unsafe {
                    windows_sys::Win32::Graphics::Gdi::DeleteObject(h as _);
                }
            }
        }
    }
}

use crate::windows_overlay::types::{
    DibSurface, OverlayPalette, OverlayRow, OverlayTheme, MODE_STRIP_DEFAULT_TEXT, PALETTE_DARK,
};

// ==================== WINDOW ANIMATION ====================

pub(crate) struct WindowAnimation {
    pub(crate) start: Instant,
    pub(crate) duration_ms: u32,
    pub(crate) from_left: i32,
    pub(crate) from_top: i32,
    pub(crate) from_width: i32,
    pub(crate) from_height: i32,
    pub(crate) to_left: i32,
    pub(crate) to_top: i32,
    pub(crate) to_width: i32,
    pub(crate) to_height: i32,
    pub(crate) from_alpha: u8,
    pub(crate) to_alpha: u8,
    pub(crate) hide_on_complete: bool,
}

// ==================== ICON CACHE METRICS ====================

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IconCacheMetrics {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) load_failures: u64,
    pub(crate) evictions: u64,
}

// ==================== OVERLAY SHELL STATE ====================

pub(crate) fn state_for(hwnd: HWND) -> Option<&'static mut OverlayShellState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayShellState };
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

pub(crate) struct OverlayShellState {
    pub(crate) edit_hwnd: HWND,
    pub(crate) list_hwnd: HWND,
    pub(crate) status_hwnd: HWND,
    pub(crate) help_hwnd: HWND,
    pub(crate) help_tip_hwnd: HWND,
    pub(crate) footer_hint_hwnd: HWND,
    pub(crate) mode_strip_hwnd: HWND,
    pub(crate) everything_hwnd: HWND,
    pub(crate) overlay_hwnd: HWND,

    pub(crate) edit_prev_proc: isize,
    pub(crate) list_prev_proc: isize,
    pub(crate) help_prev_proc: isize,
    pub(crate) help_tip_prev_proc: isize,
    pub(crate) footer_hint_prev_proc: isize,

    pub(crate) input_font: isize,
    pub(crate) title_font: isize,
    pub(crate) meta_font: isize,
    pub(crate) status_font: isize,
    pub(crate) header_font: isize,
    pub(crate) top_hit_font: isize,
    pub(crate) hint_font: isize,
    pub(crate) help_tip_font: isize,
    pub(crate) help_icon_font: isize,
    pub(crate) search_icon_font: isize,
    pub(crate) footer_font: isize,
    pub(crate) command_prefix_font: isize,
    pub(crate) command_badge_font: isize,
    pub(crate) command_icon_font: isize,
    pub(crate) command_icon_fallback_font: isize,

    // GDI+ font handles (pre-created from GDI fonts)
    pub(crate) gdiplus_title_font: isize,
    pub(crate) gdiplus_meta_font: isize,
    pub(crate) gdiplus_status_font: isize,
    pub(crate) gdiplus_header_font: isize,
    pub(crate) gdiplus_help_tip_font: isize,
    pub(crate) gdiplus_footer_font: isize,
    pub(crate) gdiplus_hint_font: isize,

    pub(crate) panel_brush: isize,
    pub(crate) border_brush: isize,
    pub(crate) input_brush: isize,
    pub(crate) results_brush: isize,
    pub(crate) selection_brush: isize,
    pub(crate) selection_border_brush: isize,
    pub(crate) row_hover_brush: isize,
    pub(crate) row_separator_brush: isize,
    pub(crate) selection_accent_brush: isize,
    pub(crate) icon_brush: isize,

    pub(crate) theme: OverlayTheme,
    pub(crate) palette: OverlayPalette,

    pub(crate) status_is_error: bool,
    pub(crate) no_results_mode: bool,
    pub(crate) no_results_anim_pending: bool,
    pub(crate) status_center_aligned: bool,
    pub(crate) help_hovered: bool,
    pub(crate) help_tip_visible: bool,
    pub(crate) results_visible: bool,
    pub(crate) dwm_rounded_enabled: bool,
    pub(crate) mica_enabled: bool,
    pub(crate) help_config_path: String,
    pub(crate) active_query: String,
    pub(crate) command_mode_input: bool,
    pub(crate) command_uninstall_quick_mode: bool,
    pub(crate) command_badge_anim_start: Option<Instant>,
    pub(crate) expanded_rows: i32,
    pub(crate) placeholder_hint: String,
    pub(crate) mode_strip_text: String,

    pub(crate) hover_index: i32,
    pub(crate) wheel_delta_remainder: i32,
    pub(crate) pending_wheel_delta: i32,
    pub(crate) suppress_next_hover_sync: bool,
    pub(crate) results_content_anim_start: Option<Instant>,

    pub(crate) window_anim: Option<WindowAnimation>,
    pub(crate) loading: bool,
    pub(crate) loading_frame: u32,
    pub(crate) loading_tick_skip: u32,
    pub(crate) rows: Vec<OverlayRow>,
    pub(crate) icon_cache: HashMap<String, isize>,
    pub(crate) icon_cache_lru: VecDeque<String>,
    pub(crate) icon_cache_metrics: IconCacheMetrics,
    pub(crate) game_mode_enabled: bool,
    pub(crate) hotkey_issue_active: bool,
    pub(crate) everything_active: bool,
    pub(crate) tray_icon_added: bool,
    pub(crate) tray_icon_handle: isize,
    pub(crate) gdi_cache: GdiObjectCache,

    // DPI-aware sizing
    pub(crate) dpi: u32,
    pub(crate) icon_draw_size: i32,
    pub(crate) icon_container_size: i32,

    // Async icon loader state
    pub(crate) icon_load_sender: Option<mpsc::Sender<IconLoadRequest>>,
    pub(crate) icon_load_receiver: Option<mpsc::Receiver<IconLoadResult>>,
    pub(crate) icon_load_thread: Option<JoinHandle<()>>,
    pub(crate) pending_icon_loads: HashSet<String>,

    // GDI+ for antialiased selection highlight
    pub(crate) gdiplus: Option<GdiplusContext>,

    // 32-bit DIB for per-pixel alpha rendering (Mica backdrop)
    #[allow(dead_code)]
    pub(crate) dib: Option<DibSurface>,

    /// Current per-window alpha (0-255) used with UpdateLayeredWindow.
    #[allow(dead_code)]
    pub(crate) window_alpha: u8,
}

impl Default for OverlayShellState {
    fn default() -> Self {
        Self {
            edit_hwnd: std::ptr::null_mut(),
            list_hwnd: std::ptr::null_mut(),
            status_hwnd: std::ptr::null_mut(),
            help_hwnd: std::ptr::null_mut(),
            help_tip_hwnd: std::ptr::null_mut(),
            footer_hint_hwnd: std::ptr::null_mut(),
            mode_strip_hwnd: std::ptr::null_mut(),
            everything_hwnd: std::ptr::null_mut(),
            overlay_hwnd: std::ptr::null_mut(),
            edit_prev_proc: 0,
            list_prev_proc: 0,
            help_prev_proc: 0,
            help_tip_prev_proc: 0,
            footer_hint_prev_proc: 0,
            input_font: 0,
            title_font: 0,
            meta_font: 0,
            status_font: 0,
            header_font: 0,
            top_hit_font: 0,
            hint_font: 0,
            help_tip_font: 0,
            help_icon_font: 0,
            search_icon_font: 0,
            footer_font: 0,
            command_prefix_font: 0,
            command_badge_font: 0,
            command_icon_font: 0,
            command_icon_fallback_font: 0,
            gdiplus_title_font: 0,
            gdiplus_meta_font: 0,
            gdiplus_status_font: 0,
            gdiplus_header_font: 0,
            gdiplus_help_tip_font: 0,
            gdiplus_footer_font: 0,
            gdiplus_hint_font: 0,
            panel_brush: 0,
            border_brush: 0,
            input_brush: 0,
            results_brush: 0,
            selection_brush: 0,
            selection_border_brush: 0,
            row_hover_brush: 0,
            row_separator_brush: 0,
            selection_accent_brush: 0,
            icon_brush: 0,
            theme: OverlayTheme::Dark,
            palette: PALETTE_DARK,
            status_is_error: false,
            no_results_mode: false,
            no_results_anim_pending: false,
            status_center_aligned: false,
            help_hovered: false,
            help_tip_visible: false,
            results_visible: false,
            dwm_rounded_enabled: false,
            mica_enabled: false,
            help_config_path: String::new(),
            active_query: String::new(),
            command_mode_input: false,
            command_uninstall_quick_mode: false,
            command_badge_anim_start: None,
            expanded_rows: 0,
            placeholder_hint: String::new(),
            mode_strip_text: MODE_STRIP_DEFAULT_TEXT.to_string(),
            hover_index: -1,
            wheel_delta_remainder: 0,
            pending_wheel_delta: 0,
            suppress_next_hover_sync: false,
            results_content_anim_start: None,
            window_anim: None,
            loading: false,
            loading_frame: 0,
            loading_tick_skip: 0,
            rows: Vec::new(),
            icon_cache: HashMap::new(),
            icon_cache_lru: VecDeque::new(),
            icon_cache_metrics: IconCacheMetrics::default(),
            game_mode_enabled: false,
            hotkey_issue_active: false,
            everything_active: false,
            tray_icon_added: false,
            tray_icon_handle: 0,
            gdi_cache: GdiObjectCache::new(),
            icon_load_sender: None,
            icon_load_receiver: None,
            icon_load_thread: None,
            pending_icon_loads: HashSet::new(),
            dpi: 96,
            icon_draw_size: 32,
            icon_container_size: 34,
            gdiplus: None,
            dib: None,
            window_alpha: 255,
        }
    }
}
