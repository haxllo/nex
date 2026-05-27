use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

// ==================== CONSTANTS ====================

pub(crate) const CLASS_NAME: &str = "NexOverlayWindowClass";
pub(crate) const WINDOW_TITLE: &str = "Nex Launcher";
pub(crate) const INPUT_CLASS: &str = "EDIT";
pub(crate) const LIST_CLASS: &str = "LISTBOX";
pub(crate) const STATUS_CLASS: &str = "STATIC";

// Overlay layout tokens.
pub(crate) const WINDOW_WIDTH: i32 = 576;
pub(crate) const COMPACT_HEIGHT: i32 = 62;
pub(crate) const PANEL_RADIUS: i32 = COMPACT_HEIGHT + 10;
pub(crate) const WINDOW_OFFSET_Y: i32 = 0;
pub(crate) const PANEL_MARGIN_X: i32 = 14;
pub(crate) const PANEL_MARGIN_BOTTOM: i32 = 8;
pub(crate) const INPUT_HEIGHT: i32 = 36;
pub(crate) const INPUT_TOP: i32 = (COMPACT_HEIGHT - INPUT_HEIGHT) / 2;
pub(crate) const DIVIDER_TOP_SPACING: i32 = 0;
pub(crate) const DIVIDER_HEIGHT: i32 = 1;
pub(crate) const DIVIDER_BOTTOM_SPACING: i32 = 5;
pub(crate) const INPUT_TO_LIST_GAP: i32 =
    DIVIDER_TOP_SPACING + DIVIDER_HEIGHT + DIVIDER_BOTTOM_SPACING;
pub(crate) const MODE_STRIP_HEIGHT: i32 = 16;
pub(crate) const STATUS_HEIGHT: i32 = 18;
pub(crate) const NO_RESULTS_INLINE_WIDTH: i32 = 96;
pub(crate) const ROW_HEIGHT: i32 = 58;
pub(crate) const LIST_RADIUS: i32 = 16;
pub(crate) const MAX_VISIBLE_ROWS: usize = 8;
pub(crate) const ROW_INSET_X: i32 = 10;
// Base icon sizes at 96 DPI; scaled at runtime by state.dpi.
// These are kept as defaults for tests and before DPI query.
pub(crate) const ROW_ICON_SIZE: i32 = 34;
pub(crate) const ROW_ICON_DRAW_SIZE: i32 = 32;
pub(crate) const ROW_ICON_GAP: i32 = 10;
pub(crate) const ROW_VERTICAL_INSET: i32 = 2;
pub(crate) const ROW_ACTIVE_RADIUS: i32 = 8;
pub(crate) const ROW_TITLE_BLOCK_HEIGHT: i32 = 21;
pub(crate) const ROW_META_BLOCK_HEIGHT: i32 = 16;
pub(crate) const ROW_TEXT_LINE_GAP: i32 = 3;
pub(crate) const HEADER_ROW_LABEL_HEIGHT: i32 = 14;
pub(crate) const HEADER_ROW_LINE_GAP: i32 = 10;
pub(crate) const HEADER_ROW_LINE_HEIGHT: i32 = 1;
pub(crate) const FOOTER_HINT_HEIGHT: i32 = 26;
pub(crate) const FOOTER_SEPARATOR_HEIGHT: i32 = 1;
pub(crate) const FOOTER_CONTENT_PAD_Y: i32 = 4;
pub(crate) const FOOTER_SEPARATOR_TO_CONTENT_GAP: i32 = 10;
pub(crate) const FOOTER_CONTENT_PAD_X: i32 = 14;
pub(crate) const FOOTER_HINT_LABEL_OPEN: &str = "Open";
pub(crate) const FOOTER_HINT_LABEL_MOVE: &str = "Move";
pub(crate) const FOOTER_HINT_LABEL_CLOSE: &str = "Close";
pub(crate) const FOOTER_KEY_ENTER: &str = "\u{21B5}";
pub(crate) const FOOTER_KEY_UP: &str = "\u{2191}";
pub(crate) const FOOTER_KEY_DOWN: &str = "\u{2193}";
pub(crate) const FOOTER_KEY_ESC: &str = "Esc";
pub(crate) const FOOTER_KEY_TEXT_SHIFT_Y: i32 = 1;
pub(crate) const FOOTER_KEYCAP_GAP: i32 = 6;
pub(crate) const FOOTER_HINT_GROUP_GAP: i32 = 10;
pub(crate) const FOOTER_HINT_LABEL_GAP: i32 = 6;

