use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DT_END_ELLIPSIS,
    DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, KillTimer, LoadCursorW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HTCAPTION,
    HWND_NOTOPMOST, IDC_ARROW, LWA_ALPHA, MSG, SM_CXSCREEN, SM_CYSCREEN, SWP_NOZORDER, SW_SHOW,
    WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};

use crate::windows_overlay::animation::blend_color;
use crate::windows_overlay::gdiplus_rendering::{
    GdiplusContext, GpRectF, SMOOTHING_MODE_ANTI_ALIAS,
};
use crate::windows_overlay::layout::{
    apply_rounded_corners_hwnd, try_enable_dwm_rounded_corners, try_enable_mica,
};
use crate::windows_overlay::types::{
    detect_system_theme, palette_for_theme, to_wide, OverlayPalette, OverlayTheme, WINDOW_WIDTH,
};

const WINDOW_CLASS: &str = "NexIndexingProgress";
const WIDTH: i32 = WINDOW_WIDTH;
const HEIGHT: i32 = 126;
const HMARGIN: i32 = 24;
const TITLE_TOP: i32 = 20;
const TITLE_HEIGHT: i32 = 24;
const SUBTITLE_TOP: i32 = 45;
const SUBTITLE_HEIGHT: i32 = 22;
const PROGRESS_TOP: i32 = 82;
const PROGRESS_HEIGHT: i32 = 10;
const PROGRESS_TIMER_ID: usize = 10;
const PROGRESS_INTERVAL_MS: u32 = 50;
const PROGRESS_MAX: u32 = 100;
const PROGRESS_RADIUS: i32 = 4;
const FONT_TITLE_HEIGHT: i32 = -16;
const FONT_BODY_HEIGHT: i32 = -13;
const FONT_WEIGHT_TITLE: i32 = 600;
const FONT_WEIGHT_BODY: i32 = 400;

struct ProgressState {
    panel_brush: isize,
    title_font: isize,
    body_font: isize,
    palette: OverlayPalette,
    gdiplus: Option<GdiplusContext>,
    shared: Arc<AtomicU32>,
    last_painted: u32,
}

