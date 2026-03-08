#[cfg(target_os = "windows")]
mod imp {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        OnceLock,
    };
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{
        GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
    };
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        AddFontResourceExW, BeginPaint, CreateFontW, CreatePen, CreateRoundRectRgn,
        CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FillRgn, FrameRgn, GetDC,
        GetTextExtentPoint32W, GetTextMetricsW, InvalidateRect, ReleaseDC, RoundRect, SelectObject,
        SetBkColor, SetBkMode, SetTextColor, SetWindowRgn, TextOutW, ANTIALIASED_QUALITY,
        CLEARTYPE_QUALITY, DEFAULT_CHARSET, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_LEFT,
        DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FR_PRIVATE, HDC, OPAQUE, OUT_DEFAULT_PRECIS,
        PAINTSTRUCT, PS_SOLID, TEXTMETRICW, TRANSPARENT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    use windows_sys::Win32::UI::Controls::{
        ImageList_GetIcon, DRAWITEMSTRUCT, EM_SETSEL, MEASUREITEMSTRUCT, ODS_SELECTED,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SetFocus, VK_BACK, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
    };
    use windows_sys::Win32::UI::Shell::{
        ExtractIconExW, FindExecutableW, HlinkResolveShortcutToString, SHGetFileInfoW,
        SHParseDisplayName, SHFILEINFOW, SHGFI_ICON, SHGFI_ICONLOCATION, SHGFI_LARGEICON,
        SHGFI_PIDL, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AnimateWindow, CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyIcon,
        DispatchMessageW, DrawIconEx, FindWindowW, GetClientRect, GetCursorPos,
        GetForegroundWindow, GetMessageW, GetParent, GetSystemMetrics, GetWindowLongPtrW,
        GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HideCaret, IsChild, KillTimer,
        LoadCursorW, MoveWindow, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
        SendMessageW, SetCursor, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer,
        SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, AW_ACTIVATE,
        AW_BLEND, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, DI_NORMAL, EN_CHANGE,
        ES_AUTOHSCROLL, ES_MULTILINE, GWLP_USERDATA, GWLP_WNDPROC, GWL_STYLE, HMENU, HWND_TOPMOST,
        IDC_ARROW, IDC_HAND, LBN_DBLCLK, LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY,
        LBS_OWNERDRAWFIXED, LB_ADDSTRING, LB_GETCOUNT, LB_GETCURSEL, LB_GETITEMRECT,
        LB_GETTOPINDEX, LB_ITEMFROMPOINT, LB_RESETCONTENT, LB_SETCURSEL, LB_SETTOPINDEX, LWA_ALPHA,
        MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WM_ACTIVATE,
        WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX,
        WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONUP,
        WM_MEASUREITEM, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
        WM_SETFOCUS, WM_SETFONT, WM_SETREDRAW, WM_SIZE, WM_TIMER, WNDCLASSW, WS_CHILD,
        WS_CLIPCHILDREN, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    const CLASS_NAME: &str = "SwiftFindOverlayWindowClass";
    const WINDOW_TITLE: &str = "SwiftFind Launcher";
    const INPUT_CLASS: &str = "EDIT";
    const LIST_CLASS: &str = "LISTBOX";
    const STATUS_CLASS: &str = "STATIC";

    // Overlay layout tokens.
    const WINDOW_WIDTH: i32 = 576;
    const COMPACT_HEIGHT: i32 = 62;
    const PANEL_RADIUS: i32 = COMPACT_HEIGHT + 10;
    const WINDOW_OFFSET_Y: i32 = 0;
    const PANEL_MARGIN_X: i32 = 14;
    const PANEL_MARGIN_BOTTOM: i32 = 8;
    const INPUT_HEIGHT: i32 = 36;
    const INPUT_TOP: i32 = (COMPACT_HEIGHT - INPUT_HEIGHT) / 2;
    const DIVIDER_TOP_SPACING: i32 = 0;
    const DIVIDER_HEIGHT: i32 = 1;
    const DIVIDER_BOTTOM_SPACING: i32 = 5;
    const INPUT_TO_LIST_GAP: i32 = DIVIDER_TOP_SPACING + DIVIDER_HEIGHT + DIVIDER_BOTTOM_SPACING;
    const MODE_STRIP_HEIGHT: i32 = 16;
    const STATUS_HEIGHT: i32 = 18;
    const NO_RESULTS_INLINE_WIDTH: i32 = 96;
    const ROW_HEIGHT: i32 = 58;
    const LIST_RADIUS: i32 = 16;
    const MAX_VISIBLE_ROWS: usize = 8;
    const ROW_INSET_X: i32 = 10;
    const ROW_ICON_SIZE: i32 = 34;
    const ROW_ICON_DRAW_SIZE: i32 = 32;
    const ROW_ICON_GAP: i32 = 10;
    const ROW_VERTICAL_INSET: i32 = 2;
    const ROW_ACTIVE_RADIUS: i32 = 10;
    const ROW_TITLE_BLOCK_HEIGHT: i32 = 21;
    const ROW_META_BLOCK_HEIGHT: i32 = 16;
    const ROW_TEXT_LINE_GAP: i32 = 3;
    const HEADER_ROW_LABEL_HEIGHT: i32 = 14;
    const HEADER_ROW_LINE_GAP: i32 = 10;
    const HEADER_ROW_LINE_HEIGHT: i32 = 1;
    const FOOTER_HINT_HEIGHT: i32 = 26;
    const FOOTER_SEPARATOR_HEIGHT: i32 = 1;
    const FOOTER_CONTENT_PAD_Y: i32 = 4;
    const FOOTER_SEPARATOR_TO_CONTENT_GAP: i32 = 10;
    const FOOTER_CONTENT_PAD_X: i32 = 14;
    const FOOTER_HINT_LABEL_OPEN: &str = "Open";
    const FOOTER_HINT_LABEL_MOVE: &str = "Move";
    const FOOTER_HINT_LABEL_CLOSE: &str = "Close";
    const FOOTER_KEY_ENTER: &str = "\u{21B5}";
    const FOOTER_KEY_UP: &str = "\u{2191}";
    const FOOTER_KEY_DOWN: &str = "\u{2193}";
    const FOOTER_KEY_ESC: &str = "Esc";
    const FOOTER_KEYCAP_HEIGHT: i32 = 18;
    const FOOTER_KEYCAP_PAD_X: i32 = 8;
    const FOOTER_KEYCAP_MIN_WIDTH: i32 = 22;
    const FOOTER_KEYCAP_RADIUS: i32 = 8;
    const FOOTER_KEYCAP_GAP: i32 = 6;
    const FOOTER_KEYCAP_TEXT_NUDGE_Y: i32 = -1;
    const FOOTER_HINT_GROUP_GAP: i32 = 10;
    const FOOTER_HINT_LABEL_GAP: i32 = 6;

    const CONTROL_ID_INPUT: usize = 1001;
    const CONTROL_ID_LIST: usize = 1002;
    const CONTROL_ID_STATUS: usize = 1003;
    const CONTROL_ID_HELP: usize = 1004;
    const CONTROL_ID_HELP_TIP: usize = 1005;
    const CONTROL_ID_FOOTER_HINT: usize = 1006;
    const CONTROL_ID_MODE_STRIP: usize = 1007;
    const STATIC_NOTIFY_STYLE: u32 = 0x0100; // SS_NOTIFY
    const STATIC_CENTER_STYLE: u32 = 0x00000001; // SS_CENTER
    const STATIC_RIGHT_STYLE: u32 = 0x00000002; // SS_RIGHT
    const EX_NOACTIVATE_STYLE: u32 = 0x08000000; // WS_EX_NOACTIVATE

    const SWIFTFIND_WM_ESCAPE: u32 = WM_APP + 1;
    const SWIFTFIND_WM_QUERY_CHANGED: u32 = WM_APP + 2;
    const SWIFTFIND_WM_MOVE_UP: u32 = WM_APP + 3;
    const SWIFTFIND_WM_MOVE_DOWN: u32 = WM_APP + 4;
    const SWIFTFIND_WM_SUBMIT: u32 = WM_APP + 5;
    const SWIFTFIND_WM_EXTERNAL_SHOW: u32 = WM_APP + 16;
    const SWIFTFIND_WM_EXTERNAL_QUIT: u32 = WM_APP + 17;
    const EM_GETRECT: u32 = 0x00B2;
    const EM_SETRECTNP: u32 = 0x00B4;

    const TIMER_WINDOW_ANIM: usize = 0xBEF1;
    const TIMER_HELP_HOVER: usize = 0xBEF3;
    const TIMER_ICON_CACHE_IDLE: usize = 0xBEF4;
    const TIMER_RESULTS_CONTENT_FADE: usize = 0xBEF5;
    const TIMER_COMMAND_BADGE_FADE: usize = 0xBEF6;
    const TIMER_QUERY_DEBOUNCE: usize = 0xBEF7;

    const OVERLAY_ANIM_MS: u32 = 150;
    const OVERLAY_HIDE_ANIM_MS: u32 = 115;
    const OVERLAY_ALPHA_OPAQUE: u8 = 255;
    // Results panel expand/collapse animation (scroll behavior remains immediate).
    const RESULTS_ANIM_MS: u32 = 110;
    const RESULTS_CONTENT_FADE_MS: u32 = 120;
    const ANIM_FRAME_MS: u64 = 8;
    const WHEEL_LINES_PER_NOTCH: i32 = 3;
    const MAX_PENDING_WHEEL_DELTA: i32 = 120 * 8;
    const HELP_HOVER_POLL_MS: u32 = 33;
    const DEFAULT_ICON_CACHE_IDLE_MS: u32 = 90_000;
    const DEFAULT_ICON_CACHE_MAX_ENTRIES: usize = 96;
    const NO_RESULTS_FADE_MS: u32 = 85;
    static ICON_CACHE_IDLE_MS_RUNTIME: AtomicU32 = AtomicU32::new(DEFAULT_ICON_CACHE_IDLE_MS);
    static ICON_CACHE_MAX_ENTRIES_RUNTIME: AtomicUsize =
        AtomicUsize::new(DEFAULT_ICON_CACHE_MAX_ENTRIES);

    // Typography tokens.
    const FONT_INPUT_HEIGHT: i32 = -19;
    const FONT_TITLE_HEIGHT: i32 = -15;
    const FONT_META_HEIGHT: i32 = -13;
    const FONT_STATUS_HEIGHT: i32 = -11;
    const FONT_HEADER_HEIGHT: i32 = -12;
    const FONT_TOP_HIT_HEIGHT: i32 = -16;
    const FONT_HINT_HEIGHT: i32 = -11;
    const FONT_HELP_TIP_HEIGHT: i32 = -11;
    const FONT_HELP_ICON_HEIGHT: i32 = -14;
    const FONT_FOOTER_HEIGHT: i32 = -13;
    const FONT_COMMAND_ICON_HEIGHT: i32 = -16;
    const FONT_COMMAND_PREFIX_HEIGHT: i32 = -22;
    const FONT_COMMAND_BADGE_HEIGHT: i32 = -24;
    const FONT_WEIGHT_INPUT: i32 = 400;
    const FONT_WEIGHT_TITLE: i32 = 500;
    const FONT_WEIGHT_META: i32 = 500;
    const FONT_WEIGHT_STATUS: i32 = 400;
    const FONT_WEIGHT_HEADER: i32 = 500;
    const FONT_WEIGHT_TOP_HIT: i32 = 600;
    const FONT_WEIGHT_HINT: i32 = 400;
    const FONT_WEIGHT_HELP_TIP: i32 = 400;
    const FONT_WEIGHT_HELP_ICON: i32 = 400;
    const FONT_WEIGHT_FOOTER: i32 = 500;
    const FONT_WEIGHT_COMMAND_ICON: i32 = 400;
    const FONT_WEIGHT_COMMAND_PREFIX: i32 = 800;
    const FONT_WEIGHT_COMMAND_BADGE: i32 = 800;
    const ICON_FONT_FAMILY_PRIMARY: &str = "Segoe Fluent Icons";
    const ICON_FONT_FAMILY_FALLBACK: &str = "Segoe MDL2 Assets";
    const COMMAND_PREFIX_FONT_FAMILY: &str = "Segoe Fluent Icons";
    const INPUT_TEXT_SHIFT_X: i32 = 10;
    const INPUT_TEXT_SHIFT_Y: i32 = 0;
    const INPUT_TEXT_LINE_HEIGHT_FALLBACK: i32 = 20;
    const INPUT_TEXT_LEFT_INSET: i32 = 19;
    const INPUT_TEXT_RIGHT_INSET: i32 = 10;
    const COMMAND_PREFIX_TEXT: &str = "\u{E76C}";
    const COMMAND_PREFIX_RESERVED_WIDTH: i32 = 34;
    const COMMAND_PREFIX_GAP: i32 = 12;
    const COMMAND_PREFIX_LEFT_SHIFT: i32 = 20;
    const COMMAND_PREFIX_INPUT_PAD: i32 = 16;
    const COMMAND_BADGE_INPUT_PAD: i32 = 20;
    const COMMAND_PREFIX_OPACITY: f32 = 0.60;
    const COMMAND_PREFIX_EMBOLDEN_OPACITY: f32 = 0.40;
    const COMMAND_PREFIX_EMBOLDEN_OFFSET_PX: i32 = 1;
    const COMMAND_BADGE_TEXT: &str = "U";
    const COMMAND_BADGE_GAP_FROM_PREFIX: i32 = 1;
    const COMMAND_BADGE_ANIM_MS: u32 = 110;
    const COMMAND_BADGE_SLIDE_PX: i32 = 6;
    const HELP_ICON_SIZE: i32 = 14;
    const HELP_ICON_RIGHT_INSET: i32 = 12;
    const HELP_ICON_GAP_FROM_INPUT: i32 = 8;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum OverlayTheme {
        Dark,
        Light,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct OverlayPalette {
        panel_bg: u32,
        panel_border: u32,
        input_bg: u32,
        results_bg: u32,
        text_primary: u32,
        text_secondary: u32,
        text_error: u32,
        text_highlight: u32,
        text_hint: u32,
        text_section: u32,
        text_hint_footer: u32,
        text_mode_strip: u32,
        selection: u32,
        selection_border: u32,
        row_hover: u32,
        row_separator: u32,
        selection_accent: u32,
        icon_bg: u32,
        icon_text: u32,
        help_icon: u32,
        help_icon_hover: u32,
        help_tip_bg: u32,
        help_tip_text: u32,
    }

    const PALETTE_DARK: OverlayPalette = OverlayPalette {
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

    const PALETTE_LIGHT: OverlayPalette = OverlayPalette {
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
    const HELP_TIP_WIDTH: i32 = 132;
    const HELP_TIP_HEIGHT: i32 = 26;
    const HELP_TIP_RADIUS: i32 = 10;
    const HELP_TIP_TEXT_PAD_X: i32 = 8;
    const DEFAULT_FONT_FAMILY: &str = "Segoe UI Variable Text";
    const GEIST_FONT_FAMILY: &str = "Geist";
    const HOTKEY_HELP_TEXT_FALLBACK: &str = "Click to change hotkey";
    const HELP_ICON_TEXT: &str = "\u{E946}";
    const NO_RESULTS_STATUS_TEXT: &str = "No results";
    const INPUT_PLACEHOLDER_TEXT: &str = "Type to search";
    const COMMAND_INPUT_PLACEHOLDER_TEXT: &str = "Search the web or run a command";
    const FOOTER_HINT_TEXT: &str = "Enter Open  •  ↑↓ Move  •  Esc Close";
    const MODE_STRIP_DEFAULT_TEXT: &str = "All   Apps   Files   Actions   Clipboard";

    fn palette_for_theme(theme: OverlayTheme) -> OverlayPalette {
        match theme {
            OverlayTheme::Dark => PALETTE_DARK,
            OverlayTheme::Light => PALETTE_LIGHT,
        }
    }

    fn detect_system_theme() -> OverlayTheme {
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
        QueryDelayElapsed,
        MoveSelection(i32),
        Submit,
        Escape,
        ExternalShow,
        ExternalQuit,
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
        hwnd: HWND,
    }

    struct OverlayShellState {
        edit_hwnd: HWND,
        list_hwnd: HWND,
        status_hwnd: HWND,
        help_hwnd: HWND,
        help_tip_hwnd: HWND,
        footer_hint_hwnd: HWND,
        mode_strip_hwnd: HWND,

        edit_prev_proc: isize,
        list_prev_proc: isize,
        help_prev_proc: isize,
        help_tip_prev_proc: isize,
        footer_hint_prev_proc: isize,

        input_font: isize,
        title_font: isize,
        meta_font: isize,
        status_font: isize,
        header_font: isize,
        top_hit_font: isize,
        hint_font: isize,
        help_tip_font: isize,
        help_icon_font: isize,
        footer_font: isize,
        command_prefix_font: isize,
        command_badge_font: isize,
        command_icon_font: isize,
        command_icon_fallback_font: isize,

        panel_brush: isize,
        border_brush: isize,
        input_brush: isize,
        results_brush: isize,
        selection_brush: isize,
        selection_border_brush: isize,
        row_hover_brush: isize,
        row_separator_brush: isize,
        selection_accent_brush: isize,
        icon_brush: isize,
        help_tip_brush: isize,
        help_tip_border_brush: isize,
        theme: OverlayTheme,
        palette: OverlayPalette,

        status_is_error: bool,
        no_results_mode: bool,
        no_results_anim_pending: bool,
        status_center_aligned: bool,
        help_hovered: bool,
        help_tip_visible: bool,
        results_visible: bool,
        dwm_rounded_enabled: bool,
        help_config_path: String,
        active_query: String,
        command_mode_input: bool,
        command_uninstall_quick_mode: bool,
        command_badge_anim_start: Option<Instant>,
        expanded_rows: i32,
        placeholder_hint: String,
        mode_strip_text: String,

        hover_index: i32,
        wheel_delta_remainder: i32,
        pending_wheel_delta: i32,
        suppress_next_hover_sync: bool,
        results_content_anim_start: Option<Instant>,

        window_anim: Option<WindowAnimation>,
        rows: Vec<OverlayRow>,
        icon_cache: HashMap<String, isize>,
        icon_cache_lru: VecDeque<String>,
        icon_cache_metrics: IconCacheMetrics,
    }

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct IconCacheMetrics {
        hits: u64,
        misses: u64,
        load_failures: u64,
        evictions: u64,
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
                footer_font: 0,
                command_prefix_font: 0,
                command_badge_font: 0,
                command_icon_font: 0,
                command_icon_fallback_font: 0,
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
                help_tip_brush: 0,
                help_tip_border_brush: 0,
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
                rows: Vec::new(),
                icon_cache: HashMap::new(),
                icon_cache_lru: VecDeque::new(),
                icon_cache_metrics: IconCacheMetrics::default(),
            }
        }
    }

    struct WindowAnimation {
        start: Instant,
        duration_ms: u32,
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
        hide_on_complete: bool,
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
            hide_overlay_immediate(self.hwnd);
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

        pub fn schedule_query_delay(&self, delay_ms: u32) {
            let delay = delay_ms.clamp(10, 2_000);
            unsafe {
                KillTimer(self.hwnd, TIMER_QUERY_DEBOUNCE);
                SetTimer(self.hwnd, TIMER_QUERY_DEBOUNCE, delay, None);
            }
        }

        pub fn cancel_query_delay(&self) {
            unsafe {
                KillTimer(self.hwnd, TIMER_QUERY_DEBOUNCE);
            }
        }

        pub fn set_performance_tuning(
            &self,
            idle_cache_trim_ms: u32,
            active_memory_target_mb: u16,
        ) {
            configure_runtime_performance_tuning(idle_cache_trim_ms, active_memory_target_mb);
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

                if rows.is_empty() {
                    schedule_icon_cache_idle_cleanup(self.hwnd);
                    state.results_content_anim_start = None;
                    unsafe {
                        KillTimer(self.hwnd, TIMER_RESULTS_CONTENT_FADE);
                    }
                    if state.results_visible && !state.rows.is_empty() {
                        self.collapse_results();
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
                let status_only_row =
                    rows.len() == 1 && matches!(rows[0].role, OverlayRowRole::Status);
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
                    InvalidateRect(self.hwnd, std::ptr::null(), 0);
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
            let count = unsafe { SendMessageW(state.list_hwnd, LB_GETCOUNT, 0, 0) as i32 };
            if state.hover_index >= 0 && state.hover_index < count {
                return row_result_index(state, state.hover_index as usize);
            }
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

                if msg.message == SWIFTFIND_WM_QUERY_CHANGED {
                    // Coalesce bursts of EN_CHANGE notifications into one query update.
                    let mut drain: MSG = unsafe { std::mem::zeroed() };
                    loop {
                        let removed = unsafe {
                            PeekMessageW(
                                &mut drain,
                                std::ptr::null_mut(),
                                SWIFTFIND_WM_QUERY_CHANGED,
                                SWIFTFIND_WM_QUERY_CHANGED,
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
                if msg.message == WM_TIMER && msg.wParam == TIMER_QUERY_DEBOUNCE {
                    unsafe {
                        KillTimer(self.hwnd, TIMER_QUERY_DEBOUNCE);
                    }
                    on_event(OverlayEvent::QueryDelayElapsed);
                    continue;
                }

                match msg.message {
                    WM_HOTKEY => on_event(OverlayEvent::Hotkey(msg.wParam as i32)),
                    SWIFTFIND_WM_MOVE_UP => on_event(OverlayEvent::MoveSelection(-1)),
                    SWIFTFIND_WM_MOVE_DOWN => on_event(OverlayEvent::MoveSelection(1)),
                    SWIFTFIND_WM_SUBMIT => on_event(OverlayEvent::Submit),
                    SWIFTFIND_WM_ESCAPE => on_event(OverlayEvent::Escape),
                    SWIFTFIND_WM_EXTERNAL_SHOW => on_event(OverlayEvent::ExternalShow),
                    SWIFTFIND_WM_EXTERNAL_QUIT => on_event(OverlayEvent::ExternalQuit),
                    _ => {}
                }

                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
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
                    ShowWindow(state.list_hwnd, SW_HIDE);
                    ShowWindow(state.footer_hint_hwnd, SW_HIDE);
                    ShowWindow(state.mode_strip_hwnd, SW_HIDE);
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
            let final_left = rect.left;
            let final_top = rect.top;
            let final_width = rect.right - rect.left;
            let final_height = rect.bottom - rect.top;

            let end_width = ((final_width as f32) * 0.96_f32) as i32;
            let end_height = ((final_height as f32) * 0.96_f32) as i32;
            let end_left = final_left + (final_width - end_width) / 2;
            let end_top = final_top + (final_height - end_height) / 2;
            start_window_animation(
                self.hwnd,
                final_left,
                final_top,
                final_width,
                final_height,
                end_left,
                end_top,
                end_width,
                end_height,
                OVERLAY_ALPHA_OPAQUE,
                0,
                OVERLAY_HIDE_ANIM_MS,
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
                    state.theme = detect_system_theme();
                    state.palette = palette_for_theme(state.theme);
                    state.dwm_rounded_enabled = try_enable_dwm_rounded_corners(hwnd);
                    state.panel_brush =
                        unsafe { CreateSolidBrush(state.palette.panel_bg) } as isize;
                    state.border_brush =
                        unsafe { CreateSolidBrush(state.palette.panel_border) } as isize;
                    state.input_brush =
                        unsafe { CreateSolidBrush(state.palette.input_bg) } as isize;
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
                    state.help_tip_brush =
                        unsafe { CreateSolidBrush(state.palette.help_tip_bg) } as isize;
                    state.help_tip_border_brush =
                        unsafe { CreateSolidBrush(state.palette.panel_border) } as isize;
                    crate::logging::info(&format!(
                        "[swiftfind-core] overlay_theme mode={}",
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
                    state.footer_font = create_font(FONT_FOOTER_HEIGHT, FONT_WEIGHT_FOOTER);
                    state.command_prefix_font = create_font_with_family(
                        FONT_COMMAND_PREFIX_HEIGHT,
                        FONT_WEIGHT_COMMAND_PREFIX,
                        command_prefix_font_family_wide(),
                    );
                    state.command_badge_font =
                        create_font(FONT_COMMAND_BADGE_HEIGHT, FONT_WEIGHT_COMMAND_BADGE);
                    state.command_icon_font = create_font_with_family(
                        FONT_COMMAND_ICON_HEIGHT,
                        FONT_WEIGHT_COMMAND_ICON,
                        icon_font_family_primary_wide(),
                    );
                    state.command_icon_fallback_font = create_font_with_family(
                        FONT_COMMAND_ICON_HEIGHT,
                        FONT_WEIGHT_COMMAND_ICON,
                        icon_font_family_fallback_wide(),
                    );

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
                            state.help_tip_hwnd,
                            WM_SETFONT,
                            state.help_tip_font as usize,
                            1,
                        );
                        state.edit_prev_proc = SetWindowLongPtrW(
                            state.edit_hwnd,
                            GWLP_WNDPROC,
                            control_subclass_proc as *const () as isize,
                        );
                        state.list_prev_proc = SetWindowLongPtrW(
                            state.list_hwnd,
                            GWLP_WNDPROC,
                            control_subclass_proc as *const () as isize,
                        );
                        state.help_prev_proc = SetWindowLongPtrW(
                            state.help_hwnd,
                            GWLP_WNDPROC,
                            control_subclass_proc as *const () as isize,
                        );
                        state.help_tip_prev_proc = SetWindowLongPtrW(
                            state.help_tip_hwnd,
                            GWLP_WNDPROC,
                            control_subclass_proc as *const () as isize,
                        );
                        state.footer_hint_prev_proc = SetWindowLongPtrW(
                            state.footer_hint_hwnd,
                            GWLP_WNDPROC,
                            control_subclass_proc as *const () as isize,
                        );
                        SetWindowLongPtrW(state.help_tip_hwnd, GWLP_USERDATA, hwnd as isize);

                        ShowWindow(state.list_hwnd, SW_HIDE);
                        ShowWindow(state.help_tip_hwnd, SW_HIDE);
                        ShowWindow(state.footer_hint_hwnd, SW_HIDE);
                        ShowWindow(state.mode_strip_hwnd, SW_HIDE);
                    }

                    state.results_visible = false;
                    state.hover_index = -1;
                    layout_children(hwnd, state);
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
                        PostMessageW(hwnd, SWIFTFIND_WM_QUERY_CHANGED, 0, 0);
                    }
                    return 0;
                }
                if control_id == CONTROL_ID_LIST && notification as u32 == LBN_DBLCLK as u32 {
                    unsafe {
                        PostMessageW(hwnd, SWIFTFIND_WM_SUBMIT, 0, 0);
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
                    if target == state.help_tip_hwnd {
                        unsafe {
                            SetTextColor(wparam as _, state.palette.help_tip_text);
                            SetBkColor(wparam as _, state.palette.help_tip_bg);
                            SetBkMode(wparam as _, OPAQUE as i32);
                        }
                        return state.help_tip_brush;
                    }
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
                        PostMessageW(hwnd, SWIFTFIND_WM_ESCAPE, 0, 0);
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
                            unsafe {
                                KillTimer(hwnd, TIMER_RESULTS_CONTENT_FADE);
                            }
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
            WM_CLOSE => {
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                }
                0
            }
            WM_DESTROY => {
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
            SWIFTFIND_WM_ESCAPE
            | SWIFTFIND_WM_QUERY_CHANGED
            | SWIFTFIND_WM_MOVE_UP
            | SWIFTFIND_WM_MOVE_DOWN
            | SWIFTFIND_WM_SUBMIT => 0,
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    extern "system" fn control_subclass_proc(
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
                        PostMessageW(parent, SWIFTFIND_WM_QUERY_CHANGED, 0, 0);
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
                        PostMessageW(parent, SWIFTFIND_WM_QUERY_CHANGED, 0, 0);
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
                            PostMessageW(parent, SWIFTFIND_WM_QUERY_CHANGED, 0, 0);
                        }
                        return 0;
                    }
                }
                // Suppress default control beep for handled launcher keys.
                // Enter submits through WM_KEYDOWN -> SWIFTFIND_WM_SUBMIT.
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
                    let selected_before = unsafe { SendMessageW(hwnd, LB_GETCURSEL, 0, 0) as i32 };
                    state.hover_index = next_hover;
                    if next_hover >= 0 && next_hover != selected_before {
                        unsafe {
                            SendMessageW(hwnd, LB_SETCURSEL, next_hover as usize, 0);
                        }
                    }
                    let selected_after = unsafe { SendMessageW(hwnd, LB_GETCURSEL, 0, 0) as i32 };
                    invalidate_list_row(hwnd, previous_hover);
                    invalidate_list_row(hwnd, next_hover);
                    invalidate_list_row(hwnd, selected_before);
                    invalidate_list_row(hwnd, selected_after);
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
                            PostMessageW(parent, SWIFTFIND_WM_SUBMIT, 0, 0);
                        }
                    }
                }
                return 0;
            }
            if message == WM_LBUTTONUP && (hwnd == state.help_hwnd || hwnd == state.help_tip_hwnd) {
                if let Err(error) = open_help_config_file(state) {
                    state.status_is_error = true;
                    state.help_tip_visible = false;
                    let wide = to_wide(&format!("Help open error: {error}"));
                    unsafe {
                        SetWindowTextW(state.status_hwnd, wide.as_ptr());
                        InvalidateRect(state.status_hwnd, std::ptr::null(), 1);
                    }
                    layout_children(parent, state);
                }
                return 0;
            }
        }

        if message == WM_KEYDOWN {
            match wparam as u16 {
                VK_ESCAPE => {
                    unsafe {
                        PostMessageW(parent, SWIFTFIND_WM_ESCAPE, 0, 0);
                    }
                    return 0;
                }
                VK_UP => {
                    unsafe {
                        PostMessageW(parent, SWIFTFIND_WM_MOVE_UP, 0, 0);
                    }
                    return 0;
                }
                VK_DOWN => {
                    unsafe {
                        PostMessageW(parent, SWIFTFIND_WM_MOVE_DOWN, 0, 0);
                    }
                    return 0;
                }
                VK_RETURN => {
                    unsafe {
                        PostMessageW(parent, SWIFTFIND_WM_SUBMIT, 0, 0);
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
            std::mem::transmute::<isize, windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC>(
                prev_ptr,
            )
        };
        let result = unsafe { CallWindowProcW(prev_proc, hwnd, message, wparam, lparam) };
        if hwnd == state.edit_hwnd && message == WM_PAINT {
            paint_edit_placeholder(hwnd, state);
            paint_edit_command_prefix(hwnd, state);
        }
        result
    }

    fn paint_edit_placeholder(edit_hwnd: HWND, state: &OverlayShellState) {
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

    fn paint_edit_command_prefix(edit_hwnd: HWND, state: &OverlayShellState) {
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
        let reserved =
            COMMAND_PREFIX_RESERVED_WIDTH + COMMAND_PREFIX_GAP + COMMAND_PREFIX_LEFT_SHIFT;
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
                let badge_color =
                    blend_color(state.palette.input_bg, state.palette.text_hint, opacity);
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

    fn command_badge_animation_tick(state: &mut OverlayShellState) -> bool {
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

    fn set_uninstall_quick_mode(
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

    fn hide_input_caret(edit_hwnd: HWND) {
        unsafe {
            let _ = HideCaret(edit_hwnd);
        }
    }

    fn draw_list_row(hwnd: HWND, dis: &mut DRAWITEMSTRUCT) {
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
        let selected_visible =
            !status_row && !section_row && selected_flag && (state.hover_index < 0 || hovered);
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
                        let line_color = blend_color(
                            palette.results_bg,
                            palette.row_separator,
                            content_progress,
                        );
                        let line_brush = CreateSolidBrush(line_color);
                        FillRect(dis.hDC, &line_rect, line_brush as _);
                        DeleteObject(line_brush as _);
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
                let hover_color =
                    blend_color(palette.results_bg, palette.row_hover, content_progress);
                let fill_brush = CreateSolidBrush(hover_color);
                let fill_pen = CreatePen(PS_SOLID, 1, hover_color);
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
                DeleteObject(fill_pen as _);
                DeleteObject(fill_brush as _);
            }

            let old_font = SelectObject(dis.hDC, state.title_font as _);
            SetBkMode(dis.hDC, TRANSPARENT as i32);
            let primary_text =
                blend_color(palette.results_bg, palette.text_primary, content_progress);
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
                let total_height =
                    ROW_TITLE_BLOCK_HEIGHT + ROW_TEXT_LINE_GAP + ROW_META_BLOCK_HEIGHT;
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

    fn row_is_selectable(state: &OverlayShellState, index: usize) -> bool {
        state.rows.get(index).is_some_and(|row| {
            matches!(row.role, OverlayRowRole::Item | OverlayRowRole::TopHit)
                && row.result_index >= 0
        })
    }

    fn row_result_index(state: &OverlayShellState, index: usize) -> Option<usize> {
        state.rows.get(index).and_then(|row| {
            if matches!(row.role, OverlayRowRole::Item | OverlayRowRole::TopHit)
                && row.result_index >= 0
            {
                Some(row.result_index as usize)
            } else {
                None
            }
        })
    }

    fn row_index_for_result_index(state: &OverlayShellState, result_index: usize) -> Option<usize> {
        state.rows.iter().position(|row| {
            matches!(row.role, OverlayRowRole::Item | OverlayRowRole::TopHit)
                && row.result_index == result_index as i32
        })
    }

    fn initial_visible_row_count(rows: &[OverlayRow]) -> usize {
        if rows.is_empty() {
            return 0;
        }

        let mut selectable_seen = 0usize;
        let mut rendered_rows = 0usize;
        for row in rows {
            rendered_rows += 1;
            if matches!(row.role, OverlayRowRole::Item | OverlayRowRole::TopHit) {
                selectable_seen += 1;
                if selectable_seen >= MAX_VISIBLE_ROWS {
                    break;
                }
            }
        }

        rendered_rows.max(1)
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
        cursor.x >= rect.left
            && cursor.x < rect.right
            && cursor.y >= rect.top
            && cursor.y < rect.bottom
    }

    fn complete_window_animation_if_running(hwnd: HWND, state: &mut OverlayShellState) {
        let Some(anim) = state.window_anim.take() else {
            return;
        };

        unsafe {
            KillTimer(hwnd, TIMER_WINDOW_ANIM);
        }

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
                SetLayeredWindowAttributes(hwnd, 0, OVERLAY_ALPHA_OPAQUE, LWA_ALPHA);
            }
            state.pending_wheel_delta = 0;
            return;
        }

        layout_children(hwnd, state);
        flush_pending_wheel_after_animation(state);
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

    fn measure_text_size(hdc: HDC, text: &str) -> SIZE {
        if text.is_empty() {
            return SIZE { cx: 0, cy: 0 };
        }
        let wide = to_wide_no_nul(text);
        let mut size: SIZE = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size) };
        if ok == 0 {
            SIZE {
                cx: 0,
                cy: current_text_height(hdc),
            }
        } else {
            size
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ActionIconKind {
        WebSearch,
        Uninstall,
        Clipboard,
        Settings,
        Diagnostics,
        Logs,
        Rebuild,
        Generic,
    }

    fn action_icon_kind_for_title(title: &str) -> ActionIconKind {
        let lower = title.to_ascii_lowercase();
        if lower.contains("web") || lower.contains("search") {
            ActionIconKind::WebSearch
        } else if lower.contains("uninstall") || lower.contains("remove") {
            ActionIconKind::Uninstall
        } else if lower.contains("clipboard") {
            ActionIconKind::Clipboard
        } else if lower.contains("config") || lower.contains("setting") || lower.contains("prefer")
        {
            ActionIconKind::Settings
        } else if lower.contains("diagnostic")
            || lower.contains("bundle")
            || lower.contains("support")
        {
            ActionIconKind::Diagnostics
        } else if lower.contains("log") {
            ActionIconKind::Logs
        } else if lower.contains("rebuild") || lower.contains("index") || lower.contains("refresh")
        {
            ActionIconKind::Rebuild
        } else {
            ActionIconKind::Generic
        }
    }

    fn action_icon_codepoint(kind: ActionIconKind) -> u32 {
        match kind {
            ActionIconKind::WebSearch => 0xE721,   // Search
            ActionIconKind::Uninstall => 0xE74D,   // Delete
            ActionIconKind::Clipboard => 0xE8C8,   // Clipboard List
            ActionIconKind::Settings => 0xE713,    // Settings
            ActionIconKind::Diagnostics => 0xE8A5, // Page/Report
            ActionIconKind::Logs => 0xE8B7,        // Folder
            ActionIconKind::Rebuild => 0xE895,     // Sync
            ActionIconKind::Generic => 0xE756,     // Command Prompt
        }
    }

    fn draw_action_icon(
        hdc: HDC,
        icon_rect: &RECT,
        row: &OverlayRow,
        state: &OverlayShellState,
        color: u32,
    ) -> bool {
        if !row.kind.eq_ignore_ascii_case("action") {
            return false;
        }
        let codepoint = action_icon_codepoint(action_icon_kind_for_title(&row.title));
        let Some(glyph) = char::from_u32(codepoint) else {
            return false;
        };
        let glyph_text = glyph.to_string();
        let glyph_wide = to_wide(&glyph_text);
        let mut glyph_rect = *icon_rect;
        glyph_rect.top += 1;

        unsafe {
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, color);

            if state.command_icon_font != 0 {
                let old_font = SelectObject(hdc, state.command_icon_font as _);
                let drawn = DrawTextW(
                    hdc,
                    glyph_wide.as_ptr(),
                    -1,
                    &mut glyph_rect,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                ) != 0;
                SelectObject(hdc, old_font);
                if drawn {
                    return true;
                }
            }

            if state.command_icon_fallback_font != 0 {
                let old_font = SelectObject(hdc, state.command_icon_fallback_font as _);
                let drawn = DrawTextW(
                    hdc,
                    glyph_wide.as_ptr(),
                    -1,
                    &mut glyph_rect,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                ) != 0;
                SelectObject(hdc, old_font);
                if drawn {
                    return true;
                }
            }
        }
        false
    }

    fn draw_row_icon(
        hdc: HDC,
        icon_rect: &RECT,
        row: &OverlayRow,
        state: &mut OverlayShellState,
    ) -> bool {
        let Some(icon_handle) = icon_handle_for_row(state, row) else {
            return false;
        };
        let icon_size = ROW_ICON_DRAW_SIZE;
        let x = icon_rect.left + (ROW_ICON_SIZE - icon_size) / 2;
        let y = icon_rect.top + (ROW_ICON_SIZE - icon_size) / 2;
        unsafe {
            DrawIconEx(
                hdc,
                x,
                y,
                icon_handle as _,
                icon_size,
                icon_size,
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            ) != 0
        }
    }

    fn icon_handle_for_row(state: &mut OverlayShellState, row: &OverlayRow) -> Option<isize> {
        let key = icon_cache_key(row);
        if let Some(cached) = state.icon_cache.get(&key).copied() {
            state.icon_cache_metrics.hits = state.icon_cache_metrics.hits.saturating_add(1);
            touch_icon_cache_key(state, &key);
            return if cached == 0 { None } else { Some(cached) };
        }
        state.icon_cache_metrics.misses = state.icon_cache_metrics.misses.saturating_add(1);

        let loaded = load_shell_icon_for_row(row).unwrap_or(0);
        if loaded == 0 {
            state.icon_cache_metrics.load_failures =
                state.icon_cache_metrics.load_failures.saturating_add(1);
        }
        insert_icon_cache_entry(state, key, loaded);
        if loaded == 0 {
            None
        } else {
            Some(loaded)
        }
    }

    fn icon_cache_key(row: &OverlayRow) -> String {
        let kind = row.kind.to_ascii_lowercase();
        let source = row.icon_path.trim().to_ascii_lowercase();
        if source.is_empty() {
            format!("kind:{kind}")
        } else {
            format!("kind:{kind}|{source}")
        }
    }

    fn load_shell_icon_for_row(row: &OverlayRow) -> Option<isize> {
        let kind = row.kind.to_ascii_lowercase();
        let source = row.icon_path.trim();
        let is_app_shortcut = kind == "app" && source.to_ascii_lowercase().ends_with(".lnk");

        // Action/command rows are semantic operations, not filesystem targets.
        // Force deterministic in-app iconography instead of generic shell-file icons.
        if kind == "action" {
            return None;
        }

        if kind == "folder" {
            return shell_icon_with_attrs("folder", FILE_ATTRIBUTE_DIRECTORY);
        }

        if !source.is_empty() {
            if let Some(icon) = shell_icon_from_appsfolder_target(source) {
                return Some(icon);
            }
            if is_app_shortcut {
                if let Some(icon) = executable_icon_from_shortcut_hlink(source) {
                    return Some(icon);
                }
                if let Some(icon) = shortcut_target_icon(source) {
                    return Some(icon);
                }
                if let Some(icon) = executable_icon_from_shortcut(source) {
                    return Some(icon);
                }
                if let Some(icon) = shortcut_system_icon_without_overlay(source) {
                    return Some(icon);
                }
                // Do not extract icon directly from `.lnk` for app entries:
                // this is the primary source of shortcut-arrow overlays.
                if let Some(icon) = shell_icon_with_attrs("swiftfind.exe", FILE_ATTRIBUTE_NORMAL) {
                    return Some(icon);
                }
                return None;
            }
            if let Some(icon) = shell_icon_for_existing_path(source) {
                return Some(icon);
            }
            if let Some(icon) = shell_icon_with_attrs(source, FILE_ATTRIBUTE_NORMAL) {
                return Some(icon);
            }
        }

        if kind == "app" {
            if let Some(icon) = shell_icon_with_attrs("swiftfind.exe", FILE_ATTRIBUTE_NORMAL) {
                return Some(icon);
            }
        }

        shell_icon_with_attrs("swiftfind.file", FILE_ATTRIBUTE_NORMAL)
    }

    fn shortcut_target_icon(shortcut_path: &str) -> Option<isize> {
        let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let wide_shortcut = to_wide(shortcut_path);
        let result = unsafe {
            SHGetFileInfoW(
                wide_shortcut.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICONLOCATION,
            )
        };
        if result == 0 {
            return None;
        }

        let icon_source = wide_buf_to_string(&info.szDisplayName);
        if icon_source.trim().is_empty() {
            return None;
        }
        if let Some(icon) = shell_icon_from_appsfolder_target(icon_source.trim()) {
            return Some(icon);
        }
        let (icon_path, parsed_index) = split_icon_resource_spec(icon_source.trim());
        let icon_index = if info.iIcon == 0 {
            parsed_index.unwrap_or(0)
        } else {
            info.iIcon
        };
        extract_icon_from_path(icon_path, icon_index)
    }

    fn extract_icon_from_path(path: &str, icon_index: i32) -> Option<isize> {
        let normalized = normalize_icon_source_path(path);
        if normalized.is_empty() {
            return None;
        }
        let wide_source = to_wide(&normalized);
        let mut large_icon = std::ptr::null_mut();
        let mut small_icon = std::ptr::null_mut();
        let extracted = unsafe {
            ExtractIconExW(
                wide_source.as_ptr(),
                icon_index,
                &mut large_icon,
                &mut small_icon,
                1,
            )
        };

        if !small_icon.is_null() {
            unsafe {
                DestroyIcon(small_icon);
            }
        }

        if extracted == 0 || large_icon.is_null() {
            return shell_icon_from_display_name(&normalized);
        }
        Some(large_icon as isize)
    }

    fn executable_icon_from_shortcut(shortcut_path: &str) -> Option<isize> {
        let wide_shortcut = to_wide(shortcut_path);
        let mut exe_out = vec![0u16; 260];
        let result = unsafe {
            FindExecutableW(
                wide_shortcut.as_ptr(),
                std::ptr::null(),
                exe_out.as_mut_ptr(),
            )
        };
        if (result as isize) <= 32 {
            return None;
        }
        let exe = wide_buf_to_string(&exe_out);
        let normalized = normalize_icon_source_path(exe.trim());
        if normalized.is_empty() {
            return None;
        }
        extract_icon_from_path(&normalized, 0)
    }

    fn executable_icon_from_shortcut_hlink(shortcut_path: &str) -> Option<isize> {
        let wide_shortcut = to_wide(shortcut_path);
        let mut target: windows_sys::core::PWSTR = std::ptr::null_mut();
        let mut location: windows_sys::core::PWSTR = std::ptr::null_mut();
        let hr = unsafe {
            HlinkResolveShortcutToString(wide_shortcut.as_ptr(), &mut target, &mut location)
        };

        let resolved_target = pwstr_to_string_and_free(target);
        let resolved_location = pwstr_to_string_and_free(location);

        if hr < 0 {
            return None;
        }
        let resolved_location_trimmed = resolved_location.trim();
        if !resolved_location_trimmed.is_empty() {
            if let Some(icon) = shell_icon_from_appsfolder_target(resolved_location_trimmed) {
                return Some(icon);
            }
            let (icon_path, parsed_index) = split_icon_resource_spec(resolved_location_trimmed);
            let normalized_icon_path = normalize_icon_source_path(icon_path);
            if is_icon_module_path(&normalized_icon_path) {
                if let Some(icon) =
                    extract_icon_from_path(&normalized_icon_path, parsed_index.unwrap_or(0))
                {
                    return Some(icon);
                }
            }
        }
        if let Some(icon) = shell_icon_from_appsfolder_target(resolved_target.trim()) {
            return Some(icon);
        }
        let normalized = normalize_icon_source_path(resolved_target.trim());
        if !normalized.is_empty() {
            if let Some(icon) = extract_icon_from_path(&normalized, 0) {
                return Some(icon);
            }
        }
        None
    }

    fn shell_icon_for_existing_path(path: &str) -> Option<isize> {
        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let wide = to_wide(path);
        // Prefer direct shell icon extraction for concrete files/apps.
        // This tends to pick a better source icon than generic image-list lookup.
        let flags = SHGFI_ICON | SHGFI_LARGEICON;
        let result = unsafe {
            SHGetFileInfoW(
                wide.as_ptr(),
                0,
                &mut sfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
        if result == 0 || sfi.hIcon.is_null() {
            None
        } else {
            Some(sfi.hIcon as isize)
        }
    }

    fn shell_icon_with_attrs(path_hint: &str, attrs: u32) -> Option<isize> {
        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let wide = to_wide(path_hint);
        let flags = SHGFI_SYSICONINDEX | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES;
        let result = unsafe {
            SHGetFileInfoW(
                wide.as_ptr(),
                attrs,
                &mut sfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
        if result == 0 || sfi.iIcon < 0 {
            None
        } else {
            let icon = unsafe { ImageList_GetIcon(result as _, sfi.iIcon, 0) };
            if icon.is_null() {
                None
            } else {
                Some(icon as isize)
            }
        }
    }

    fn shortcut_system_icon_without_overlay(shortcut_path: &str) -> Option<isize> {
        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let wide = to_wide(shortcut_path);
        let flags = SHGFI_SYSICONINDEX | SHGFI_LARGEICON;
        let result = unsafe {
            SHGetFileInfoW(
                wide.as_ptr(),
                0,
                &mut sfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
        if result == 0 || sfi.iIcon < 0 {
            return None;
        }
        let icon = unsafe { ImageList_GetIcon(result as _, sfi.iIcon, 0) };
        if icon.is_null() {
            None
        } else {
            Some(icon as isize)
        }
    }

    fn shell_icon_from_appsfolder_target(target: &str) -> Option<isize> {
        for candidate in appsfolder_display_name_candidates(target) {
            if let Some(icon) = shell_icon_from_display_name(&candidate) {
                return Some(icon);
            }
        }
        None
    }

    fn appsfolder_display_name_candidates(target: &str) -> Vec<String> {
        let trimmed = target.trim().trim_matches('"');
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut candidates = Vec::with_capacity(6);
        push_unique_candidate(&mut candidates, trimmed);

        if let Some(appsfolder_token) = extract_appsfolder_token(trimmed) {
            push_unique_candidate(&mut candidates, &appsfolder_token);
            if appsfolder_token
                .to_ascii_lowercase()
                .starts_with("shell:appsfolder\\")
            {
                push_unique_candidate(&mut candidates, &appsfolder_token[6..]);
            } else if appsfolder_token
                .to_ascii_lowercase()
                .starts_with("appsfolder\\")
            {
                push_unique_candidate(&mut candidates, &format!("shell:{appsfolder_token}"));
            }
        }

        let lowered = trimmed.to_ascii_lowercase();
        if lowered.starts_with("appsfolder\\") {
            push_unique_candidate(&mut candidates, &format!("shell:{trimmed}"));
        } else if lowered.starts_with("shell:appsfolder\\") {
            push_unique_candidate(&mut candidates, &trimmed[6..]);
        } else if let Some(index) = lowered.find("appsfolder\\") {
            push_unique_candidate(&mut candidates, &format!("shell:{}", &trimmed[index..]));
        }

        candidates
    }

    fn push_unique_candidate(candidates: &mut Vec<String>, value: &str) {
        let normalized = value.trim();
        if normalized.is_empty() {
            return;
        }
        if candidates
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(normalized))
        {
            return;
        }
        candidates.push(normalized.to_string());
    }

    fn extract_appsfolder_token(raw: &str) -> Option<String> {
        let trimmed = raw.trim().trim_matches('"');
        if trimmed.is_empty() {
            return None;
        }

        let lowered = trimmed.to_ascii_lowercase();
        let start = lowered
            .find("shell:appsfolder\\")
            .or_else(|| lowered.find("appsfolder\\"))?;
        let tail = &trimmed[start..];
        if tail.is_empty() {
            return None;
        }

        let mut end = tail.len();
        for (index, ch) in tail.char_indices() {
            if index == 0 {
                continue;
            }
            if ch.is_whitespace() || ch == '"' || ch == '\'' {
                end = index;
                break;
            }
        }

        let token = tail[..end]
            .trim()
            .trim_end_matches(',')
            .trim_end_matches(';')
            .trim_matches('"')
            .trim_matches('\'');
        if token.is_empty() {
            None
        } else {
            Some(token.to_string())
        }
    }

    fn is_icon_module_path(path: &str) -> bool {
        let lowered = path.to_ascii_lowercase();
        lowered.ends_with(".exe") || lowered.ends_with(".dll") || lowered.ends_with(".ico")
    }

    fn shell_icon_from_display_name(display_name: &str) -> Option<isize> {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            return None;
        }

        let wide = to_wide(trimmed);
        let mut pidl: *mut windows_sys::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        let hr = unsafe {
            SHParseDisplayName(
                wide.as_ptr(),
                std::ptr::null_mut(),
                &mut pidl,
                0,
                std::ptr::null_mut(),
            )
        };
        if hr < 0 || pidl.is_null() {
            return None;
        }

        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let flags = SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON;
        let result = unsafe {
            SHGetFileInfoW(
                pidl as *const u16,
                0,
                &mut sfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
        unsafe {
            CoTaskMemFree(pidl as _);
        }
        if result == 0 || sfi.hIcon.is_null() {
            None
        } else {
            Some(sfi.hIcon as isize)
        }
    }

    fn clear_icon_cache(state: &mut OverlayShellState) {
        let cleared_entries = state.icon_cache.len();
        for handle in state.icon_cache.values() {
            if *handle != 0 {
                unsafe {
                    DestroyIcon(*handle as _);
                }
            }
        }
        state.icon_cache.clear();
        state.icon_cache_lru.clear();
        log_icon_cache_metrics(state, "cache_clear", cleared_entries);
    }

    fn touch_icon_cache_key(state: &mut OverlayShellState, key: &str) {
        if let Some(index) = state.icon_cache_lru.iter().position(|k| k == key) {
            state.icon_cache_lru.remove(index);
        }
        state.icon_cache_lru.push_back(key.to_string());
    }

    fn insert_icon_cache_entry(state: &mut OverlayShellState, key: String, handle: isize) {
        if let Some(previous) = state.icon_cache.insert(key.clone(), handle) {
            if previous != 0 {
                unsafe {
                    DestroyIcon(previous as _);
                }
            }
        }
        touch_icon_cache_key(state, &key);
        while state.icon_cache.len() > runtime_icon_cache_max_entries() {
            let Some(oldest_key) = state.icon_cache_lru.pop_front() else {
                break;
            };
            if oldest_key == key {
                continue;
            }
            if let Some(oldest_handle) = state.icon_cache.remove(&oldest_key) {
                state.icon_cache_metrics.evictions =
                    state.icon_cache_metrics.evictions.saturating_add(1);
                if oldest_handle != 0 {
                    unsafe {
                        DestroyIcon(oldest_handle as _);
                    }
                }
            }
        }
    }

    fn log_icon_cache_metrics(state: &mut OverlayShellState, reason: &str, cleared_entries: usize) {
        let metrics = state.icon_cache_metrics;
        if metrics.hits == 0
            && metrics.misses == 0
            && metrics.load_failures == 0
            && metrics.evictions == 0
            && cleared_entries == 0
        {
            return;
        }
        crate::logging::info(&format!(
            "[swiftfind-core] overlay_icon_cache reason={} hits={} misses={} load_failures={} evictions={} cleared_entries={}",
            reason,
            metrics.hits,
            metrics.misses,
            metrics.load_failures,
            metrics.evictions,
            cleared_entries
        ));
        state.icon_cache_metrics = IconCacheMetrics::default();
    }

    fn log_memory_snapshot(reason: &str) {
        let process = unsafe { GetCurrentProcess() };
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetProcessMemoryInfo(
                process,
                &mut counters as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        };
        if ok == 0 {
            return;
        }

        let mb_divisor = 1024.0_f64 * 1024.0_f64;
        let working_set_mb = (counters.WorkingSetSize as f64) / mb_divisor;
        let private_mb = (counters.PagefileUsage as f64) / mb_divisor;
        crate::logging::info(&format!(
            "[swiftfind-core] memory_snapshot reason={} working_set_mb={:.1} private_mb={:.1}",
            reason, working_set_mb, private_mb
        ));
    }

    fn configure_runtime_performance_tuning(idle_cache_trim_ms: u32, active_memory_target_mb: u16) {
        let idle_ms = idle_cache_trim_ms.clamp(250, 120_000);
        ICON_CACHE_IDLE_MS_RUNTIME.store(idle_ms, Ordering::Relaxed);

        // Keep icon-cache size proportional to active-memory target with a tighter cap so
        // active working set stays stable on large result sets.
        let max_entries = ((active_memory_target_mb as usize).saturating_mul(5) / 4).clamp(32, 256);
        ICON_CACHE_MAX_ENTRIES_RUNTIME.store(max_entries, Ordering::Relaxed);
        crate::logging::info(&format!(
            "[swiftfind-core] overlay_tuning idle_cache_trim_ms={} active_memory_target_mb={} icon_cache_max_entries={}",
            idle_ms, active_memory_target_mb, max_entries
        ));
    }

    fn runtime_icon_cache_idle_ms() -> u32 {
        ICON_CACHE_IDLE_MS_RUNTIME
            .load(Ordering::Relaxed)
            .clamp(250, 120_000)
    }

    fn runtime_icon_cache_max_entries() -> usize {
        ICON_CACHE_MAX_ENTRIES_RUNTIME
            .load(Ordering::Relaxed)
            .clamp(32, 256)
    }

    fn schedule_icon_cache_idle_cleanup(hwnd: HWND) {
        unsafe {
            KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
            SetTimer(
                hwnd,
                TIMER_ICON_CACHE_IDLE,
                runtime_icon_cache_idle_ms(),
                None,
            );
        }
    }

    fn cancel_icon_cache_idle_cleanup(hwnd: HWND) {
        unsafe {
            KillTimer(hwnd, TIMER_ICON_CACHE_IDLE);
        }
    }

    fn target_top_index_for_selection(
        list_hwnd: HWND,
        selected_index: i32,
        count: i32,
        current_top: i32,
    ) -> i32 {
        let visible_rows = visible_row_capacity(list_hwnd);
        let mut target_top = current_top;

        if selected_index < current_top {
            target_top = selected_index;
        } else if selected_index >= current_top + visible_rows {
            target_top = selected_index - visible_rows + 1;
        }

        let max_top = (count - visible_rows).max(0);
        target_top.clamp(0, max_top)
    }

    fn visible_row_capacity(list_hwnd: HWND) -> i32 {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(list_hwnd, &mut rect);
        }
        let height = (rect.bottom - rect.top).max(0);
        let rows = height / ROW_HEIGHT;
        rows.max(1)
    }

    fn start_window_animation(
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
        let Some(state) = state_for(hwnd) else {
            return;
        };

        state.window_anim = Some(WindowAnimation {
            start: Instant::now(),
            duration_ms: duration_ms.max(1),
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

    fn window_animation_tick(hwnd: HWND, state: &mut OverlayShellState) -> bool {
        let Some(anim) = state.window_anim.as_ref() else {
            return false;
        };

        let elapsed_ms = anim.start.elapsed().as_millis() as u32;
        let t = (elapsed_ms as f32 / anim.duration_ms as f32).clamp(0.0, 1.0);
        let eased = ease_out(t);

        let left = lerp_i32(anim.from_left, anim.to_left, eased);
        let top = lerp_i32(anim.from_top, anim.to_top, eased);
        let width = lerp_i32(anim.from_width, anim.to_width, eased);
        let height = lerp_i32(anim.from_height, anim.to_height, eased);
        let alpha = lerp_i32(anim.from_alpha as i32, anim.to_alpha as i32, eased) as u8;
        apply_window_state(hwnd, left, top, width, height, alpha);

        if t >= 1.0 {
            let hide_on_complete = anim.hide_on_complete;
            state.window_anim = None;
            if hide_on_complete {
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                    SetLayeredWindowAttributes(hwnd, 0, OVERLAY_ALPHA_OPAQUE, LWA_ALPHA);
                }
                state.pending_wheel_delta = 0;
                state.suppress_next_hover_sync = false;
            } else if !state.results_visible {
                unsafe {
                    ShowWindow(state.list_hwnd, SW_HIDE);
                    ShowWindow(state.mode_strip_hwnd, SW_HIDE);
                    SendMessageW(state.list_hwnd, LB_RESETCONTENT, 0, 0);
                }
                state.rows.clear();
                state.hover_index = -1;
                state.pending_wheel_delta = 0;
                state.suppress_next_hover_sync = false;
            } else {
                flush_pending_wheel_after_animation(state);
            }
            return false;
        }

        true
    }

    fn flush_pending_wheel_after_animation(state: &mut OverlayShellState) {
        if state.pending_wheel_delta == 0 {
            return;
        }
        let pending = state.pending_wheel_delta;
        state.pending_wheel_delta = 0;
        scroll_list_by_wheel_delta(state, pending);
    }

    fn results_content_animation_tick(hwnd: HWND, state: &mut OverlayShellState) -> bool {
        let progress = results_content_progress(state);
        unsafe {
            InvalidateRect(state.list_hwnd, std::ptr::null(), 0);
        }
        if progress >= 1.0 {
            state.results_content_anim_start = None;
            layout_children(hwnd, state);
            unsafe {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            return false;
        }
        true
    }

    fn results_content_progress(state: &OverlayShellState) -> f32 {
        let Some(start) = state.results_content_anim_start.as_ref() else {
            return 1.0;
        };
        let elapsed_ms = start.elapsed().as_millis() as u32;
        let t = (elapsed_ms as f32 / RESULTS_CONTENT_FADE_MS as f32).clamp(0.0, 1.0);
        ease_out(t)
    }

    fn apply_window_state(hwnd: HWND, left: i32, top: i32, width: i32, height: i32, alpha: u8) {
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                left,
                top,
                width.max(1),
                height.max(1),
                SWP_NOACTIVATE,
            );
            // Keep clipping region synced with animated size to prevent transient sharp corners.
            apply_rounded_corners_hwnd(hwnd);
            SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
        }
    }

    fn hide_overlay_immediate(hwnd: HWND) {
        if let Some(state) = state_for(hwnd) {
            state.help_tip_visible = false;
            state.help_hovered = false;
            state.suppress_next_hover_sync = false;
            state.results_content_anim_start = None;
            state.command_mode_input = false;
            state.command_uninstall_quick_mode = false;
            state.command_badge_anim_start = None;
            state.placeholder_hint.clear();
            unsafe {
                apply_edit_text_rect(
                    state.edit_hwnd,
                    state.command_mode_input,
                    state.command_uninstall_quick_mode,
                );
                ShowWindow(state.help_tip_hwnd, SW_HIDE);
                InvalidateRect(state.edit_hwnd, std::ptr::null(), 1);
            }
        }
        unsafe {
            KillTimer(hwnd, TIMER_WINDOW_ANIM);
            KillTimer(hwnd, TIMER_HELP_HOVER);
            KillTimer(hwnd, TIMER_RESULTS_CONTENT_FADE);
            KillTimer(hwnd, TIMER_COMMAND_BADGE_FADE);
            SetLayeredWindowAttributes(hwnd, 0, OVERLAY_ALPHA_OPAQUE, LWA_ALPHA);
            ShowWindow(hwnd, SW_HIDE);
        }
        log_memory_snapshot("overlay_hide");
        schedule_icon_cache_idle_cleanup(hwnd);
    }

    fn layout_children(hwnd: HWND, state: &mut OverlayShellState) {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(hwnd, &mut rect);
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }

        let input_width = width - PANEL_MARGIN_X * 2;
        let no_results_inline =
            state.no_results_mode && !state.results_visible && !state.status_is_error;
        let help_reserved = if no_results_inline {
            NO_RESULTS_INLINE_WIDTH + HELP_ICON_GAP_FROM_INPUT
        } else {
            HELP_ICON_SIZE + HELP_ICON_RIGHT_INSET + HELP_ICON_GAP_FROM_INPUT
        };
        let edit_width = (input_width - help_reserved).max(120);
        let status_len = unsafe { GetWindowTextLengthW(state.status_hwnd) };
        let status_visible = status_len > 0;
        let footer_status_mode = state.results_visible && status_visible && !no_results_inline;
        let footer_hint_mode = state.results_visible
            && state.results_content_anim_start.is_none()
            && !footer_status_mode
            && !no_results_inline;
        let mode_strip_visible = false;
        // Keep input exactly centered in compact mode and stable across states.
        let input_top = INPUT_TOP.max(0);
        let status_top = if footer_status_mode {
            (height - PANEL_MARGIN_X - STATUS_HEIGHT).max(COMPACT_HEIGHT + 2)
        } else if no_results_inline {
            input_top + ((INPUT_HEIGHT - STATUS_HEIGHT).max(0) / 2)
        } else {
            COMPACT_HEIGHT - PANEL_MARGIN_BOTTOM - STATUS_HEIGHT
        };
        let status_height = STATUS_HEIGHT;

        let mode_strip_top = COMPACT_HEIGHT + DIVIDER_TOP_SPACING + 1;
        let list_top = COMPACT_HEIGHT + INPUT_TO_LIST_GAP;
        let list_left = PANEL_MARGIN_X + 1;
        let list_width = (input_width - 2).max(0);
        let list_bottom_reserved = if footer_status_mode {
            PANEL_MARGIN_X + STATUS_HEIGHT + 3
        } else if footer_hint_mode {
            PANEL_MARGIN_X + FOOTER_HINT_HEIGHT + 4
        } else {
            PANEL_MARGIN_X + 1
        };
        let list_height = (height - list_top - list_bottom_reserved).max(0);
        let help_left = PANEL_MARGIN_X + edit_width + HELP_ICON_GAP_FROM_INPUT;
        let help_top = input_top + (INPUT_HEIGHT - HELP_ICON_SIZE) / 2;
        let footer_hint_top = (height - PANEL_MARGIN_X - FOOTER_HINT_HEIGHT).max(list_top);

        unsafe {
            MoveWindow(
                state.edit_hwnd,
                PANEL_MARGIN_X,
                input_top,
                edit_width,
                INPUT_HEIGHT,
                1,
            );
            apply_edit_text_rect(
                state.edit_hwnd,
                state.command_mode_input,
                state.command_uninstall_quick_mode,
            );
            if status_visible {
                update_status_alignment(state, no_results_inline);
                let (status_left, status_width) = if no_results_inline {
                    (
                        PANEL_MARGIN_X + edit_width + HELP_ICON_GAP_FROM_INPUT,
                        NO_RESULTS_INLINE_WIDTH,
                    )
                } else {
                    (PANEL_MARGIN_X, input_width)
                };
                ShowWindow(state.status_hwnd, SW_SHOW);
                MoveWindow(
                    state.status_hwnd,
                    status_left,
                    status_top,
                    status_width,
                    status_height,
                    1,
                );
                if no_results_inline && state.no_results_anim_pending {
                    let _ = AnimateWindow(
                        state.status_hwnd,
                        NO_RESULTS_FADE_MS,
                        AW_BLEND | AW_ACTIVATE,
                    );
                    state.no_results_anim_pending = false;
                }
            } else {
                ShowWindow(state.status_hwnd, SW_HIDE);
                update_status_alignment(state, false);
            }
            if no_results_inline {
                state.help_hovered = false;
                state.help_tip_visible = false;
                ShowWindow(state.help_hwnd, SW_HIDE);
            } else {
                MoveWindow(
                    state.help_hwnd,
                    help_left,
                    help_top,
                    HELP_ICON_SIZE,
                    HELP_ICON_SIZE,
                    1,
                );
                ShowWindow(state.help_hwnd, SW_SHOW);
            }
            if footer_hint_mode {
                MoveWindow(
                    state.footer_hint_hwnd,
                    0,
                    footer_hint_top,
                    width,
                    FOOTER_HINT_HEIGHT,
                    1,
                );
                ShowWindow(state.footer_hint_hwnd, SW_SHOW);
            } else {
                ShowWindow(state.footer_hint_hwnd, SW_HIDE);
            }
            if mode_strip_visible {
                let wide = to_wide(&state.mode_strip_text);
                SetWindowTextW(state.mode_strip_hwnd, wide.as_ptr());
                MoveWindow(
                    state.mode_strip_hwnd,
                    PANEL_MARGIN_X,
                    mode_strip_top,
                    input_width,
                    MODE_STRIP_HEIGHT,
                    1,
                );
                ShowWindow(state.mode_strip_hwnd, SW_SHOW);
            } else {
                ShowWindow(state.mode_strip_hwnd, SW_HIDE);
            }
            position_help_tip_popup(state);
            apply_help_tip_rounded_corners(
                state.help_tip_hwnd,
                help_tip_width_for_text(state),
                HELP_TIP_HEIGHT,
            );
            if state.help_tip_visible {
                ShowWindow(state.help_tip_hwnd, SW_SHOW);
            } else {
                ShowWindow(state.help_tip_hwnd, SW_HIDE);
            }
            MoveWindow(
                state.list_hwnd,
                list_left,
                list_top,
                list_width,
                list_height,
                1,
            );
            apply_list_rounded_corners(state.list_hwnd, list_width, list_height);
        }
    }

    fn apply_edit_text_rect(
        edit_hwnd: HWND,
        command_mode_input: bool,
        command_uninstall_quick_mode: bool,
    ) {
        let mut client: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(edit_hwnd, &mut client);
        }
        let width = (client.right - client.left).max(0);
        let height = (client.bottom - client.top).max(0);
        if width <= 0 || height <= 0 {
            return;
        }

        let line_height = input_line_height_for_edit(edit_hwnd, 0);
        let text_rect = compute_input_text_rect(
            width,
            height,
            line_height,
            command_mode_input,
            command_uninstall_quick_mode,
        );

        unsafe {
            SendMessageW(
                edit_hwnd,
                EM_SETRECTNP,
                0,
                (&text_rect as *const RECT) as LPARAM,
            );
            InvalidateRect(edit_hwnd, std::ptr::null(), 1);
        }
    }

    fn update_status_alignment(state: &mut OverlayShellState, centered: bool) {
        if state.status_hwnd.is_null() || state.status_center_aligned == centered {
            return;
        }

        unsafe {
            let style = GetWindowLongPtrW(state.status_hwnd, GWL_STYLE) as u32;
            let mut updated = style & !(STATIC_CENTER_STYLE | STATIC_RIGHT_STYLE);
            updated |= if centered {
                STATIC_CENTER_STYLE
            } else {
                STATIC_RIGHT_STYLE
            };
            SetWindowLongPtrW(state.status_hwnd, GWL_STYLE, updated as isize);
            InvalidateRect(state.status_hwnd, std::ptr::null(), 1);
        }
        state.status_center_aligned = centered;
    }

    fn compute_input_text_rect(
        width: i32,
        height: i32,
        line_height: i32,
        command_mode_input: bool,
        command_uninstall_quick_mode: bool,
    ) -> RECT {
        let line_height = line_height.clamp(14, (height - 2).max(14));
        let centered_top = ((height - line_height) / 2).max(0) + INPUT_TEXT_SHIFT_Y;
        let max_top = (height - line_height).max(0);
        let top = centered_top.clamp(0, max_top);
        let prefix_left_pad = if command_mode_input {
            COMMAND_PREFIX_INPUT_PAD
        } else {
            0
        };
        let quick_badge_left_pad = if command_mode_input && command_uninstall_quick_mode {
            COMMAND_BADGE_INPUT_PAD
        } else {
            0
        };
        let mut text_rect = RECT {
            left: INPUT_TEXT_LEFT_INSET
                + INPUT_TEXT_SHIFT_X
                + prefix_left_pad
                + quick_badge_left_pad,
            top,
            right: width - INPUT_TEXT_RIGHT_INSET + INPUT_TEXT_SHIFT_X,
            bottom: top + line_height,
        };
        if text_rect.right <= text_rect.left {
            text_rect.right = width;
        }
        if text_rect.bottom <= text_rect.top {
            text_rect.top = 0;
            text_rect.bottom = height;
        }
        text_rect
    }

    fn input_line_height_for_edit(edit_hwnd: HWND, fallback_font: isize) -> i32 {
        let hdc = unsafe { GetDC(edit_hwnd) };
        if hdc.is_null() {
            return INPUT_TEXT_LINE_HEIGHT_FALLBACK;
        }

        let font_to_use = if fallback_font != 0 {
            fallback_font
        } else if let Some(state) = state_for(unsafe { GetParent(edit_hwnd) }) {
            state.input_font
        } else {
            0
        };

        let old_font = if font_to_use != 0 {
            unsafe { SelectObject(hdc, font_to_use as _) }
        } else {
            std::ptr::null_mut()
        };

        let mut tm: TEXTMETRICW = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetTextMetricsW(hdc, &mut tm) };

        if !old_font.is_null() {
            unsafe {
                SelectObject(hdc, old_font);
            }
        }
        unsafe {
            ReleaseDC(edit_hwnd, hdc);
        }

        if ok == 0 {
            INPUT_TEXT_LINE_HEIGHT_FALLBACK
        } else {
            tm.tmHeight as i32
        }
    }

    fn apply_list_rounded_corners(list_hwnd: HWND, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        unsafe {
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, LIST_RADIUS, LIST_RADIUS);
            SetWindowRgn(list_hwnd, region, 1);
        }
    }

    fn apply_help_tip_rounded_corners(help_tip_hwnd: HWND, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        unsafe {
            let region = CreateRoundRectRgn(
                0,
                0,
                width + 1,
                height + 1,
                HELP_TIP_RADIUS,
                HELP_TIP_RADIUS,
            );
            SetWindowRgn(help_tip_hwnd, region, 1);
        }
    }

    fn position_help_tip_popup(state: &OverlayShellState) {
        let mut help_rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(state.help_hwnd, &mut help_rect);
        }
        if help_rect.right <= help_rect.left || help_rect.bottom <= help_rect.top {
            return;
        }

        let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let tip_width = help_tip_width_for_text(state);

        // Anchor to the help icon: starts above "?" and may extend outside the panel.
        let mut tip_left = help_rect.left - HELP_TIP_TEXT_PAD_X;
        let mut tip_top = help_rect.top - HELP_TIP_HEIGHT - 8;
        if tip_top < 8 {
            tip_top = help_rect.bottom + 8;
        }

        let max_left = (screen_w - tip_width - 8).max(8);
        let max_top = (screen_h - HELP_TIP_HEIGHT - 8).max(8);
        tip_left = tip_left.clamp(8, max_left);
        tip_top = tip_top.clamp(8, max_top);

        unsafe {
            MoveWindow(
                state.help_tip_hwnd,
                tip_left,
                tip_top,
                tip_width,
                HELP_TIP_HEIGHT,
                1,
            );
        }
    }

    fn help_tip_width_for_text(state: &OverlayShellState) -> i32 {
        let text = help_hint_text(state);
        if text.is_empty() {
            return HELP_TIP_WIDTH;
        }

        let hdc = unsafe { GetDC(state.help_tip_hwnd) };
        if hdc.is_null() {
            return HELP_TIP_WIDTH;
        }

        let width = unsafe {
            let old_font = if state.help_tip_font != 0 {
                SelectObject(hdc, state.help_tip_font as _)
            } else {
                std::ptr::null_mut()
            };
            let wide = to_wide_no_nul(&text);
            let mut size: SIZE = std::mem::zeroed();
            let ok = GetTextExtentPoint32W(hdc, wide.as_ptr(), wide.len() as i32, &mut size);
            if !old_font.is_null() {
                SelectObject(hdc, old_font);
            }
            ReleaseDC(state.help_tip_hwnd, hdc);
            if ok == 0 {
                HELP_TIP_WIDTH
            } else {
                size.cx + HELP_TIP_TEXT_PAD_X * 2
            }
        };

        width.clamp(92, 260)
    }

    fn paint_help_tip(hwnd: HWND, state: &OverlayShellState) {
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

    fn paint_footer_hint(hwnd: HWND, state: &OverlayShellState) {
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

            let separator_color = state.palette.panel_border;
            let separator_brush = CreateSolidBrush(separator_color);
            let separator_rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: FOOTER_SEPARATOR_HEIGHT,
            };
            FillRect(hdc, &separator_rect, separator_brush as _);
            DeleteObject(separator_brush as _);

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

            let content_top =
                (FOOTER_SEPARATOR_HEIGHT + FOOTER_SEPARATOR_TO_CONTENT_GAP).min(height);
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
        (measure_text_width(hdc, text).max(1) + FOOTER_KEYCAP_PAD_X * 2)
            .max(FOOTER_KEYCAP_MIN_WIDTH)
    }

    fn draw_footer_keycap_right(
        hdc: HDC,
        state: &OverlayShellState,
        right: i32,
        content_top: i32,
        content_bottom: i32,
        text: &str,
    ) -> i32 {
        let cap_width = footer_keycap_width(hdc, text);
        let left = (right - cap_width).max(0);
        let content_height = (content_bottom - content_top).max(1);
        let top = content_top + ((content_height - FOOTER_KEYCAP_HEIGHT).max(0) / 2);
        let bottom = (top + FOOTER_KEYCAP_HEIGHT).min(content_bottom);

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
            let fill_color = blend_color(
                state.palette.results_bg,
                state.palette.selection_accent,
                0.78,
            );
            let border_color =
                blend_color(state.palette.results_bg, state.palette.panel_border, 0.88);
            let text_color =
                blend_color(state.palette.results_bg, state.palette.text_primary, 0.94);
            let fill_brush = CreateSolidBrush(fill_color);
            let border_pen = CreatePen(PS_SOLID, 1, border_color);
            let old_brush = SelectObject(hdc, fill_brush as _);
            let old_pen = SelectObject(hdc, border_pen as _);
            RoundRect(
                hdc,
                left,
                top,
                right + 1,
                bottom + 1,
                FOOTER_KEYCAP_RADIUS,
                FOOTER_KEYCAP_RADIUS,
            );
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(border_pen as _);
            DeleteObject(fill_brush as _);

            SetTextColor(hdc, text_color);
            let text_wide = to_wide_no_nul(text);
            let mut text_rect = RECT {
                left,
                top: top + FOOTER_KEYCAP_TEXT_NUDGE_Y,
                right: right + 1,
                bottom: bottom + 1 + FOOTER_KEYCAP_TEXT_NUDGE_Y,
            };
            DrawTextW(
                hdc,
                text_wide.as_ptr(),
                text_wide.len() as i32,
                &mut text_rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
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

    fn invalidate_list_row(list_hwnd: HWND, row: i32) {
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

    fn sync_help_hover_with_cursor(hwnd: HWND, state: &mut OverlayShellState) {
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
                position_help_tip_popup(state);
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

    fn help_hint_text(state: &OverlayShellState) -> String {
        if state.help_config_path.trim().is_empty() {
            HOTKEY_HELP_TEXT_FALLBACK.to_string()
        } else {
            "Click to edit hotkey".to_string()
        }
    }

    fn open_help_config_file(state: &mut OverlayShellState) -> Result<(), String> {
        let cfg_path = state.help_config_path.trim().to_string();
        let target = if cfg_path.is_empty() {
            if let Ok(appdata) = std::env::var("APPDATA") {
                format!("{appdata}\\SwiftFind\\config.toml")
            } else {
                return Err("APPDATA is not set; cannot locate config path.".to_string());
            }
        } else {
            cfg_path
        };

        let path = std::path::Path::new(&target);
        open_config_file_with_default_app(path)?;
        state.status_is_error = false;
        state.help_tip_visible = false;
        let wide = to_wide("Opened config file. Restart SwiftFind after changes.");
        unsafe {
            SetWindowTextW(state.status_hwnd, wide.as_ptr());
            InvalidateRect(state.status_hwnd, std::ptr::null(), 1);
        }
        Ok(())
    }

    fn open_config_file_with_default_app(path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create config directory: {e}"))?;
        }
        if !path.exists() {
            let cfg = crate::config::load(Some(path))
                .map_err(|e| format!("failed to load default config template: {e}"))?;
            crate::config::write_user_template(&cfg, path)
                .map_err(|e| format!("failed to create config file: {e}"))?;
        }

        let launch_target = path.to_string_lossy().into_owned();
        crate::action_executor::launch_path(&launch_target)
            .map_err(|e| format!("failed to open config file: {e}"))?;
        Ok(())
    }

    fn draw_panel_background(hwnd: HWND) {
        let Some(state) = state_for(hwnd) else {
            return;
        };

        let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        unsafe {
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut client_rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client_rect);
            let width = client_rect.right - client_rect.left;
            let height = client_rect.bottom - client_rect.top;
            if width > 0 && height > 0 {
                if state.dwm_rounded_enabled {
                    // In DWM mode, let DWM draw the rounded border (anti-aliased).
                    // We only fill panel background to avoid jagged inner rounded edges.
                    FillRect(hdc, &client_rect, state.panel_brush as _);
                } else {
                    // Paint border as an outer rounded fill, then paint panel fill as inner rounded fill.
                    // This avoids the angular look that FrameRgn can produce at tight corner radii.
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

        // Draw a full divider stroke (matching panel border color) between input and results.
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

    fn apply_rounded_corners_hwnd(hwnd: HWND) {
        if let Some(state) = state_for(hwnd) {
            if state.dwm_rounded_enabled {
                // Let DWM own the main window corner clipping when available.
                // Avoid SetWindowRgn here to prevent aliasing on curve edges.
                return;
            }
        }

        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(hwnd, &mut rect);
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }

        unsafe {
            let region =
                CreateRoundRectRgn(0, 0, width + 1, height + 1, PANEL_RADIUS, PANEL_RADIUS);
            SetWindowRgn(hwnd, region, 1);
        }
    }

    fn try_enable_dwm_rounded_corners(hwnd: HWND) -> bool {
        let corner_pref = DWMWCP_ROUND;
        let hr_corner = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &corner_pref as *const _ as *const c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };
        // Use DWM border in rounded mode for cleaner anti-aliased edge.
        let border_color: u32 = state_for(hwnd)
            .map(|state| state.palette.panel_border)
            .unwrap_or(PALETTE_DARK.panel_border);
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                &border_color as *const _ as *const c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        if hr_corner >= 0 {
            crate::logging::info("[swiftfind-core] overlay_corners mode=dwm_round");
            true
        } else {
            false
        }
    }

    fn cleanup_state_resources(state: &mut OverlayShellState) {
        unsafe {
            if state.input_font != 0 {
                DeleteObject(state.input_font as _);
            }
            if state.title_font != 0 {
                DeleteObject(state.title_font as _);
            }
            if state.meta_font != 0 {
                DeleteObject(state.meta_font as _);
            }
            if state.status_font != 0 {
                DeleteObject(state.status_font as _);
            }
            if state.header_font != 0 {
                DeleteObject(state.header_font as _);
            }
            if state.top_hit_font != 0 {
                DeleteObject(state.top_hit_font as _);
            }
            if state.hint_font != 0 {
                DeleteObject(state.hint_font as _);
            }
            if state.help_tip_font != 0 {
                DeleteObject(state.help_tip_font as _);
            }
            if state.help_icon_font != 0 {
                DeleteObject(state.help_icon_font as _);
            }
            if state.footer_font != 0 {
                DeleteObject(state.footer_font as _);
            }
            if state.command_prefix_font != 0 {
                DeleteObject(state.command_prefix_font as _);
            }
            if state.command_badge_font != 0 {
                DeleteObject(state.command_badge_font as _);
            }
            if state.command_icon_font != 0 {
                DeleteObject(state.command_icon_font as _);
            }
            if state.command_icon_fallback_font != 0 {
                DeleteObject(state.command_icon_fallback_font as _);
            }
            if state.panel_brush != 0 {
                DeleteObject(state.panel_brush as _);
            }
            if state.border_brush != 0 {
                DeleteObject(state.border_brush as _);
            }
            if state.input_brush != 0 {
                DeleteObject(state.input_brush as _);
            }
            if state.results_brush != 0 {
                DeleteObject(state.results_brush as _);
            }
            if state.selection_brush != 0 {
                DeleteObject(state.selection_brush as _);
            }
            if state.selection_border_brush != 0 {
                DeleteObject(state.selection_border_brush as _);
            }
            if state.row_hover_brush != 0 {
                DeleteObject(state.row_hover_brush as _);
            }
            if state.row_separator_brush != 0 {
                DeleteObject(state.row_separator_brush as _);
            }
            if state.selection_accent_brush != 0 {
                DeleteObject(state.selection_accent_brush as _);
            }
            if state.icon_brush != 0 {
                DeleteObject(state.icon_brush as _);
            }
            if state.help_tip_brush != 0 {
                DeleteObject(state.help_tip_brush as _);
            }
            if state.help_tip_border_brush != 0 {
                DeleteObject(state.help_tip_border_brush as _);
            }
        }
        clear_icon_cache(state);
    }

    fn state_for(hwnd: HWND) -> Option<&'static mut OverlayShellState> {
        let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayShellState };
        if state_ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *state_ptr })
        }
    }

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
                    std::env::var("SWIFTFIND_FONT_FAMILY").ok().as_deref(),
                    register_private_geist_fonts(),
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

    fn resolve_font_family(font_env: Option<&str>, geist_loaded: bool) -> String {
        if let Some(value) = font_env.map(|v| v.trim()).filter(|v| !v.is_empty()) {
            return value.to_string();
        }
        if geist_loaded {
            GEIST_FONT_FAMILY.to_string()
        } else {
            DEFAULT_FONT_FAMILY.to_string()
        }
    }

    fn register_private_geist_fonts() -> bool {
        static REGISTERED: OnceLock<bool> = OnceLock::new();
        *REGISTERED.get_or_init(|| {
            let mut candidates = Vec::new();
            if let Ok(dir) = std::env::var("SWIFTFIND_FONT_DIR") {
                let trimmed = dir.trim();
                if !trimmed.is_empty() {
                    candidates.push(PathBuf::from(trimmed));
                }
            }
            if let Ok(cwd) = std::env::current_dir() {
                candidates.push(cwd.join("apps/assets/fonts/Geist/otf"));
                candidates.push(cwd.join("fonts/Geist/otf"));
                candidates.push(cwd.join("assets/fonts/Geist/otf"));
            }
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    candidates.push(exe_dir.join("..").join("assets/fonts/Geist/otf"));
                    candidates.push(exe_dir.join("assets/fonts/Geist/otf"));
                }
            }

            let files = [
                "Geist-Regular.otf",
                "Geist-Medium.otf",
                "Geist-SemiBold.otf",
                "Geist-Bold.otf",
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
                    let added = unsafe {
                        AddFontResourceExW(font_wide.as_ptr(), FR_PRIVATE, std::ptr::null())
                    };
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

    fn path_to_wide(path: &Path) -> Vec<u16> {
        path.to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    fn create_font(height: i32, weight: i32) -> isize {
        create_font_with_family_quality(
            height,
            weight,
            font_family_wide(),
            CLEARTYPE_QUALITY as u32,
        )
    }

    fn create_font_with_family(height: i32, weight: i32, family_wide: &[u16]) -> isize {
        create_font_with_family_quality(height, weight, family_wide, ANTIALIASED_QUALITY as u32)
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

    fn lerp_i32(from: i32, to: i32, t: f32) -> i32 {
        (from as f32 + (to - from) as f32 * t).round() as i32
    }

    fn blend_color(from: u32, to: u32, t: f32) -> u32 {
        let t = t.clamp(0.0, 1.0);
        let fr = (from & 0xFF) as f32;
        let fg = ((from >> 8) & 0xFF) as f32;
        let fb = ((from >> 16) & 0xFF) as f32;

        let tr = (to & 0xFF) as f32;
        let tg = ((to >> 8) & 0xFF) as f32;
        let tb = ((to >> 16) & 0xFF) as f32;

        let r = (fr + (tr - fr) * t).round() as u32;
        let g = (fg + (tg - fg) * t).round() as u32;
        let b = (fb + (tb - fb) * t).round() as u32;

        (r & 0xFF) | ((g & 0xFF) << 8) | ((b & 0xFF) << 16)
    }

    fn ease_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t) * (1.0 - t)
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn to_wide_no_nul(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    fn wide_buf_to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|ch| *ch == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    fn pwstr_to_string_and_free(ptr: windows_sys::core::PWSTR) -> String {
        if ptr.is_null() {
            return String::new();
        }

        let mut len = 0usize;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let out = String::from_utf16_lossy(slice);
            CoTaskMemFree(ptr as _);
            out
        }
    }

    fn split_icon_resource_spec(raw: &str) -> (&str, Option<i32>) {
        let trimmed = raw.trim();
        let Some(idx) = trimmed.rfind(',') else {
            return (trimmed, None);
        };
        let (path_part, index_part) = trimmed.split_at(idx);
        let parsed = index_part[1..].trim().parse::<i32>().ok();
        if parsed.is_some() {
            (path_part.trim(), parsed)
        } else {
            (trimmed, None)
        }
    }

    fn normalize_icon_source_path(raw: &str) -> String {
        let mut s = raw
            .trim()
            .trim_matches('"')
            .trim_start_matches('@')
            .trim()
            .to_string();
        if s.is_empty() {
            return String::new();
        }
        s = expand_environment_variables(&s);

        // Strip trailing arguments from command-like targets by clipping to known icon-bearing extensions.
        let lower = s.to_ascii_lowercase();
        for ext in [".exe", ".ico", ".dll"] {
            if let Some(pos) = lower.find(ext) {
                let end = pos + ext.len();
                if end <= s.len() {
                    s.truncate(end);
                    return s;
                }
            }
        }

        s
    }

    fn expand_environment_variables(raw: &str) -> String {
        if !raw.contains('%') {
            return raw.to_string();
        }
        let input = to_wide(raw);
        let required =
            unsafe { ExpandEnvironmentStringsW(input.as_ptr(), std::ptr::null_mut(), 0) };
        if required <= 1 {
            return raw.to_string();
        }
        let mut out = vec![0u16; required as usize];
        let written =
            unsafe { ExpandEnvironmentStringsW(input.as_ptr(), out.as_mut_ptr(), required) };
        if written <= 1 {
            return raw.to_string();
        }
        wide_buf_to_string(&out)
    }

    #[cfg(test)]
    mod tests {
        use super::{resolve_font_family, DEFAULT_FONT_FAMILY, GEIST_FONT_FAMILY};

        #[test]
        fn uses_explicit_font_env_when_provided() {
            let resolved = resolve_font_family(Some("Segoe UI"), true);
            assert_eq!(resolved, "Segoe UI");
        }

        #[test]
        fn uses_geist_when_loaded_without_env_override() {
            let resolved = resolve_font_family(None, true);
            assert_eq!(resolved, GEIST_FONT_FAMILY);
        }

        #[test]
        fn falls_back_to_default_when_geist_not_loaded() {
            let resolved = resolve_font_family(None, false);
            assert_eq!(resolved, DEFAULT_FONT_FAMILY);
        }
    }

    pub fn is_instance_window_present() -> bool {
        let hwnd = unsafe { FindWindowW(class_name_wide().as_ptr(), std::ptr::null()) };
        !hwnd.is_null()
    }

    pub fn signal_existing_instance_show() -> Result<bool, String> {
        let hwnd = unsafe { FindWindowW(class_name_wide().as_ptr(), std::ptr::null()) };
        if hwnd.is_null() {
            return Ok(false);
        }

        let ok = unsafe { PostMessageW(hwnd, SWIFTFIND_WM_EXTERNAL_SHOW, 0, 0) };
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

        let ok = unsafe { PostMessageW(hwnd, SWIFTFIND_WM_EXTERNAL_QUIT, 0, 0) };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            return Err(format!("PostMessageW(quit) failed with error {error}"));
        }
        Ok(true)
    }
}

#[cfg(target_os = "windows")]
pub use imp::{
    is_instance_window_present, signal_existing_instance_quit, signal_existing_instance_show,
    NativeOverlayShell, OverlayEvent, OverlayRow, OverlayRowRole,
};

#[cfg(not(target_os = "windows"))]
pub fn is_instance_window_present() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn signal_existing_instance_show() -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
pub fn signal_existing_instance_quit() -> Result<bool, String> {
    Ok(false)
}