pub(crate) const CONTROL_ID_INPUT: usize = 1001;
pub(crate) const CONTROL_ID_LIST: usize = 1002;
pub(crate) const CONTROL_ID_STATUS: usize = 1003;
pub(crate) const CONTROL_ID_HELP: usize = 1004;
pub(crate) const CONTROL_ID_HELP_TIP: usize = 1005;
pub(crate) const CONTROL_ID_FOOTER_HINT: usize = 1006;
pub(crate) const CONTROL_ID_MODE_STRIP: usize = 1007;
pub(crate) const CONTROL_ID_EVERYTHING: usize = 1008;
pub(crate) const STATIC_NOTIFY_STYLE: u32 = 0x0100;
pub(crate) const STATIC_LEFT_STYLE: u32 = 0x00000000;
pub(crate) const STATIC_CENTER_STYLE: u32 = 0x00000001;
pub(crate) const STATIC_RIGHT_STYLE: u32 = 0x00000002;
pub(crate) const EVERYTHING_INDICATOR_TEXT: &str = "\u{26A1} Everything";
pub(crate) const EX_NOACTIVATE_STYLE: u32 = 0x08000000;

pub(crate) const NEX_WM_ESCAPE: u32 = WM_APP + 1;
pub(crate) const NEX_WM_QUERY_CHANGED: u32 = WM_APP + 2;
pub(crate) const NEX_WM_MOVE_UP: u32 = WM_APP + 3;
pub(crate) const NEX_WM_MOVE_DOWN: u32 = WM_APP + 4;
pub(crate) const NEX_WM_SUBMIT: u32 = WM_APP + 5;
pub(crate) const NEX_WM_EXTERNAL_SHOW: u32 = WM_APP + 16;
pub(crate) const NEX_WM_EXTERNAL_QUIT: u32 = WM_APP + 17;
pub(crate) const NEX_WM_TRAY_ICON: u32 = WM_APP + 18;
pub(crate) const NEX_WM_TRAY_TOGGLE_GAME_MODE: u32 = WM_APP + 19;
pub(crate) const NEX_WM_TRAY_CHECK_UPDATES: u32 = WM_APP + 20;
pub(crate) const NEX_WM_SEARCH_RESULTS_READY: u32 = WM_APP + 21;
pub(crate) const NEX_WM_ICON_LOADED: u32 = WM_APP + 22;
pub(crate) const EM_GETRECT: u32 = 0x00B2;
pub(crate) const EM_SETRECTNP: u32 = 0x00B4;
pub(crate) const TRAY_ICON_ID: u32 = 1;
pub(crate) const TRAY_MENU_SHOW: usize = 41001;
pub(crate) const TRAY_MENU_OPEN_CONFIG: usize = 41002;
pub(crate) const TRAY_MENU_CHECK_UPDATES: usize = 41003;
pub(crate) const TRAY_MENU_GAME_MODE: usize = 41004;
pub(crate) const TRAY_MENU_QUIT: usize = 41005;

pub(crate) const TIMER_WINDOW_ANIM: usize = 0xBEF1;
pub(crate) const TIMER_HELP_HOVER: usize = 0xBEF3;
pub(crate) const TIMER_ICON_CACHE_IDLE: usize = 0xBEF4;
pub(crate) const TIMER_RESULTS_CONTENT_FADE: usize = 0xBEF5;
pub(crate) const TIMER_COMMAND_BADGE_FADE: usize = 0xBEF6;

pub(crate) const OVERLAY_ANIM_MS: u32 = 150;
pub(crate) const OVERLAY_ALPHA_OPAQUE: u8 = 255;
pub(crate) const RESULTS_ANIM_MS: u32 = 110;
pub(crate) const RESULTS_CONTENT_FADE_MS: u32 = 120;
pub(crate) const ANIM_FRAME_MS: u64 = 8;
pub(crate) const WHEEL_LINES_PER_NOTCH: i32 = 3;
pub(crate) const MAX_PENDING_WHEEL_DELTA: i32 = 120 * 8;
pub(crate) const HELP_HOVER_POLL_MS: u32 = 33;
pub(crate) const DEFAULT_ICON_CACHE_IDLE_MS: u32 = 90_000;
pub(crate) const DEFAULT_ICON_CACHE_MAX_ENTRIES: usize = 96;
pub(crate) const NO_RESULTS_FADE_MS: u32 = 85;
pub(crate) static ICON_CACHE_IDLE_MS_RUNTIME: AtomicU32 =
    AtomicU32::new(DEFAULT_ICON_CACHE_IDLE_MS);