unsafe extern "system" fn progress_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, PROGRESS_TIMER_ID);
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ProgressState;
            if !ptr.is_null() {
                let state = Box::from_raw(ptr);
                if state.panel_brush != 0 {
                    DeleteObject(state.panel_brush as _);
                }
                if state.title_font != 0 {
                    DeleteObject(state.title_font as _);
                }
                if state.body_font != 0 {
                    DeleteObject(state.body_font as _);
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            0
        }
        WM_TIMER if wparam == PROGRESS_TIMER_ID => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ProgressState;
            if !ptr.is_null() {
                let current = (*ptr).shared.load(Ordering::Relaxed).min(PROGRESS_MAX);
                if current != (*ptr).last_painted {
                    (*ptr).last_painted = current;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_NCHITTEST => HTCAPTION as isize,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_progress_window(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint_progress_window(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ProgressState;
    if ptr.is_null() {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut paint);
        if !hdc.is_null() {
            EndPaint(hwnd, &paint);
        }
        return;
    }

    let state = &mut *ptr;
    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let width = (client.right - client.left).max(0);
    let height = (client.bottom - client.top).max(0);

    let mut paint: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut paint);
    if hdc.is_null() {
        return;
    }

    if let Some(ref gdiplus) = state.gdiplus {
        if let Some(graphics) = gdiplus.create_graphics(hdc as isize) {
            GdiplusContext::set_smoothing_mode(graphics, SMOOTHING_MODE_ANTI_ALIAS);

            let panel_bg = GdiplusContext::gdi_color_to_argb(state.palette.panel_bg);
            let panel_border = GdiplusContext::gdi_color_to_argb(state.palette.panel_border);
            gdiplus.fill_rect(graphics, 0, 0, width, height, panel_bg);
            let border = GpRectF {
                x: 0.5,
                y: 0.5,
                width: (width - 1).max(0) as f32,
                height: (height - 1).max(0) as f32,
            };
            gdiplus.draw_rounded_rect_border_on_graphics_f(
                graphics,
                &border,
                22,
                panel_border,
                1.0,
            );

            let track_left = HMARGIN;
            let track_width = (width - (HMARGIN * 2)).max(1);
            let track_color = GdiplusContext::gdi_color_to_argb(state.palette.row_hover);
            let fill_color = GdiplusContext::gdi_color_to_argb(blend_color(
                state.palette.panel_bg,
                state.palette.text_primary,
                0.78,
            ));
            gdiplus.fill_rounded_rect_on_graphics(
                graphics,
                track_left,
                PROGRESS_TOP,
                track_width,
                PROGRESS_HEIGHT,
                PROGRESS_RADIUS,
                track_color,
            );

            let pct = state.shared.load(Ordering::Relaxed).min(PROGRESS_MAX);
            let fill_width =
                ((track_width as u32).saturating_mul(pct) / PROGRESS_MAX).max(1) as i32;
            gdiplus.fill_rounded_rect_on_graphics(
                graphics,
                track_left,
                PROGRESS_TOP,
                fill_width.min(track_width),
                PROGRESS_HEIGHT,
                PROGRESS_RADIUS,
                fill_color,
            );

            GdiplusContext::delete_graphics(graphics);
        }
    }

    SetBkMode(hdc, TRANSPARENT as i32);
    draw_text(
        hdc,
        state.title_font,
        state.palette.text_primary,
        "Indexing Nex",
        RECT {
            left: HMARGIN,
            top: TITLE_TOP,
            right: width - HMARGIN,
            bottom: TITLE_TOP + TITLE_HEIGHT,
        },
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );
    draw_text(
        hdc,
        state.body_font,
        state.palette.text_secondary,
        "Scanning apps and files",
        RECT {
            left: HMARGIN,
            top: SUBTITLE_TOP,
            right: width - HMARGIN - 54,
            bottom: SUBTITLE_TOP + SUBTITLE_HEIGHT,
        },
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
    );

    let pct = state.shared.load(Ordering::Relaxed).min(PROGRESS_MAX);
    draw_text(
        hdc,
        state.body_font,
        state.palette.text_hint,
        &format!("{pct}%"),
        RECT {
            left: width - HMARGIN - 52,
            top: SUBTITLE_TOP,
            right: width - HMARGIN,
            bottom: SUBTITLE_TOP + SUBTITLE_HEIGHT,
        },
        DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
    );

    EndPaint(hwnd, &paint);
}

unsafe fn draw_text(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: isize,
    color: u32,
    text: &str,
    mut rect: RECT,
    format: u32,
) {
    let old_font = if font != 0 {
        SelectObject(hdc, font as _)
    } else {
        std::ptr::null_mut()
    };
    SetTextColor(hdc, color);
    let wide = to_wide(text);
    DrawTextW(hdc, wide.as_ptr(), -1, &mut rect, format);
    if !old_font.is_null() {
        SelectObject(hdc, old_font);
    }
}

unsafe fn center_window(hwnd: HWND) {
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let x = (screen_w - WIDTH) / 2;
    let y = (screen_h - HEIGHT) / 2;
    SetWindowPos(hwnd, HWND_NOTOPMOST, x, y, WIDTH, HEIGHT, SWP_NOZORDER);
}

pub(crate) fn run_with_progress_window<F, T>(work: F) -> T
where
    F: FnOnce(Arc<AtomicU32>) -> T + Send + 'static,
    T: Send + 'static,
{
    let progress = Arc::new(AtomicU32::new(0));
    let progress_clone = progress.clone();

    let hwnd = unsafe {
        create_progress_window(progress).expect("failed to create indexing progress window")
    };
    let hwnd_raw = hwnd as isize;

    unsafe {
        center_window(hwnd);
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        ShowWindow(hwnd, SW_SHOW);
    }

    let (tx, rx) = mpsc::channel::<T>();
    std::thread::spawn(move || {
        let outcome = work(progress_clone);
        let _ = tx.send(outcome);
        unsafe {
            PostMessageW(hwnd_raw as *mut core::ffi::c_void, WM_CLOSE, 0, 0);
        }
    });

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } != 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    rx.recv()
        .expect("indexing thread finished without sending result")
}

unsafe fn create_progress_window(shared: Arc<AtomicU32>) -> Result<HWND, String> {
    let instance = GetModuleHandleW(std::ptr::null());
    let theme = detect_system_theme();
    let palette = palette_for_theme(theme);
    let panel_brush = CreateSolidBrush(palette.panel_bg) as isize;

    let class_name = to_wide(WINDOW_CLASS);
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.style = CS_HREDRAW | CS_VREDRAW;
    wc.lpfnWndProc = Some(progress_wnd_proc);
    wc.hInstance = instance;
    wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
    wc.hbrBackground = panel_brush as _;
    wc.lpszClassName = class_name.as_ptr();
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED,
        class_name.as_ptr(),
        to_wide("Nex Indexing").as_ptr(),
        WS_POPUP | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        WIDTH,
        HEIGHT,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        std::ptr::null_mut(),
    );

    if hwnd.is_null() {
        DeleteObject(panel_brush as _);
        return Err(format!("CreateWindowExW failed: {}", GetLastError()));
    }

    let state = Box::new(ProgressState {
        panel_brush,
        title_font: create_font(FONT_TITLE_HEIGHT, FONT_WEIGHT_TITLE),
        body_font: create_font(FONT_BODY_HEIGHT, FONT_WEIGHT_BODY),
        palette,
        gdiplus: GdiplusContext::new(),
        shared,
        last_painted: 0,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    try_enable_dwm_rounded_corners(hwnd);
    try_enable_mica(hwnd, theme == OverlayTheme::Dark);
    apply_rounded_corners_hwnd(hwnd);
    SetTimer(hwnd, PROGRESS_TIMER_ID, PROGRESS_INTERVAL_MS, None);

    Ok(hwnd)
}

unsafe fn create_font(height: i32, weight: i32) -> isize {
    create_font_for_family(height, weight, "Segoe UI Variable Display")
        .or_else(|| create_font_for_family(height, weight, "Segoe UI"))
        .unwrap_or(0)
}

unsafe fn create_font_for_family(height: i32, weight: i32, family: &str) -> Option<isize> {
    let wide = to_wide(family);
    let font = CreateFontW(
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
        CLEARTYPE_QUALITY as u32,
        FF_DONTCARE as u32,
        wide.as_ptr(),
    ) as isize;
    if font == 0 {
        None
    } else {
        Some(font)
    }
}