pub(crate) static ICON_CACHE_MAX_ENTRIES_RUNTIME: AtomicUsize =
    AtomicUsize::new(DEFAULT_ICON_CACHE_MAX_ENTRIES);

// Typography tokens.
pub(crate) const FONT_INPUT_HEIGHT: i32 = -19;
pub(crate) const FONT_TITLE_HEIGHT: i32 = -15;
pub(crate) const FONT_META_HEIGHT: i32 = -13;
pub(crate) const FONT_STATUS_HEIGHT: i32 = -11;
pub(crate) const FONT_HEADER_HEIGHT: i32 = -12;
pub(crate) const FONT_TOP_HIT_HEIGHT: i32 = -16;
pub(crate) const FONT_HINT_HEIGHT: i32 = -11;
pub(crate) const FONT_HELP_TIP_HEIGHT: i32 = -11;
pub(crate) const FONT_HELP_ICON_HEIGHT: i32 = -14;
pub(crate) const FONT_FOOTER_HEIGHT: i32 = -13;
pub(crate) const FONT_COMMAND_ICON_HEIGHT: i32 = -24;
pub(crate) const FONT_COMMAND_PREFIX_HEIGHT: i32 = -22;
pub(crate) const FONT_COMMAND_BADGE_HEIGHT: i32 = -24;
pub(crate) const FONT_WEIGHT_INPUT: i32 = 400;
pub(crate) const FONT_WEIGHT_TITLE: i32 = 500;
pub(crate) const FONT_WEIGHT_META: i32 = 500;
pub(crate) const FONT_WEIGHT_STATUS: i32 = 400;
pub(crate) const FONT_WEIGHT_HEADER: i32 = 500;
pub(crate) const FONT_WEIGHT_TOP_HIT: i32 = 600;
pub(crate) const FONT_WEIGHT_HINT: i32 = 400;
pub(crate) const FONT_WEIGHT_HELP_TIP: i32 = 400;
pub(crate) const FONT_WEIGHT_HELP_ICON: i32 = 400;
pub(crate) const FONT_WEIGHT_FOOTER: i32 = 500;
pub(crate) const FONT_WEIGHT_COMMAND_ICON: i32 = 400;
pub(crate) const FONT_WEIGHT_COMMAND_PREFIX: i32 = 800;
pub(crate) const FONT_WEIGHT_COMMAND_BADGE: i32 = 800;
pub(crate) const ICON_FONT_FAMILY_PRIMARY: &str = "Segoe Fluent Icons";
pub(crate) const ICON_FONT_FAMILY_FALLBACK: &str = "Segoe MDL2 Assets";
pub(crate) const COMMAND_PREFIX_FONT_FAMILY: &str = "Segoe Fluent Icons";
pub(crate) const INPUT_TEXT_SHIFT_X: i32 = 10;
pub(crate) const INPUT_TEXT_SHIFT_Y: i32 = 0;
pub(crate) const INPUT_TEXT_SEARCH_PAD: i32 = 26;
pub(crate) const INPUT_TEXT_LINE_HEIGHT_FALLBACK: i32 = 20;
pub(crate) const INPUT_TEXT_LEFT_INSET: i32 = 19;
pub(crate) const INPUT_TEXT_RIGHT_INSET: i32 = 10;
pub(crate) const SEARCH_ICON_TEXT: &str = "\u{E721}";
pub(crate) const SEARCH_ICON_LEFT: i32 = 12;
pub(crate) const COMMAND_PREFIX_TEXT: &str = "\u{E76C}";
pub(crate) const COMMAND_PREFIX_RESERVED_WIDTH: i32 = 34;
pub(crate) const COMMAND_PREFIX_GAP: i32 = 12;
pub(crate) const COMMAND_PREFIX_LEFT_SHIFT: i32 = 20;
pub(crate) const COMMAND_PREFIX_INPUT_PAD: i32 = 16;
pub(crate) const COMMAND_BADGE_INPUT_PAD: i32 = 20;
pub(crate) const COMMAND_PREFIX_OPACITY: f32 = 0.60;
pub(crate) const COMMAND_PREFIX_EMBOLDEN_OPACITY: f32 = 0.40;
pub(crate) const COMMAND_PREFIX_EMBOLDEN_OFFSET_PX: i32 = 1;
pub(crate) const COMMAND_BADGE_TEXT: &str = "U";
pub(crate) const COMMAND_BADGE_GAP_FROM_PREFIX: i32 = 1;
pub(crate) const COMMAND_BADGE_ANIM_MS: u32 = 110;
pub(crate) const COMMAND_BADGE_SLIDE_PX: i32 = 6;
pub(crate) const HELP_ICON_SIZE: i32 = 14;
pub(crate) const HELP_ICON_RIGHT_INSET: i32 = 12;
pub(crate) const HELP_ICON_GAP_FROM_INPUT: i32 = 8;
pub(crate) const HELP_TIP_WIDTH: i32 = 132;
pub(crate) const HELP_TIP_HEIGHT: i32 = 26;
pub(crate) const HELP_TIP_RADIUS: i32 = 10;
pub(crate) const HELP_TIP_TEXT_PAD_X: i32 = 8;
pub(crate) const PRIMARY_FONT_FAMILY: &str = "Inter";
pub(crate) const FALLBACK_FONT_CHAIN: &[&str] = &[
    "Segoe UI Variable Display",
    "Segoe UI Variable",
    "Segoe UI",
    "Inter",
    "SF Pro Display",
    "Cascadia Mono",
    "Consolas",
    "Courier New",
    "Lucida Console",
];
pub(crate) const HOTKEY_HELP_TEXT_FALLBACK: &str = "Click to change hotkey";
pub(crate) const HELP_ICON_TEXT: &str = "\u{E946}";
pub(crate) const NO_RESULTS_STATUS_TEXT: &str = "No results";
pub(crate) const INPUT_PLACEHOLDER_TEXT: &str = "Type to search";
pub(crate) const COMMAND_INPUT_PLACEHOLDER_TEXT: &str = "Search the web or run a command";
pub(crate) const FOOTER_HINT_TEXT: &str =
    "Enter Open  \u{2022}  \u{2191}\u{2193} Move  \u{2022}  Esc Close";
pub(crate) const MODE_STRIP_DEFAULT_TEXT: &str = "All   Apps   Files   Actions   Clipboard";

// ==================== ENUMS & STRUCTS ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayTheme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayPalette {
    pub(crate) panel_bg: u32,
    pub(crate) panel_border: u32,
    pub(crate) input_bg: u32,
    pub(crate) results_bg: u32,
    pub(crate) text_primary: u32,
    pub(crate) text_secondary: u32,
    pub(crate) text_error: u32,
    pub(crate) text_highlight: u32,
    pub(crate) text_hint: u32,
    pub(crate) text_section: u32,
    pub(crate) text_hint_footer: u32,
    pub(crate) text_mode_strip: u32,
    pub(crate) selection: u32,
    pub(crate) selection_border: u32,
    pub(crate) row_hover: u32,
    pub(crate) row_separator: u32,
    pub(crate) selection_accent: u32,
    pub(crate) icon_bg: u32,
    pub(crate) icon_text: u32,
    pub(crate) help_icon: u32,
    pub(crate) help_icon_hover: u32,
    pub(crate) help_tip_bg: u32,
    pub(crate) help_tip_text: u32,
}

pub(crate) const PALETTE_DARK: OverlayPalette = OverlayPalette {
    panel_bg: 0x00272727,
    panel_border: 0x00424242,
    input_bg: 0x00272727,
    results_bg: 0x00272727,
    text_primary: 0x00F5F5F5,
    text_secondary: 0x00C4C4C4,
    text_error: 0x00E8E8E8,
    text_highlight: 0x00FFFFFF,
    text_hint: 0x00BEBEBE,
    text_section: 0x009E9E9E,
    text_hint_footer: 0x009A9A9A,
    text_mode_strip: 0x00ABABAB,
    selection: 0x00262626,
    selection_border: 0x00383838,
    row_hover: 0x00313131,
    row_separator: 0x00161616,
    selection_accent: 0x00343434,
    icon_bg: 0x001D1D1D,
    icon_text: 0x00F0F0F0,
    help_icon: 0x00B5B5B5,
    help_icon_hover: 0x00F5F5F5,
    help_tip_bg: 0x00272727,
    help_tip_text: 0x00B5B5B5,
};

pub(crate) const PALETTE_LIGHT: OverlayPalette = OverlayPalette {
    panel_bg: 0x00F3F3F3,
    panel_border: 0x00C9C9C9,
    input_bg: 0x00F3F3F3,
    results_bg: 0x00F3F3F3,
    text_primary: 0x001A1A1A,
    text_secondary: 0x003F3F3F,
    text_error: 0x003E3E3E,
    text_highlight: 0x000D0D0D,
    text_hint: 0x00606060,
    text_section: 0x00606060,
    text_hint_footer: 0x00686868,
    text_mode_strip: 0x00626262,
    selection: 0x00E5E5E5,
    selection_border: 0x00D3D3D3,
    row_hover: 0x00ECECEC,
    row_separator: 0x00DCDCDC,
    selection_accent: 0x00D8D8D8,
    icon_bg: 0x00DFDFDF,
    icon_text: 0x00202020,
    help_icon: 0x00505050,
    help_icon_hover: 0x001A1A1A,
    help_tip_bg: 0x00F3F3F3,
    help_tip_text: 0x00505050,
};

pub(crate) fn palette_for_theme(theme: OverlayTheme) -> OverlayPalette {
    match theme {
        OverlayTheme::Dark => PALETTE_DARK,
        OverlayTheme::Light => PALETTE_LIGHT,
    }
}

pub(crate) fn detect_system_theme() -> OverlayTheme {
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
        OverlayTheme::Light
    } else {
        OverlayTheme::Dark
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayEvent {
    Hotkey(i32),
    QueryChanged(String),
    MoveSelection(i32),
    Submit,
    TrayToggleGameMode,
    TrayCheckForUpdates,
    Escape,
    ExternalShow,
    ExternalQuit,
    SearchResultsReady,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayRowRole {
    Item,
    Header,
    TopHit,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayRow {
    pub role: OverlayRowRole,
    pub result_index: i32,
    pub kind: String,
    pub title: String,
    pub path: String,
    pub icon_path: String,
}

pub struct NativeOverlayShell {
    pub(crate) hwnd: HWND,
}

// ==================== HELPERS ====================

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    wide.push(0);
    wide
}

pub(crate) fn to_wide_no_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_for_theme_dark_returns_dark_palette() {
        let p = palette_for_theme(OverlayTheme::Dark);
        assert_eq!(p.panel_bg, PALETTE_DARK.panel_bg);
        assert_eq!(p.text_primary, PALETTE_DARK.text_primary);
    }

    #[test]
    fn palette_for_theme_light_returns_light_palette() {
        let p = palette_for_theme(OverlayTheme::Light);
        assert_eq!(p.panel_bg, PALETTE_LIGHT.panel_bg);
        assert_eq!(p.text_primary, PALETTE_LIGHT.text_primary);
    }

    #[test]
    fn overlay_theme_debug_and_eq() {
        assert_eq!(format!("{:?}", OverlayTheme::Dark), "Dark");
        assert_eq!(format!("{:?}", OverlayTheme::Light), "Light");
        assert_ne!(OverlayTheme::Dark, OverlayTheme::Light);
    }

    #[test]
    fn overlay_palette_has_expected_fields() {
        let p = PALETTE_DARK;
        assert!(p.panel_border != 0);
        assert!(p.text_primary != 0);
        assert!(p.text_secondary != 0);
    }

    #[test]
    fn to_wide_appends_null_terminator() {
        let w = to_wide("abc");
        assert_eq!(w, vec![97, 98, 99, 0]);
    }

    #[test]
    fn to_wide_no_nul_does_not_append_terminator() {
        let w = to_wide_no_nul("abc");
        assert_eq!(w, vec![97, 98, 99]);
    }

    #[test]
    fn to_wide_empty_returns_only_null() {
        let w = to_wide("");
        assert_eq!(w, vec![0]);
    }

    #[test]
    fn to_wide_no_nul_empty_returns_empty() {
        let w = to_wide_no_nul("");
        assert_eq!(w, Vec::<u16>::new());
    }

    #[test]
    fn overlay_row_creation() {
        let row = OverlayRow {
            role: OverlayRowRole::Item,
            result_index: 0,
            kind: "file".into(),
            title: "test.txt".into(),
            path: "C:\\test.txt".into(),
            icon_path: "C:\\test.txt".into(),
        };
        assert_eq!(row.title, "test.txt");
        assert_eq!(row.role, OverlayRowRole::Item);
    }

    #[test]
    fn overlay_event_variants() {
        let e1 = OverlayEvent::Hotkey(162);
        let e2 = OverlayEvent::QueryChanged("hello".into());
        let e3 = OverlayEvent::Submit;
        assert!(format!("{:?}", e1).contains("Hotkey"));
        assert!(format!("{:?}", e2).contains("QueryChanged"));
        assert!(format!("{:?}", e3).contains("Submit"));
    }
}
