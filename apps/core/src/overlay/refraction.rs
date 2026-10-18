//! Native glass spike: top-level glass window + DXGI capture feed.
//!
//! History: a transparent D3D11 *child* beneath the WebView presented
//! fine but never reached the screen (input worked, pixels didn't —
//! under flip, bitblt and GDI alike). The glass is now a top-level
//! window: borderless, click-through, never activating, hidden from
//! taskbar and Alt-Tab, kept pixel-aligned below the main panel.
//!
//! Step 3 feeds it live desktop pixels via DXGI Desktop Duplication
//! (no picker, no OS border — WGC's interop helpers aren't projected
//! in this windows version). All capture + render work runs on one
//! worker thread; the UI thread only moves/shows the window. The host
//! sends configs; the worker owns every D3D/D2D object.
//!
//! Refraction is enabled by default. Set `NEX_REFRACT_LAB=0` to force
//! the Acrylic fallback. Logs use the `[nex][refract]` prefix.

#![cfg(target_os = "windows")]

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::overlay::host::UiCommand;
use tao::event_loop::EventLoopProxy;
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DISPLACEMENTMAP_PROP_SCALE,
    D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT,
    D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT,
    D2D1_SATURATION_PROP_SATURATION,
    D2D1_CHANNEL_SELECTOR_G, D2D1_CHANNEL_SELECTOR_R,
    CLSID_D2D1DisplacementMap, CLSID_D2D1GaussianBlur, CLSID_D2D1Saturation, ID2D1Bitmap1,
    ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Factory1, ID2D1Image,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_SCALING, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT, DXGI_SWAP_EFFECT_DISCARD,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter,
    IDXGIDevice, IDXGIFactory, IDXGIFactory2, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_ALPHA_MODE_UNSPECIFIED,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, RegisterClassW,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SW_SHOWNA,
    WDA_EXCLUDEFROMCAPTURE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Foundation::POINT;
use windows::{core::w, Win32::System::LibraryLoader::GetModuleHandleW};

/// Refraction is on by default; an explicit zero keeps the Acrylic fallback.
pub fn enabled() -> bool {
    std::env::var("NEX_REFRACT_LAB")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

/// Match the reference liquid-glass renderer at 96 DPI. Heavy enough to
/// read as frost rather than acrylic, light enough to keep the bezel
/// bend legible.
const BLUR_STDDEV: f32 = 8.0;
/// Backdrop vibrancy: the refracted image is lifted slightly so it does
/// not read as greyed out behind the tint.
const SATURATION: f32 = 1.2;
const BEND_SCALE: f32 = 10.0;
/// Show swell: the lens eases from flat to full just after the reveal, so
/// the panel reads as glass settling rather than a pane switching on.
/// One shot per show; nothing animates while hidden.
const SWELL_MS: f32 = 220.0;
const BEZEL_MARGIN: f32 = 32.0;
const CORNER_RADIUS: f32 = 8.0;
/// Material follows the backdrop, like Apple's vibrancy: a bright scene
/// gets a light surface with dark text, a dark scene the reverse, so
/// the text colour never fights what is behind the glass. Alpha is the
/// wash at the material's own extreme; hysteresis (plus the dwell below)
/// stops flapping when the backdrop sits near the switch.
const TINT_MIN_ALPHA: f32 = 0.10;
const TINT_MAX_ALPHA: f32 = 0.28;
const MATERIAL_SWITCH: f32 = 0.55;
const MATERIAL_HYSTERESIS: f32 = 0.18;
/// Consecutive frames past the switch before the material flips. Scene
/// cuts and fast scrolling cross it for a frame or two; without this the
/// palette flashes.
const MATERIAL_DWELL: u32 = 8;
const TINT_DARK: (f32, f32, f32) = (12.0 / 255.0, 14.0 / 255.0, 20.0 / 255.0);
const TINT_LIGHT: (f32, f32, f32) = (255.0 / 255.0, 255.0 / 255.0, 255.0 / 255.0);
/// Per-frame exponential smoothing on backdrop luminance (~150ms at
/// 60fps) so a scene cut eases the tint instead of snapping it.
const LUMA_EMA: f32 = 0.10;
/// Extra desktop pixels around the panel keep the blur kernel away from
/// the capture boundary: 3σ + 2, the reference renderer's rule, now
/// scaled up with the blur. The panel must sit this far inside the
/// monitor or the crop clamps and the worker drops frames, so the pad
/// is kept to the blur's actual reach rather than padded for comfort.
const BLUR_PAD: u32 = (BLUR_STDDEV as u32) * 3 + 2;

/// Render cap: live frames present at most this often.
const MIN_FRAME_MS: u128 = 16;

/// How recently the desktop must have handed us a frame for the backdrop
/// to count as moving. Comfortably longer than a 30fps frame interval,
/// short enough that a show over a paused player still animates the swell
/// off the held mirror.
const DESKTOP_RECENT_MS: u64 = 60;

/// While hidden, poll the desktop at most this often to keep the backdrop
/// material current. The palette the page opens with comes from this, so
/// a slow poll is enough, and presenting is skipped.
const HIDDEN_POLL_MS: u64 = 250;

/// Host -> worker config. `main_sys == 0` / `None` means unchanged.
struct GlassConfig {
    visible: Option<bool>,
    main_sys: isize,
    max_wh: Option<(u32, u32)>,
    /// Requested panel size (physical px). The worker renders this
    /// instead of polling the window rect, so content tracks the region
    /// on the same tick — no frame of stale rows on growth.
    panel_wh: Option<(u32, u32)>,
    /// Text theme. Seeds the material before the first frame (a light
    /// theme opens on a light surface); every frame then re-decides
    /// from the backdrop.
    theme_dark: Option<bool>,
}

/// Host-facing handle. All methods are UI-thread window ops plus a
/// config send; every D3D/D2D object lives on the worker.
pub struct GlassLayer {
    window: GlassWindow,
    tx: Sender<GlassConfig>,
    ready: Arc<AtomicBool>,
}

struct GlassWindow {
    hwnd: HWND,
    /// Physical HWND size. The glass lives at max-panel size permanently;
    /// resizes only shrink/grow the round region below, so DWM and the
    /// swapchain never see a resize in steady state (no flash, no lag).
    hwnd_size: std::cell::Cell<(u32, u32)>,
    last_region: std::cell::Cell<Option<(u32, u32, u64)>>,
}

impl GlassLayer {
    /// Create the glass top-level window + worker. `None` when explicitly
    /// disabled or on any failure (warn-logged, caller falls back to
    /// Acrylic). Starts hidden; the host shows it with the panel.
    pub fn create_for_window(
        main_sys: isize,
        max_w_px: u32,
        max_h_px: u32,
        proxy: EventLoopProxy<UiCommand>,
    ) -> Option<GlassLayer> {
        if !enabled() {
            crate::logging::info("[nex][refract] disabled by environment — acrylic path");
            return None;
        }
        match GlassWindow::create(main_sys, max_w_px, max_h_px) {
            Ok(window) => {
                // The main window's own DWM backdrop would blur our live
                // pixels along with the desktop (that's the acrylic wash
                // hiding the glass). Hand the background to the glass;
                // Acrylic stays as-is when this call fails or glass dies.
                disable_main_backdrop(main_sys);
                let (tx, rx) = channel();
                let ready = Arc::new(AtomicBool::new(false));
                let worker_ready = Arc::clone(&ready);
                // HWND is a raw pointer (not Send): move it as isize and
                // rebuild it on the worker.
                let hwnd_sys = window.hwnd().0 as isize;
                match std::thread::Builder::new()
                    .name("glass-capture".into())
                    .spawn(move || {
                        worker_main(hwnd_sys, main_sys, rx, proxy, worker_ready, (max_w_px, max_h_px))
                    })
                {
                    Ok(_) => {
                        crate::logging::info("[nex][refract] capture worker spawned");
                        Some(GlassLayer { window, tx, ready })
                    }
                    Err(error) => {
                        crate::logging::warn(&format!(
                            "[nex][refract] disabled, acrylic fallback: worker spawn failed: {error}"
                        ));
                        None
                    }
                }
            }
            Err(error) => {
                crate::logging::warn(&format!("[nex][refract] disabled, acrylic fallback: {error}"));
                None
            }
        }
    }

    /// Align to the main window and mark visible. Called on show/resize.
    /// Position/region only — never touches worker visibility: that is
    /// owned by set_visible(), otherwise every resize would resurrect a
    /// parked glass layer (visible square edges behind opaque panels).
    ///
    /// The HWND stays at max-panel size; only the round region tracks
    /// the current height, so a resize is one synchronous region call —
    /// no SetWindowPos, no swapchain resize, nothing to lag a frame.
    pub fn sync_to_main(
        &self,
        main_sys: isize,
        scale: f64,
        logical_w: f64,
        logical_h: f64,
        max_w_px: u32,
        max_h_px: u32,
    ) {
        // Requested size, not main_rect: tao applies set_inner_size
        // async, so reading the window back yields the stale rect.
        let (w, h) = (physical_px(logical_w, scale), physical_px(logical_h, scale));
        self.window.ensure_window_max(main_sys, max_w_px, max_h_px);
        self.window.apply_round_region(w, h, scale);
        let _ = self.tx.send(GlassConfig {
            visible: None,
            main_sys,
            max_wh: Some((max_w_px, max_h_px)),
            panel_wh: Some((w, h)),
            theme_dark: None,
        });
    }

    /// Show/hide with the panel. No animation, no activation.
    pub fn set_visible(&self, show: bool) {
        if !show || self.ready.load(Ordering::Acquire) {
            self.window.show(show);
        }
        let _ = self.tx.send(GlassConfig {
            visible: Some(show),
            main_sys: 0,
            max_wh: None,
            panel_wh: None,
            theme_dark: None,
        });
    }

    /// Tell the renderer a show is coming without revealing the window:
    /// the panel is only revealed on its first paint, and by then the
    /// worker has re-presented the held mirror, so the uncloak carries
    /// current pixels instead of the last session's.
    pub fn prepare_show(&self) {
        let _ = self.tx.send(GlassConfig {
            visible: Some(true),
            main_sys: 0,
            max_wh: None,
            panel_wh: None,
            theme_dark: None,
        });
    }

    /// Text theme. Seeds the material (light surface + dark text, or
    /// the reverse) until a captured frame can decide from the backdrop
    /// itself. Sent on create and on every show.
    pub fn set_theme(&self, dark: bool) {
        let _ = self.tx.send(GlassConfig {
            visible: None,
            main_sys: 0,
            max_wh: None,
            panel_wh: None,
            theme_dark: Some(dark),
        });
    }

    /// Reveal only after the renderer has presented a captured frame.
    pub fn show_ready_frame(&self) {
        if self.ready.load(Ordering::Acquire) {
            self.window.show(true);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}

/// Hand the background to the glass: the main window's DWM backdrop
/// would otherwise blur our live pixels into the acrylic wash.
fn disable_main_backdrop(main_sys: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
    };
    let backdrop = DWMSBT_NONE;
    let hr = unsafe {
        DwmSetWindowAttribute(
            main_sys as *mut std::ffi::c_void,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &backdrop as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&backdrop) as u32,
        )
    };
    if hr == 0 {
        crate::logging::info("[nex][refract] main DWM backdrop off (glass owns the background)");
    } else {
        crate::logging::warn(&format!(
            "[nex][refract] backdrop off failed: HRESULT {hr} (glass will wash out)"
        ));
        return;
    }
    // Read back ground truth: the set above can silently not stick.
    let mut back: i32 = -1;
    let read_hr = unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            main_sys as *mut std::ffi::c_void,
            windows_sys::Win32::Graphics::Dwm::DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &mut back as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of_val(&back) as u32,
        )
    };
    // NONE=1, TRANSIENTWINDOW=3. Anything but 1 means the wash is still on.
    crate::logging::info(&format!(
        "[nex][refract] backdrop readback={back} (want 1=NONE, 3=TRANSIENT; get/rc={read_hr})"
    ));
}

/// Composition-level visibility. A cloaked window keeps its swapchain
/// image while staying out of the composition, which is what makes an
/// instant reveal possible.
fn set_glass_cloaked(hwnd: HWND, cloak: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let value: i32 = if cloak { 1 } else { 0 };
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd.0 as *mut std::ffi::c_void,
            DWMWA_CLOAK as u32,
            &value as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        )
    };
    if hr < 0 {
        crate::logging::warn(&format!(
            "[nex][refract] glass cloak={cloak} failed: HRESULT {hr}"
        ));
    }
}

impl GlassWindow {
    /// Max-panel HWND, top-left aligned to the main window. The visible
    /// shape comes from the round region (per-resize); the window and
    /// swapchain buffers stay max-size so neither ever resizes.
    fn create(main_sys: isize, max_w_px: u32, max_h_px: u32) -> Result<GlassWindow, String> {
        let instance: HMODULE = unsafe { GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e:?}"))? };
        let class = w!("NexGlassLayer");
        let wndclass = WNDCLASSW {
            style: windows::Win32::UI::WindowsAndMessaging::WNDCLASS_STYLES(0),
            lpfnWndProc: Some(glass_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance.into(),
            hIcon: windows::Win32::UI::WindowsAndMessaging::HICON(std::ptr::null_mut()),
            hCursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR(std::ptr::null_mut()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(std::ptr::null_mut()),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: class,
        };
        let atom = unsafe { RegisterClassW(&wndclass) };
        if atom == 0 {
            crate::logging::info(&format!(
                "[nex][refract] RegisterClassW returned 0, continuing (last error {})",
                last_error_code()
            ));
        }

        let main = HWND(main_sys as *mut std::ffi::c_void);
        let (x, y) = main_origin(main_sys).unwrap_or((0, 0));
        let (width, height) = (max_w_px, max_h_px);

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class,
                w!("NexGlass"),
                WS_POPUP,
                x,
                y,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| format!("CreateWindowExW glass failed: {e:?} (last error {})", last_error_code()))?
        };
        // Self-exclusion first: without it we'd capture our own panel
        // into a feedback mirror. Failure is warn-and-continue for the
        // spike (the log says which side refused).
        for (label, target) in [("glass", hwnd), ("main", main)] {
            match unsafe { SetWindowDisplayAffinity(target, WDA_EXCLUDEFROMCAPTURE) } {
                Ok(()) => crate::logging::info(&format!("[nex][refract] {label} excluded from capture")),
                Err(error) => crate::logging::warn(&format!(
                    "[nex][refract] {label} exclusion failed: {error:?} (feedback risk)"
                )),
            }
        }
        // Shown once, cloaked from here on. Revealing a window that was
        // hidden put an empty window over the desktop: a hidden window
        // loses its swapchain image, so the panel stayed see-through
        // until the next present landed. A cloaked window is not
        // composed but stays shown, so the last presented frame is
        // there the instant it is uncloaked.
        set_glass_cloaked(hwnd, true);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }

        // No DWM transitions on the glass itself: sync_to_main
        // re-inserts it in z-order on every resize, and an animated
        // insert over a flip swapchain reads as a black flash.
        {
            use windows_sys::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
            };
            let off = 1_i32;
            unsafe {
                DwmSetWindowAttribute(
                    hwnd.0 as *mut std::ffi::c_void,
                    DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                    &off as *const _ as *const std::ffi::c_void,
                    std::mem::size_of_val(&off) as u32,
                );
            }
        }
        crate::logging::info(&format!(
            "[nex][refract] glass top-level hwnd={hwnd:?} {width}x{height}px at ({x},{y})"
        ));
        Ok(GlassWindow {
            hwnd,
            hwnd_size: std::cell::Cell::new((width, height)),
            last_region: std::cell::Cell::new(None),
        })
    }

    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Grow the HWND if the max panel outgrew it (DPI/scale change).
    /// Steady-state resizes never come here: the region clips the
    /// max-size window, so no SetWindowPos churn per resize.
    fn ensure_window_max(&self, main_sys: isize, max_w: u32, max_h: u32) {
        let cur = self.hwnd_size.get();
        if max_w <= cur.0 && max_h <= cur.1 {
            return;
        }
        let Some((x, y)) = main_origin(main_sys) else {
            return;
        };
        let main = HWND(main_sys as *mut std::ffi::c_void);
        if let Err(error) = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(main),
                x,
                y,
                max_w as i32,
                max_h as i32,
                SWP_NOACTIVATE,
            )
        } {
            crate::logging::warn(&format!("[nex][refract] glass grow failed: {error:?}"));
        } else {
            self.hwnd_size.set((max_w, max_h));
        }
    }

    fn show(&self, show: bool) {
        // Visibility is the DWM cloak alone: ShowWindow(SW_HIDE) drops
        // the rendered frame, so un-hiding put an empty window on screen.
        set_glass_cloaked(self.hwnd, !show);
        crate::logging::info(&format!("[nex][refract] glass visible={show}"));
    }

    /// Match the panel's rounded corners so sharp desktop slivers never
    /// peek past the glass. Skips identical regions: re-setting the same
    /// region forces a repaint/recompose of the flip window (flash).
    fn apply_round_region(&self, w_px: u32, h_px: u32, scale: f64) {        use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
        let key = (w_px, h_px, scale.to_bits());
        if self.last_region.get() == Some(key) {
            return;
        }
        // Panel radius token is 8 logical px; region wants diameters.
        let d = ((8.0 * scale * 2.0).round() as i32).max(2);
        let rgn = unsafe {
            CreateRoundRectRgn(0, 0, w_px as i32 + 1, h_px as i32 + 1, d, d)
        };
        if rgn.is_null() {
            return;
        }
        // Ownership passes to the OS on success; delete only on failure.
        if unsafe { SetWindowRgn(self.hwnd.0, rgn, 1) } == 0 {
            unsafe {
                DeleteObject(rgn);
            }
        } else {
            self.last_region.set(Some(key));
        }
    }
}

/// Worker-owned renderer: every D3D/D2D object. Built once at max
/// panel size; panel-size changes rebuild content bitmaps only — the
/// buffers (and HWND) never resize, so presents never invalidate.
struct Engine {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain1,
    d2d: ID2D1DeviceContext,
    /// Size-agnostic effect graph, built once: rebuilds only swap the
    /// bitmaps underneath via SetInput, so a panel resize costs two
    /// bitmap creates instead of a full graph rebuild.
    blur: ID2D1Effect,
    /// Backdrop vibrancy: blur output lifted before the bend.
    saturate: ID2D1Effect,
    displace: ID2D1Effect,
    source: Option<ID2D1Bitmap1>,
    map: Option<ID2D1Bitmap1>,
    tint_brush: ID2D1SolidColorBrush,
    /// Swapchain buffer size (max panel; grow-only).
    width: u32,
    height: u32,
    /// Currently rendered panel size (top-left of the buffers; the
    /// glass region clips the rest).
    panel_w: u32,
    panel_h: u32,
}

impl Engine {
    /// Build everything at max-panel buffers with the current panel
    /// content size: device, swapchain (first working desc wins), D2D
    /// graph, and effect resources.
    fn build(hwnd: HWND, buf_w: u32, buf_h: u32, panel_w: u32, panel_h: u32) -> Result<Engine, String> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;
        unsafe {
            windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device as *mut _),
                Some(&mut feature_level as *mut _),
                Some(&mut context as *mut _),
            )
            .map_err(|e| format!("D3D11CreateDevice failed: {e:?}"))?;
        }
        let device = device.ok_or("D3D11CreateDevice returned no device")?;
        let context = context.ok_or("D3D11CreateDevice returned no context")?;

        let factory: IDXGIFactory2 =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
                .map_err(|e| format!("CreateDXGIFactory2 failed: {e:?}"))?;
        let mut desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: buf_w,
            Height: buf_h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let variants: [(&str, DXGI_SCALING, DXGI_ALPHA_MODE, u32, DXGI_SWAP_EFFECT); 4] = [
            (
                "premultiplied+stretch",
                DXGI_SCALING_STRETCH,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
                2,
                DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ),
            (
                "premultiplied+none",
                DXGI_SCALING_NONE,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
                2,
                DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ),
            (
                "bitblt+discard",
                DXGI_SCALING_STRETCH,
                DXGI_ALPHA_MODE_UNSPECIFIED,
                1,
                DXGI_SWAP_EFFECT_DISCARD,
            ),
            (
                "unspecified+stretch",
                DXGI_SCALING_STRETCH,
                DXGI_ALPHA_MODE_UNSPECIFIED,
                2,
                DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ),
        ];
        let mut swapchain: Option<IDXGISwapChain1> = None;
        for (name, scaling, alpha, buffer_count, effect) in variants {
            desc.Scaling = scaling;
            desc.AlphaMode = alpha;
            desc.BufferCount = buffer_count;
            desc.SwapEffect = effect;
            match unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) } {
                Ok(chain) => {
                    crate::logging::info(&format!("[nex][refract] swapchain ok via {name}"));
                    swapchain = Some(chain);
                    break;
                }
                Err(error) => crate::logging::warn(&format!(
                    "[nex][refract] swapchain {name} failed: {error:?}"
                )),
            }
        }
        let swapchain = swapchain.ok_or("all swapchain desc variants failed")?;

        let d2d = d2d_setup(&device)?;
        crate::logging::info(&format!(
            "[nex][refract] d2d effect ready blur={BLUR_STDDEV} sat={SATURATION} bend={BEND_SCALE} margin={BEZEL_MARGIN}px pad={BLUR_PAD}"
        ));
        let tint_brush: ID2D1SolidColorBrush = unsafe {
            d2d.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: TINT_DARK.0,
                    g: TINT_DARK.1,
                    b: TINT_DARK.2,
                    a: TINT_MIN_ALPHA + (TINT_MAX_ALPHA - TINT_MIN_ALPHA) * 0.5,
                },
                None,
            )
            .map_err(|e| format!("glass tint brush failed: {e:?}"))?
        };
        let blur: ID2D1Effect = unsafe {
            d2d.CreateEffect(&CLSID_D2D1GaussianBlur as *const _)
                .map_err(|e| format!("blur CreateEffect failed: {e:?}"))?
        };
        unsafe {
            blur
                .SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &BLUR_STDDEV.to_ne_bytes(),
                )
                .map_err(|e| format!("blur SetValue failed: {e:?}"))?;
        }
        let saturate: ID2D1Effect = unsafe {
            d2d.CreateEffect(&CLSID_D2D1Saturation as *const _)
                .map_err(|e| format!("saturation CreateEffect failed: {e:?}"))?
        };
        unsafe {
            saturate
                .SetValue(
                    D2D1_SATURATION_PROP_SATURATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &SATURATION.to_ne_bytes(),
                )
                .map_err(|e| format!("saturation SetValue failed: {e:?}"))?;
        }
        let displace: ID2D1Effect = unsafe {
            d2d.CreateEffect(&CLSID_D2D1DisplacementMap as *const _)
                .map_err(|e| format!("displace CreateEffect failed: {e:?}"))?
        };
        unsafe {
            displace
                .SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &BEND_SCALE.to_ne_bytes(),
                )
                .map_err(|e| format!("displace scale failed: {e:?}"))?;
            // The effect defaults both axes to alpha, which is opaque in
            // this map and creates a constant down-right sample offset.
            // The bezel encodes horizontal displacement in red and vertical
            // displacement in green, so select those channels explicitly.
            displace
                .SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &D2D1_CHANNEL_SELECTOR_R.0.to_ne_bytes(),
                )
                .map_err(|e| format!("displace X channel select failed: {e:?}"))?;
            displace
                .SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &D2D1_CHANNEL_SELECTOR_G.0.to_ne_bytes(),
                )
                .map_err(|e| format!("displace Y channel select failed: {e:?}"))?;
        }
        let mut engine = Engine {
            device,
            context,
            swapchain,
            d2d,
            source: None,
            map: None,
            tint_brush,
            width: buf_w,
            height: buf_h,
            panel_w,
            panel_h,
            blur,
            saturate,
            displace,
        };
        engine.rebuild_content()?;
        Ok(engine)
    }

    /// Rebuild size-dependent content: a transparent initial source,
    /// displacement map, and blur→displacement pair, sized to the
    /// current panel (top-left of the max buffers). Live captured frames
    /// replace the source via ingest_frame before presentation.
    fn rebuild_content(&mut self) -> Result<(), String> {
        let (w, h) = (
            self.panel_w as usize + BLUR_PAD as usize * 2,
            self.panel_h as usize + BLUR_PAD as usize * 2,
        );
        let src_bytes = vec![0u8; w * h * 4];
        let mut map_bytes = vec![128u8; w * h * 4];
        for alpha in map_bytes.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        let panel_map = bezel_bytes(self.panel_w as usize, self.panel_h as usize);
        for row in 0..self.panel_h as usize {
            let dst = ((row + BLUR_PAD as usize) * w + BLUR_PAD as usize) * 4;
            let src = row * self.panel_w as usize * 4;
            map_bytes[dst..dst + self.panel_w as usize * 4]
                .copy_from_slice(&panel_map[src..src + self.panel_w as usize * 4]);
        }
        let props = bitmap_props(D2D1_BITMAP_OPTIONS_NONE);
        let source: ID2D1Bitmap1 = unsafe {
            self.d2d
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: w as u32,
                        height: h as u32,
                    },
                    Some(src_bytes.as_ptr() as *const _),
                    (w * 4) as u32,
                    &props as *const _,
                )
                .map_err(|e| format!("source CreateBitmap failed: {e:?}"))?
        };
        let map: ID2D1Bitmap1 = unsafe {
            self.d2d
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: w as u32,
                        height: h as u32,
                    },
                    Some(map_bytes.as_ptr() as *const _),
                    (w * 4) as u32,
                    &props as *const _,
                )
                .map_err(|e| format!("map CreateBitmap failed: {e:?}"))?
        };
        // Re-point the shared effect graph at the new bitmaps. Effects
        // are size-agnostic; only the bitmaps track the panel.
        unsafe {
            self.blur.SetInput(0, &source, true);
            self.saturate.SetInput(0, &effect_output(&self.blur)?, true);
            self.displace.SetInput(0, &effect_output(&self.saturate)?, true);
            self.displace.SetInput(1, &map, true);
        }
        self.source = Some(source);
        self.map = Some(map);
        Ok(())
    }

    /// Upload one captured frame over the source bitmap. Size drift
    /// (panel resized mid-flight) drops the frame; the rebuild already
    /// queued covers the next one.
    fn ingest_frame(&self, pixels: &[u8], w: u32, h: u32) -> Result<(), String> {
        let source = self.source.as_ref().ok_or("no source bitmap")?;
        let expected_w = self.panel_w + BLUR_PAD * 2;
        let expected_h = self.panel_h + BLUR_PAD * 2;
        if w != expected_w || h != expected_h {
            return Err("size drift, dropped".to_string());
        }
        if pixels.len() != (w * h * 4) as usize {
            return Err("pixel size mismatch, dropped".to_string());
        }
        unsafe {
            source
                .CopyFromMemory(None, pixels.as_ptr() as *const _, w * 4)
                .map_err(|e| format!("CopyFromMemory failed: {e:?}"))?;
        }
        Ok(())
    }

    /// Upload and present one frame with the current material's tint.
    /// Uploading also refreshes the source bitmap after a resize.
    fn present_frame(
        &self,
        pixels: &[u8],
        w: u32,
        h: u32,
        material: &Material,
        bend: f32,
    ) -> Result<(), String> {
        self.set_tint(material.wash());
        self.set_bend(bend);
        self.ingest_frame(pixels, w, h)?;
        self.render_frame()
    }

    /// Lens strength: 0 is a flat pane, 1 the full rim bend. The show
    /// swell drives this; setting it costs one effect property write.
    fn set_bend(&self, bend: f32) {
        let scale = BEND_SCALE * bend.clamp(0.0, 1.0);
        unsafe {
            let _ = self.displace.SetValue(
                D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &scale.to_ne_bytes(),
            );
        }
    }

    /// Wash colour + alpha over the refracted backdrop.
    fn set_tint(&self, wash: (f32, f32, f32, f32)) {
        let (r, g, b, a) = wash;
        unsafe {
            self.tint_brush.SetColor(&D2D1_COLOR_F {
                r,
                g,
                b,
                a: a.clamp(0.0, 1.0),
            });
        }
    }

    /// Draw the effect into the current backbuffer and present it. The
    /// target rebinds every frame: bitblt-model buffers rotate.
    fn render_frame(&self) -> Result<(), String> {
        let displace = &self.displace;
        let texture: ID3D11Texture2D = unsafe {
            self.swapchain
                .GetBuffer(0)
                .map_err(|e| format!("swapchain GetBuffer failed: {e:?}"))?
        };
        let surface: IDXGISurface = texture
            .cast()
            .map_err(|e| format!("QI IDXGISurface failed: {e:?}"))?;
        let target: ID2D1Bitmap1 = unsafe {
            self.d2d
                .CreateBitmapFromDxgiSurface(&surface, Some(&target_props() as *const _))
                .map_err(|e| format!("target bind failed: {e:?}"))?
        };
        unsafe {
            self.d2d.SetTarget(&target);
            self.d2d.BeginDraw();
            let source_rect = D2D_RECT_F {
                left: BLUR_PAD as f32,
                top: BLUR_PAD as f32,
                right: (BLUR_PAD + self.panel_w) as f32,
                bottom: (BLUR_PAD + self.panel_h) as f32,
            };
            self.d2d.DrawImage(
                &effect_output(displace)?,
                None,
                Some(&source_rect as *const _),
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            let tint = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: self.width as f32,
                bottom: self.height as f32,
            };
            self.d2d
                .FillRectangle(&tint as *const _, &self.tint_brush);
            self.d2d
                .EndDraw(None, None)
                .map_err(|e| format!("EndDraw failed: {e:?}"))?;
            // Keep the newest desktop frame flowing. In a composed window,
            // Present(0) avoids the extra vblank wait that was throttling the
            // glass to ~20 FPS when the desktop was also presenting video.
            self.swapchain
                .Present(0, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("Present failed: {e:?}"))?;
        }
        Ok(())
    }

    /// Grow the buffers (DPI/scale change only). Steady-state panel
    /// resizes never come here — content rebuilds in place instead.
    fn resize_buffers(&mut self, width: u32, height: u32) -> Result<(), String> {
        // D2D retains the last target even after render_frame's local bitmap
        // is dropped. Detach it before ResizeBuffers or DXGI sees a live
        // backbuffer reference and rejects the resize.
        unsafe { self.d2d.SetTarget(None) };
        let result = unsafe {
            self.swapchain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_FLAG(0),
            )
        };
        match result {
            Ok(()) => {
                self.width = width;
                self.height = height;
                self.rebuild_content()
            }
            Err(error) => Err(format!("ResizeBuffers failed: {error:?}")),
        }
    }
}

fn d2d_setup(d3d: &ID3D11Device) -> Result<ID2D1DeviceContext, String> {
    let factory: ID2D1Factory = unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory failed: {e:?}"))?
    };
    let dxgi: IDXGIDevice = d3d
        .cast()
        .map_err(|e| format!("QI IDXGIDevice failed: {e:?}"))?;
    // CreateDevice lives on ID2D1Factory1+, not the base factory.
    let factory1: ID2D1Factory1 = factory
        .cast()
        .map_err(|e| format!("QI ID2D1Factory1 failed: {e:?}"))?;
    let device: ID2D1Device = unsafe {
        factory1
            .CreateDevice(&dxgi)
            .map_err(|e| format!("ID2D1Factory::CreateDevice failed: {e:?}"))?
    };
    let d2d: ID2D1DeviceContext = unsafe {
        device
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .map_err(|e| format!("CreateDeviceContext failed: {e:?}"))?
    };
    // DIPs == px everywhere: bitmaps carry 96 DPI and DrawImage maps 1:1.
    unsafe {
        d2d.SetDpi(96.0, 96.0);
    }
    Ok(d2d)
}

fn bitmap_props(options: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    bitmap_props_with_alpha(options, D2D1_ALPHA_MODE_PREMULTIPLIED)
}

/// Swapchain backbuffers on this box come from the bitblt path, which
/// carries no alpha — declaring PREMULTIPLIED there fails binding with
/// E_INVALIDARG. IGNORE matches the surface; the layer is opaque and
/// the page blends above it, so nothing is lost. CANNOT_DRAW is required
/// for swapchain-bound targets.
fn target_props() -> D2D1_BITMAP_PROPERTIES1 {
    bitmap_props_with_alpha(
        D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        D2D1_ALPHA_MODE_IGNORE,
    )
}

fn bitmap_props_with_alpha(
    options: D2D1_BITMAP_OPTIONS,
    alpha: windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE,
) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: alpha,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

/// Output image of an effect. Effects derive from ID2D1Properties, not
/// ID2D1Image, so chaining and drawing go through GetOutput.
fn effect_output(effect: &ID2D1Effect) -> Result<ID2D1Image, String> {
    unsafe {
        effect
            .GetOutput()
            .map_err(|e| format!("effect GetOutput failed: {e:?}"))
    }
}

/// Rounded convex-rim displacement map: neutral gray center, bevel
/// channels bend samples inward (Source[p + Amount·(channel − 0.5)]).
fn bezel_bytes(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let (width, height) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            // Rounded-rectangle signed-distance field from the lab:
            // the center remains undistorted while the curved rim eases
            // the captured image toward the pane's center.
            let px = x as f32 + 0.5 - width / 2.0;
            let py = y as f32 + 0.5 - height / 2.0;
            let qx = px.abs() - (width / 2.0 - CORNER_RADIUS);
            let qy = py.abs() - (height / 2.0 - CORNER_RADIUS);
            let ox = qx.max(0.0);
            let oy = qy.max(0.0);
            let outside = (ox * ox + oy * oy).sqrt();
            let depth = -(outside + qx.max(qy).min(0.0) - CORNER_RADIUS);
            let t = (depth / BEZEL_MARGIN).clamp(0.0, 1.0);
            let (dx, dy) = if t >= 1.0 {
                (0.0, 0.0)
            } else {
                let sx = if px < 0.0 { -1.0 } else { 1.0 };
                let sy = if py < 0.0 { -1.0 } else { 1.0 };
                let (nx, ny) = if qx > 0.0 && qy > 0.0 {
                    (qx / outside * sx, qy / outside * sy)
                } else if qx > qy {
                    (sx, 0.0)
                } else {
                    (0.0, sy)
                };
                let strength = (1.0 - t) * (1.0 - t);
                (-nx * strength, -ny * strength)
            };
            let r = (128.0 + dx * 127.0).round().clamp(0.0, 255.0) as u8;
            let g = (128.0 + dy * 127.0).round().clamp(0.0, 255.0) as u8;
            out.extend_from_slice(&[128, g, r, 255]);
        }
    }
    out
}

/// Worker-owned capture: duplication session + full-frame staging.
struct Capture {
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    frame_w: u32,
    frame_h: u32,
    origin_x: i32,
    origin_y: i32,
    monitor: HMONITOR,
    protected_logged: bool,
}

/// Lens strength for the show swell: 0 at the start, 1 once it has run
/// out. Ease-out cubic - the rim reaches shape quickly, then settles.
fn swell_bend(elapsed_ms: f32) -> f32 {
    let p = (elapsed_ms / SWELL_MS).clamp(0.0, 1.0);
    1.0 - (1.0 - p).powi(3)
}

/// What the worker should do with this pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pass {
    /// Take a frame: the panel is up, or the startup prewarm still owes one.
    Frame,
    /// Hidden idle: decide the material, then wait out the poll interval.
    Poll,
    /// Hidden idle, poll interval not elapsed yet: sleep and re-check.
    Wait,
}

/// Hidden passes decide the material only, once per `HIDDEN_POLL_MS`.
///
/// Nothing is presented while hidden and no full-rate acquire/render runs,
/// but the material must still track the backdrop: the host pushes the
/// show state — palette included — *before* it reveals the panel, so that
/// push has to already carry the current backdrop's material or the page
/// paints the last session's. The startup prewarm is exempt: it must
/// present so the first show has a ready surface.
fn pass_plan(visible: bool, prewarm_pending: bool, since_poll_ms: u64) -> Pass {
    if visible || prewarm_pending {
        return Pass::Frame;
    }
    if since_poll_ms < HIDDEN_POLL_MS {
        return Pass::Wait;
    }
    Pass::Poll
}

/// Present pacing for swell passes: a static desktop never sends a frame,
/// so the swell re-presents the held mirror — paced like a capture, so a
/// stalled desktop cannot spin the loop.
fn swell_pass_due(since_present_ms: u64) -> bool {
    since_present_ms >= MIN_FRAME_MS as u64
}

/// Does the desktop count as producing frames of its own?
///
/// `since_last_frame_ms` is how long ago the desktop last handed us a
/// frame; `None` means it never has. Deliberately a question about *age*,
/// not about the previous pass: see `swell_wants_mirror` for why a
/// per-pass answer breaks a show over playing video.
fn desktop_recent(since_last_frame_ms: Option<u64>) -> bool {
    since_last_frame_ms.is_some_and(|ms| ms < DESKTOP_RECENT_MS)
}

/// Does a due swell pass need the held mirror?
///
/// Only while the desktop is not giving frames of its own. Both sources
/// paint the same animation, but the mirror is frozen while the panel is
/// visible, and a mirror re-present resets the gate that paces captures:
/// with one running, every capture pass gets dropped and the backdrop
/// holds still for the whole swell (SWELL_MS) — a stale first frame over
/// anything playing, which is what every open used to look like.
///
/// `desktop_recent` must mean "the desktop produced a frame recently",
/// never "the last pass got one": the desktop yields a frame every
/// 16-33ms while this loop turns over in 2-5ms, so a per-pass flag reads
/// false for most passes *while video is plainly playing*, and the whole
/// show degrades back to a frozen mirror.
fn swell_wants_mirror(swell_due: bool, desktop_recent: bool) -> bool {
    swell_due && !desktop_recent
}

/// Per-show state the worker arms on a fresh show and consumes while it
/// presents: one held re-present and one lens swell.
struct ShowCycle {
    /// Re-present the held mirror on the next pass. A show is about to be
    /// revealed and a flip swapchain paints nothing until the next
    /// Present, while the desktop may stay still for a while yet.
    refresh: bool,
    /// When the current swell began, if one is running.
    swell: Option<Instant>,
}

impl ShowCycle {
    /// Arm a fresh show: present the held mirror and swell the lens in.
    fn arm(&mut self) {
        self.refresh = true;
        self.swell = Some(Instant::now());
    }

    /// Take the pending held re-present. One per show — a refresh that is
    /// never consumed re-presents the mirror every pass.
    fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh)
    }

    /// Lens strength this pass, and whether the swell is still running.
    fn bend(&mut self) -> (f32, bool) {
        match self.swell {
            Some(t0) => self.bend_at(t0.elapsed().as_secs_f32() * 1000.0),
            None => (1.0, false),
        }
    }

    /// [`Self::bend`] for an elapsed time, so the run-out is testable
    /// without a clock. Clears the swell once it reaches full.
    fn bend_at(&mut self, elapsed_ms: f32) -> (f32, bool) {
        let value = swell_bend(elapsed_ms);
        if value >= 1.0 {
            self.swell = None;
            (value, false)
        } else {
            (value, true)
        }
    }

    /// Is a swell running (so this pass should re-present the mirror)?
    fn swelling(&self) -> bool {
        self.swell.is_some()
    }
}

/// Pull the desktop's newest frame into the staging mirror, if the
/// desktop has produced one since the last acquire. Returns whether the
/// mirror moved.
///
/// A show wants the desktop as it is *now*, not as the hidden poll last
/// saw it: the poll runs every `HIDDEN_POLL_MS`, which is most of a
/// second of video motion, and the reveal shows whatever we present next.
/// AcquireNextFrame only yields when the desktop changed, so this either
/// hands us the current image or proves nothing has changed since the
/// poll - and then the mirror is already current, so a timeout is not a
/// miss. Nonblocking, and any failure (lost access, dead session) leaves
/// the mirror untouched; the capture path proper handles a dead session
/// on its next pass.
fn refresh_staging(cap: &mut Capture, context: &ID3D11DeviceContext) -> bool {
    let mut info: windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO =
        unsafe { std::mem::zeroed() };
    let mut resource: Option<IDXGIResource> = None;
    match unsafe {
        cap.duplication
            .AcquireNextFrame(0, &mut info as *mut _, &mut resource as *mut _)
    } {
        Ok(()) => {}
        Err(_) => return false,
    }
    let copied = (|| -> Result<(), String> {
        let texture: ID3D11Texture2D = resource
            .as_ref()
            .ok_or("empty frame resource")?
            .cast()
            .map_err(|e| format!("frame QI texture failed: {e:?}"))?;
        unsafe { context.CopySubresourceRegion(&cap.staging, 0, 0, 0, 0, &texture, 0, None) };
        Ok(())
    })()
    .is_ok();
    let _ = unsafe { cap.duplication.ReleaseFrame() };
    copied
}

fn worker_main(
    glass_sys: isize,
    initial_main_sys: isize,
    rx: Receiver<GlassConfig>,
    proxy: EventLoopProxy<UiCommand>,
    ready: Arc<AtomicBool>,
    init_max: (u32, u32),
) {
    let hwnd = HWND(glass_sys as *mut std::ffi::c_void);
    crate::logging::info("[nex][refract] worker alive (D3D11/DXGI/D2D need no COM init here)");
    let mut engine: Option<Engine> = None;
    let mut capture: Option<Capture> = None;
    let mut visible = false;
    let mut main_sys: isize = initial_main_sys;
    // Max panel the buffers/HWND cover (grow-only in steady state).
    let mut max_wh = init_max;
    // Requested panel size from the host; preferred over the polled
    // rect so the first render after a resize is already correct.
    let mut cfg_panel: Option<(u32, u32)> = None;
    // Build and render one frame while hidden, so the first show has a
    // ready surface instead of waiting for D3D/capture initialization.
    let mut prewarm_pending = true;
    let mut last_present = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    // Anchor for the hidden material poll.
    let mut last_warm = Instant::now();
    let mut stat_t = Instant::now();
    let (mut n_cap, mut n_pre, mut copy_us, mut render_us) = (0u64, 0u64, 0u64, 0u64);
    // Last panel position the glass was aligned to. Re-align if the
    // window position changes while the capture worker is active.
    let mut last_glass_at = (0i32, 0i32);
    // Full-frame staging mirror valid yet (fresh capture starts
    // uninitialized; first mirrored frame arms it).
    let mut staged = false;
    // When the desktop last handed us a frame. Not a per-pass flag: the
    // desktop yields one frame every 16-33ms (60Hz compose, 30fps video)
    // while this loop turns over every 2-5ms, so "did the last pass
    // acquire" is false for most passes *while video is plainly
    // playing*. The swell asks this and the answer has to mean "the
    // desktop is producing frames", not "one happened to land just now".
    let mut last_desktop_frame: Option<Instant> = None;
    // Backdrop-driven material (surface appearance + page palette).
    // Seeded dark; the host sends the text theme on create and on each
    // show, and every captured frame re-decides from the backdrop.
    let mut material = Material {
        luma: None,
        light: false,
        dwell: 0,
        fresh: false,
    };
    // Last material the page was told about; None forces a re-send.
    let mut palette_sent: Option<bool> = None;
    // Per-show state: the held re-present and the lens swell a fresh show
    // arms (see `ShowCycle`).
    let mut cycle = ShowCycle { refresh: false, swell: None };
    // Last presented capture box, fallback fill when the mirror is not
    // yet armed (static desktop + panel change before first frame).
    let mut last_frame: Option<(Vec<u8>, u32, u32)> = None;
    // Straight failures in a row: a poisoned device/capture never heals
    // by retrying the same calls, so rebuild everything after a few.
    let mut n_fail = 0u32;

    loop {
        while let Ok(cfg) = rx.try_recv() {
            if let Some(v) = cfg.visible {
                if v && !visible {
                    // Fresh show: re-announce the material, the page may
                    // have re-seeded its palette to the system theme.
                    palette_sent = None;
                    // And let the first frame decide outright, so the
                    // page palette lands with the glass instead of
                    // waiting for the desktop to change eight times.
                    material.fresh = true;
                    material.dwell = 0;
                    // Present the held mirror before the reveal and swell
                    // the lens in: a pane of glass settling, not a surface
                    // snapping on.
                    cycle.arm();
                }
                visible = v;
            }
            if cfg.main_sys != 0 {
                main_sys = cfg.main_sys;
            }
            if let Some(m) = cfg.max_wh {
                max_wh = m;
            }
            if let Some(p) = cfg.panel_wh {
                cfg_panel = Some(p);
            }
            if let Some(dark) = cfg.theme_dark {
                // The theme seeds the material once, before any backdrop
                // has been seen. After that the backdrop rules: seeding
                // on every show would flip a backdrop that has not moved
                // whenever it sits inside the hysteresis band.
                if material.luma.is_none() {
                    material.light = !dark;
                    material.dwell = 0;
                    if let Some(e) = engine.as_ref() {
                        e.set_tint(material.wash());
                    }
                }
            }
        }
        if main_sys == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        // Hidden: hold the duplication session + staging mirror (no
        // full-rate acquire/render) so the next show presents a fresh
        // frame within milliseconds instead of paying a capture restart.
        // A slow poll still decides the material (see `pass_plan`).
        // ponytail: one idle session at 4Hz; no present while hidden.
        let plan = pass_plan(visible, prewarm_pending, last_warm.elapsed().as_millis() as u64);
        if plan == Pass::Wait {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if plan == Pass::Poll {
            last_warm = Instant::now();
        }
        // Hidden idle passes decide the material only. The startup
        // prewarm still presents, so the first show has a ready surface.
        let warm = plan == Pass::Poll;
        let Some((mx, my, mw, mh)) = main_rect(main_sys) else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        // Requested size wins over the polled rect: tao applies resizes
        // async, so the rect lags the region by a frame and growth would
        // reveal stale buffer rows meanwhile.
        let (pw, ph) = cfg_panel.unwrap_or((mw, mh));
        // Position follow (see declaration): re-align + log on moves.
        if (mx, my) != last_glass_at {
            align_glass_to_main(hwnd, main_sys, max_wh.0, max_wh.1);
            crate::logging::info(&format!("[nex][refract] glass follows panel to ({mx},{my})"));
            last_glass_at = (mx, my);
        }
        // Buffers + HWND cover the max panel; the region clips the
        // current size. Panel changes rebuild content bitmaps in place
        // (buffers stay valid, presents never stall); buffers grow only
        // on DPI/scale change. No steady-state ResizeBuffers, no lag.
        // Is the desktop producing frames of its own? Recent, not just
        // "the previous pass got lucky" - see the declaration.
        let moving = desktop_recent(last_desktop_frame.map(|t| t.elapsed().as_millis() as u64));
        // Lens strength this pass: rises through the show swell, 1 after.
        let bend = cycle.bend().0;
        if engine.is_none() {
            match Engine::build(hwnd, max_wh.0, max_wh.1, pw, ph) {
                Ok(e) => engine = Some(e),
                Err(error) => {
                    crate::logging::warn(&format!("[nex][refract] engine build failed: {error}"));
                    std::thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            }
        } else if let Some(e) = engine.as_mut() {
            // Rebuild, then re-present below. Buffers stay valid through
            // a panel rebuild (no ResizeBuffers), so the old frame shows
            // meanwhile — never black.
            let mut rebuilt = false;
            let t0 = Instant::now();
            if max_wh.0 > e.width || max_wh.1 > e.height {
                // Rare (DPI/scale change): grow buffers (rebuilds inside).
                e.panel_w = pw;
                e.panel_h = ph;
                if let Err(error) = e.resize_buffers(max_wh.0, max_wh.1) {
                    crate::logging::warn(&format!("[nex][refract] engine resize failed: {error}"));
                    engine = None;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                rebuilt = true;
            } else if e.panel_w != pw || e.panel_h != ph {
                e.panel_w = pw;
                e.panel_h = ph;
                if let Err(error) = e.rebuild_content() {
                    crate::logging::warn(&format!("[nex][refract] content rebuild failed: {error}"));
                    engine = None;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                rebuilt = true;
            }
            // A show re-presents the same held frame: the window is about
            // to be revealed and a flip swapchain paints nothing until
            // the next Present, while the desktop may stay still for a
            // while yet.
            let refresh = cycle.take_refresh();
            // The swell re-presents the held frame: the desktop is often
            // static, so no captured frame would drive the animation.
            // Capped like a captured frame, and only while the desktop
            // has nothing of its own to paint it with.
            let swell_due =
                cycle.swelling() && swell_pass_due(last_present.elapsed().as_millis() as u64);
            let swelling = swell_wants_mirror(swell_due, moving);
            // Swell passes are routine (a dozen per show): keep them out
            // of the log, only rebuilds and reveal refreshes are news.
            let announce = rebuilt || refresh;
            // A show is the one pass that must carry the desktop as it is
            // now: the host reveals as soon as the page paints, so this
            // present is the first thing seen. The hidden poll left the
            // mirror up to HIDDEN_POLL_MS stale, which reads as a frozen
            // frame over anything moving, so ask the desktop for a newer
            // one first and fall back to the mirror when it has none.
            let live = refresh
                && staged
                && capture
                    .as_mut()
                    .is_some_and(|cap| refresh_staging(cap, &e.context));
            if live {
                // A frame this fresh counts as the desktop moving: it *is*
                // the desktop, taken seconds later than any pass could.
                last_desktop_frame = Some(Instant::now());
            }
            if rebuilt || refresh || swelling {
                // Fresh zeros after rebuild, or a reveal pending: present
                // immediately. Exact crop from the staging mirror - never
                // a stretched old frame; stretch is only the fallback
                // when the mirror isn't armed yet.
                let exact: Option<(Vec<u8>, u32, u32)> = if staged {
                    capture.as_ref().and_then(|cap| {
                        let pad = BLUR_PAD as i32;
                        let (nbx, nby, nbw, nbh) = clamp_panel_box(
                            mx - pad,
                            my - pad,
                            pw + BLUR_PAD * 2,
                            ph + BLUR_PAD * 2,
                            cap.origin_x,
                            cap.origin_y,
                            cap.frame_w,
                            cap.frame_h,
                        );
                        if nbw != pw + BLUR_PAD * 2 || nbh != ph + BLUR_PAD * 2 {
                            return None;
                        }
                        crop_staging(
                            &e.context,
                            &e.device,
                            &cap.staging,
                            cap.frame_w,
                            cap.frame_h,
                            nbx,
                            nby,
                            nbw,
                            nbh,
                        )
                        .ok()
                        .map(|px| (px, nbw, nbh))
                    })
                } else {
                    None
                };
                if let Some((fresh, fw, fh)) = exact {
                    match e.present_frame(&fresh, fw, fh, &material, bend) {
                        Ok(()) => {
                            n_pre += 1;
                            last_present = Instant::now();
                            last_frame = Some((fresh, fw, fh));
                            if announce {
                                // live: the desktop had a newer frame than
                                // the hidden poll's. mirror: it had none, so
                                // the mirror already was the current desktop.
                                let source = if live { "live" } else { "mirror" };
                                crate::logging::info(&format!(
                                    "[nex][refract] panel {pw}x{ph} re-presented in {}ms ({source})",
                                    t0.elapsed().as_millis()
                                ));
                            }
                        }
                        Err(error) => crate::logging::warn(&format!(
                            "[nex][refract] resize re-present failed: {error}"
                        )),
                    }
                } else if let Some((prev, cw, ch)) = last_frame.as_ref() {
                    let (ew, eh) = (pw + BLUR_PAD * 2, ph + BLUR_PAD * 2);
                    if prev.len() == (*cw as usize) * (*ch as usize) * 4 {
                        let stretched = stretch_bgra_nearest(prev, *cw, *ch, ew, eh);
                        match e.present_frame(&stretched, ew, eh, &material, bend) {
                            Ok(()) => {
                                n_pre += 1;
                                last_present = Instant::now();
                                last_frame = Some((stretched, ew, eh));
                                if announce {
                                    crate::logging::info(&format!(
                                        "[nex][refract] panel {pw}x{ph} re-presented in {}ms (held)",
                                        t0.elapsed().as_millis()
                                    ));
                                }
                            }
                            Err(error) => crate::logging::warn(&format!(
                                "[nex][refract] resize re-present failed: {error}"
                            )),
                        }
                    }
                }
            }
        }
        let eng = match engine.as_mut() {
            Some(e) => e,
            None => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        // Capture ensure (per monitor; restarts on move/ACCESS_LOST).
        let monitor_now = unsafe {
            MonitorFromWindow(HWND(main_sys as *mut std::ffi::c_void), MONITOR_DEFAULTTONEAREST)
        };
        let need_capture = match &capture {
            None => true,
            Some(c) => c.monitor != monitor_now,
        };
        if need_capture {
            capture = None;
            match Capture::start(&eng.device, monitor_now) {
                Ok(c) => {
                    crate::logging::info(&format!(
                        "[nex][refract] capture live {}x{}px",
                        c.frame_w, c.frame_h
                    ));
                    capture = Some(c);
                    staged = false;
                }
                Err(error) => {
                    crate::logging::warn(&format!("[nex][refract] capture start failed: {error}"));
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }
        let cap = match capture.as_mut() {
            Some(c) => c,
            None => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        // Poll one frame (nonblocking).
        let mut info: windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO =
            unsafe { std::mem::zeroed() };
        let mut resource: Option<IDXGIResource> = None;
        match unsafe {
            cap.duplication
                .AcquireNextFrame(0, &mut info as *mut _, &mut resource as *mut _)
        } {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(e) => {
                crate::logging::warn(&format!("[nex][refract] acquire failed: {e:?} — restarting capture"));
                capture = None;
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            Ok(()) => last_desktop_frame = Some(Instant::now()),
        }
        n_cap += 1;
        if info.ProtectedContentMaskedOut.0 != 0 {
            if !cap.protected_logged {
                cap.protected_logged = true;
                crate::logging::warn(
                    "[nex][refract] protected pixels are masked by Windows; presenting the rest of each frame",
                );
            }
        }
        // FPS cap: skip upload/draw but always release.
        if Instant::now().duration_since(last_present) < Duration::from_millis(MIN_FRAME_MS as u64) {
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let pad = BLUR_PAD as i32;
        let (bx, by, bw, bh) = clamp_panel_box(
            mx - pad, my - pad, pw + BLUR_PAD * 2, ph + BLUR_PAD * 2,
            cap.origin_x, cap.origin_y, cap.frame_w, cap.frame_h,
        );
        if bw != pw + BLUR_PAD * 2 || bh != ph + BLUR_PAD * 2 {
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            continue;
        }
        let frame_ok = (|| -> Result<(u64, u64, Vec<u8>), String> {
            let tex: ID3D11Texture2D = resource
                .as_ref()
                .ok_or("empty frame resource")?
                .cast()
                .map_err(|e| format!("frame QI texture failed: {e:?}"))?;
            // Full-frame mirror into staging: the latest desktop image
            // stays on the GPU, so a later resize crops the new box
            // exactly (no stretched stale frame) without waiting for a
            // desktop delta that may never come on a static screen.
            unsafe {
                eng.context.CopySubresourceRegion(
                    &cap.staging, 0, 0, 0, 0, &tex, 0, None,
                );
            }
            let t0 = Instant::now();
            let pixels = crop_staging(
                &eng.context,
                &eng.device,
                &cap.staging,
                cap.frame_w,
                cap.frame_h,
                bx,
                by,
                bw,
                bh,
            )?;
            let us = t0.elapsed().as_micros() as u64;
            let render_t = Instant::now();
            let flipped = material.update(&pixels);
            // A hidden poll decides the material only: no upload, no draw.
            if !warm {
                eng.present_frame(&pixels, bw, bh, &material, bend)?;
            }
            if flipped || palette_sent != Some(material.light) {
                palette_sent = Some(material.light);
                let _ = proxy.send_event(UiCommand::GlassPalette(material.light));
            }
            let render_us = render_t.elapsed().as_micros() as u64;
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            Ok((us, render_us, pixels))
        })();
        match frame_ok {
            Ok((us, frame_render_us, pixels)) => {
                copy_us += us;
                render_us += frame_render_us;
                n_fail = 0;
                staged = true;
                last_frame = Some((pixels, bw, bh));
                if !warm {
                    n_pre += 1;
                    last_present = Instant::now();
                }
                if !ready.swap(true, Ordering::AcqRel) {
                    let _ = proxy.send_event(UiCommand::GlassReady);
                }
                if prewarm_pending {
                    prewarm_pending = false;
                    if !visible {
                        crate::logging::info("[nex][refract] first frame prewarmed while hidden");
                    }
                }
            }
            Err(error) => {
                let _ = unsafe { cap.duplication.ReleaseFrame() };
                n_fail += 1;
                crate::logging::warn(&format!("[nex][refract] frame failed: {error}"));
                if n_fail >= 3 {
                    crate::logging::warn(
                        "[nex][refract] 3 straight frame failures — full engine+capture reset",
                    );
                    capture = None;
                    engine = None;
                    n_fail = 0;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        if stat_t.elapsed() >= Duration::from_secs(1) {
            let avg = if n_pre > 0 { copy_us / n_pre } else { 0 };
            let render_avg = if n_pre > 0 { render_us / n_pre } else { 0 };
            crate::logging::info(&format!(
                "[nex][refract] captured/s={n_cap} presented/s={n_pre} copy_avg={avg}us render_avg={render_avg}us"
            ));
            stat_t = Instant::now();
            n_cap = 0;
            n_pre = 0;
            copy_us = 0;
            render_us = 0;
        }
    }
}

impl Capture {
    fn start(device: &ID3D11Device, monitor: HMONITOR) -> Result<Capture, String> {
        // Independent factory (not the device's parent): enumerate every
        // adapter/output, duplicate the first matching output that takes
        // our device. Survives multi-GPU boxes where device and output
        // live on different adapters.
        let factory: IDXGIFactory = unsafe {
            CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .map_err(|e| format!("capture factory failed: {e:?}"))?
        };
        let mut duplication: Option<IDXGIOutputDuplication> = None;
        let mut ai = 0u32;
        'adapters: loop {
            let adapter: IDXGIAdapter = match unsafe { factory.EnumAdapters(ai) } {
                Ok(a) => a,
                Err(_) => break,
            };
            ai += 1;
            let mut oi = 0u32;
            loop {
                let output: IDXGIOutput = match unsafe { adapter.EnumOutputs(oi) } {
                    Ok(o) => o,
                    Err(_) => break,
                };
                oi += 1;
                let matches = unsafe {
                    output
                        .GetDesc()
                        .map(|desc| desc.Monitor == monitor)
                        .unwrap_or(false)
                };
                if !matches {
                    continue;
                }
                let output1: IDXGIOutput1 = match output.cast() {
                    Ok(o) => o,
                    Err(error) => {
                        crate::logging::warn(&format!(
                            "[nex][refract] output QI IDXGIOutput1 failed: {error:?}"
                        ));
                        continue;
                    }
                };
                match unsafe { output1.DuplicateOutput(device) } {
                    Ok(dup) => {
                        duplication = Some(dup);
                        break 'adapters;
                    }
                    Err(error) => crate::logging::warn(&format!(
                        "[nex][refract] DuplicateOutput refused (adapter mismatch?): {error:?}"
                    )),
                }
            }
        }
        let duplication = duplication.ok_or("no output on the panel monitor duplicated")?;
        let mode = unsafe { duplication.GetDesc() };
        let (fw, fh) = (mode.ModeDesc.Width, mode.ModeDesc.Height);
        if fw == 0 || fh == 0 {
            return Err("duplication mode is empty".to_string());
        }
        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut mi as *mut _) }.0 == 0 {
            return Err("GetMonitorInfoW failed".to_string());
        }
        let staging: ID3D11Texture2D = unsafe {
            let mut tex: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: fw,
                        Height: fh,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    },
                    None,
                    Some(&mut tex as *mut _),
                )
                .map_err(|e| format!("staging texture failed: {e:?}"))?;
            tex.ok_or("staging texture came back empty")?
        };
        Ok(Capture {
            duplication,
            staging,
            frame_w: fw,
            frame_h: fh,
            origin_x: mi.rcMonitor.left,
            origin_y: mi.rcMonitor.top,
            monitor,
            protected_logged: false,
        })
    }
}

/// Crop a panel box out of the staging mirror: exact desktop pixels,
/// never stretched. Out-of-range crops fail (callers fall back) rather
/// than surfacing D3D11 range faults later as device-removed.
fn crop_staging(
    context: &ID3D11DeviceContext,
    device: &ID3D11Device,
    staging: &ID3D11Texture2D,
    fw: u32,
    fh: u32,
    bx: u32,
    by: u32,
    bw: u32,
    bh: u32,
) -> Result<Vec<u8>, String> {
    if bw == 0
        || bh == 0
        || bx.checked_add(bw).map(|r| r > fw).unwrap_or(true)
        || by.checked_add(bh).map(|r| r > fh).unwrap_or(true)
    {
        return Err(format!("box {bx},{by} {bw}x{bh} outside staging {fw}x{fh}"));
    }
    let mut mapped: D3D11_MAPPED_SUBRESOURCE = unsafe { std::mem::zeroed() };
    if let Err(e) = unsafe {
        context.Map(
            staging,
            0,
            D3D11_MAP_READ,
            0,
            Some(&mut mapped as *mut _),
        )
    } {
        // Name the removal cause: TDR/hung/driver behave the
        // same at Map, and the reason decides the next move.
        let reason = match unsafe { device.GetDeviceRemovedReason() } {
            Ok(()) => "healthy".to_string(),
            Err(reason) => format!("0x{:08X}", reason.code().0 as u32),
        };
        return Err(format!("staging Map failed: {e:?} (removed reason={reason})"));
    }
    let mut pixels = vec![0u8; (bw * bh * 4) as usize];
    let base = mapped.pData as *const u8;
    let pitch = mapped.RowPitch as usize;
    for row in 0..bh as usize {
        unsafe {
            std::ptr::copy_nonoverlapping(
                base.add(by as usize * pitch + row * pitch + bx as usize * 4),
                pixels.as_mut_ptr().add(row * bw as usize * 4),
                bw as usize * 4,
            );
        }
    }
    unsafe {
        context.Unmap(staging, 0);
    }
    Ok(pixels)
}

/// Backdrop-driven material: smoothed luminance picks a light or dark
/// surface (with hysteresis), and the tint always pushes the surface
/// toward that material's own extreme so text keeps its contrast.
struct Material {
    /// Smoothed backdrop luminance; None until the first frame.
    luma: Option<f32>,
    /// Light surface + dark text (true) or dark surface + light text.
    light: bool,
    /// Consecutive frames the backdrop has sat past the switch.
    dwell: u32,
    /// The next frame decides outright - no smoothing, no dwell. Set by
    /// every fresh show: dwell guards against the backdrop changing
    /// while the panel is open, and frames only arrive when the desktop
    /// changes, so settling on open would leave the page palette
    /// seconds behind the glass.
    fresh: bool,
}

impl Material {
    /// Fold in one captured frame. Returns true when the material
    /// flipped - the caller tells the page, which swaps its palette.
    fn update(&mut self, pixels: &[u8]) -> bool {
        let luma = frame_luma(pixels);
        let ema = match self.luma {
            Some(prev) if !self.fresh => prev + (luma - prev) * LUMA_EMA,
            _ => luma,
        };
        let settle = self.fresh;
        self.fresh = false;
        self.luma = Some(ema);
        let switch = if self.light {
            MATERIAL_SWITCH - MATERIAL_HYSTERESIS
        } else {
            MATERIAL_SWITCH
        };
        if (ema > switch) == self.light {
            self.dwell = 0;
            return false;
        }
        // Past the switch - but only flip once the backdrop has stayed
        // there a moment, so a scene cut or a fast scroll cannot flash
        // the palette.
        if !settle {
            self.dwell += 1;
            if self.dwell < MATERIAL_DWELL {
                return false;
            }
        }
        self.dwell = 0;
        self.light = ema > switch;
        true
    }

    /// Wash (r, g, b, a) over the current backdrop. Colour and alpha ease
    /// across the hysteresis band rather than stepping when the palette
    /// flips, so the composite under the text never jumps; `light` only
    /// picks the page palette.
    fn wash(&self) -> (f32, f32, f32, f32) {
        let luma = self.luma.unwrap_or(0.5).clamp(0.0, 1.0);
        let floor = MATERIAL_SWITCH - MATERIAL_HYSTERESIS;
        let t = ((luma - floor) / (2.0 * MATERIAL_HYSTERESIS)).clamp(0.0, 1.0);
        // A dark material washes a bright backdrop and follows the
        // backdrop; a light one is the mirror of it. Blend both across
        // the band so the crossover is continuous.
        let alpha = |push: f32| TINT_MIN_ALPHA + (TINT_MAX_ALPHA - TINT_MIN_ALPHA) * push;
        let a = alpha(1.0 - luma) * (1.0 - t) + alpha(luma) * t;
        let mix = |dark: f32, light: f32| dark + (light - dark) * t;
        (
            mix(TINT_DARK.0, TINT_LIGHT.0),
            mix(TINT_DARK.1, TINT_LIGHT.1),
            mix(TINT_DARK.2, TINT_LIGHT.2),
            a,
        )
    }
}

/// Average luminance (BT.709) of a BGRA frame, sampled every 16th
/// pixel: plenty for a tint response, ~free next to the crop.
fn frame_luma(pixels: &[u8]) -> f32 {
    const STEP_BYTES: usize = 64; // 16 BGRA pixels
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    let mut i = 0usize;
    while i + 2 < pixels.len() {
        let (b, g, r) = (
            pixels[i] as f32,
            pixels[i + 1] as f32,
            pixels[i + 2] as f32,
        );
        sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
        n += 1.0;
        i += STEP_BYTES;
    }
    if n == 0.0 {
        // No pixels (shouldn't happen): mid gray keeps the tint neutral.
        0.5
    } else {
        (sum / n / 255.0).clamp(0.0, 1.0)
    }
}

/// Nearest-neighbor BGRA stretch: fallback only, when the staging
fn stretch_bgra_nearest(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || src.len() != sw * sh * 4 {
        return out;
    }
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            out[(y * dw + x) * 4..(y * dw + x) * 4 + 4]
                .copy_from_slice(&src[(sy * sw + sx) * 4..(sy * sw + sx) * 4 + 4]);
        }
    }
    out
}

/// Panel rect minus monitor origin, clamped to the frame. Zero area
/// when the panel sits off the duplicated output.
fn clamp_panel_box(
    mx: i32,
    my: i32,
    mw: u32,
    mh: u32,
    ox: i32,
    oy: i32,
    fw: u32,
    fh: u32,
) -> (u32, u32, u32, u32) {
    let x0 = (mx - ox).max(0).min(fw as i32) as u32;
    let y0 = (my - oy).max(0).min(fh as i32) as u32;
    let x1 = (mx - ox + mw as i32).max(0).min(fw as i32) as u32;
    let y1 = (my - oy + mh as i32).max(0).min(fh as i32) as u32;
    (
        x0,
        y0,
        x1.saturating_sub(x0),
        y1.saturating_sub(y0),
    )
}

unsafe extern "system" fn glass_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn last_error_code() -> u32 {
    unsafe { windows::Win32::Foundation::GetLastError().0 }
}

pub(crate) fn physical_px(logical: f64, scale: f64) -> u32 {
    ((logical * scale).round() as u32).max(1)
}

/// Align a max-size glass window to the main panel: same top-left,
/// ordered directly below it, never activating. Shared by the host
/// (grow path) and the worker (position follows).
fn align_glass_to_main(glass: HWND, main_sys: isize, w: u32, h: u32) -> Option<(u32, u32)> {
    let (x, y) = main_origin(main_sys)?;
    let main = HWND(main_sys as *mut std::ffi::c_void);
    if let Err(error) = unsafe {
        SetWindowPos(
            glass,
            Some(main),
            x,
            y,
            w as i32,
            h as i32,
            SWP_NOACTIVATE,
        )
    } {
        crate::logging::warn(&format!("[nex][refract] align failed: {error:?}"));
        return None;
    }
    Some((w, h))
}

/// Client origin on screen. Size deliberately excluded: tao applies
/// set_inner_size async, so the live rect lags the request.
fn main_origin(main_sys: isize) -> Option<(i32, i32)> {
    let hwnd = HWND(main_sys as *mut std::ffi::c_void);
    let mut origin = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(hwnd, &mut origin) }.0 == 0 {
        return None;
    }
    Some((origin.x, origin.y))
}

fn main_rect(main_sys: isize) -> Option<(i32, i32, u32, u32)> {
    // Client rect mapped to screen, not extended frame bounds: DWM's
    // extended bounds can include invisible shadow margins (asymmetric
    // — observed as glass spilling past the panel's left edge), and
    // GetWindowRect has the same shadow problem in reverse. The client
    // area is exactly the visible panel in every mode.
    let hwnd = HWND(main_sys as *mut std::ffi::c_void);
    let mut client: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return None;
    }
    let mut origin = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(hwnd, &mut origin) }.0 == 0 {
        return None;
    }
    let (w, h) = (
        (client.right - client.left).max(1),
        (client.bottom - client.top).max(1),
    );
    Some((origin.x, origin.y, w as u32, h as u32))
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_panel_box, desktop_recent, frame_luma, pass_plan, stretch_bgra_nearest, swell_bend,
        swell_pass_due, swell_wants_mirror, Material, Pass, ShowCycle, DESKTOP_RECENT_MS,
        HIDDEN_POLL_MS, MATERIAL_HYSTERESIS, MATERIAL_SWITCH, MIN_FRAME_MS, SWELL_MS,
        TINT_MAX_ALPHA, TINT_MIN_ALPHA,
    };

    fn bgra(b: u8, g: u8, r: u8, pixels: usize) -> Vec<u8> {
        (0..pixels).flat_map(|_| [b, g, r, 255]).collect()
    }

    fn gray(level: u8, pixels: usize) -> Vec<u8> {
        bgra(level, level, level, pixels)
    }

    /// Feed frames at one level; returns how many flips were reported.
    fn warm(m: &mut Material, level: u8, frames: usize) -> usize {
        (0..frames).filter(|_| m.update(&gray(level, 64))).count()
    }

    #[test]
    fn luma_bounds_and_weighting() {
        assert_eq!(frame_luma(&[]), 0.5);
        assert!(frame_luma(&bgra(0, 0, 0, 4096)) < 0.01);
        assert!(frame_luma(&bgra(255, 255, 255, 4096)) > 0.99);
        // BT.709: green reads brighter than an equal-valued blue.
        assert!(frame_luma(&bgra(0, 255, 0, 4096)) > frame_luma(&bgra(255, 0, 0, 4096)));
        // The tint range is monotone and inside the brush's legal range.
        assert!(0.0 < TINT_MIN_ALPHA && TINT_MIN_ALPHA < TINT_MAX_ALPHA && TINT_MAX_ALPHA <= 1.0);
    }

    #[test]
    fn material_follows_the_backdrop() {
        let mut m = Material {
            luma: None,
            light: false,
            dwell: 0,
            fresh: false,
        };
        // A bright scene turns the surface light — reported exactly once.
        assert_eq!(warm(&mut m, 255, 60), 1);
        assert!(m.light);
        // A dark scene takes it back — also exactly once.
        assert_eq!(warm(&mut m, 0, 60), 1);
        assert!(!m.light);
    }

    #[test]
    fn swell_runs_out_and_never_overshoots() {
        assert_eq!(swell_bend(0.0), 0.0);
        // Monotone rise, easing off: the second half adds less than the
        // first.
        let half = swell_bend(SWELL_MS * 0.5);
        assert!(half > 0.5);
        assert!(1.0 - half < half - swell_bend(0.0));
        // Clamped: a stalled loop cannot bend past the reference scale.
        assert_eq!(swell_bend(SWELL_MS), 1.0);
        assert_eq!(swell_bend(SWELL_MS * 5.0), 1.0);
    }

    /// Luminance of the glass: the wash composited over the backdrop.
    fn composite(wash: (f32, f32, f32, f32), luma: f32) -> f32 {
        let tint = 0.2126 * wash.0 + 0.7152 * wash.1 + 0.0722 * wash.2;
        wash.3 * tint + (1.0 - wash.3) * luma
    }

    #[test]
    fn wash_stays_in_band_and_crosses_the_switch_smoothly() {
        let at = |luma: f32| Material {
            luma: Some(luma),
            light: false,
            dwell: 0,
            fresh: false,
        }
        .wash();
        for luma in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let a = at(luma).3;
            assert!(
                (TINT_MIN_ALPHA..=TINT_MAX_ALPHA).contains(&a),
                "alpha {a} out of band at {luma}"
            );
        }
        // The palette flips at the switch: the glass itself must not.
        let lo = MATERIAL_SWITCH - 0.001;
        let hi = MATERIAL_SWITCH + 0.001;
        let jump = (composite(at(hi), hi) - composite(at(lo), lo)).abs();
        assert!(jump < 0.01, "wetness jumped {jump} across the switch");
    }

    #[test]
    fn dwell_swallows_transient_backdrops() {
        // One bright frame past the switch counts, but does not flip;
        // the next frame back under the switch resets the count.
        let mut m = Material {
            luma: Some(0.54),
            light: false,
            dwell: 0,
            fresh: false,
        };
        assert!(!m.update(&gray(255, 64)));
        assert!(!m.light);
        assert_eq!(m.dwell, 1, "a crossing frame starts the dwell count");
        assert!(!m.update(&gray(20, 64)));
        assert_eq!(m.dwell, 0, "a frame back under the switch resets it");
        assert!(!m.light);
    }

    #[test]
    fn a_fresh_show_decides_on_its_first_frame() {
        // Panel was open over a dark backdrop, reopens over a bright one:
        // the first frame must flip the palette, not the eighth. Frames
        // only arrive when the desktop changes, so waiting to settle
        // would leave the page palette seconds behind the glass.
        let mut m = Material {
            luma: Some(0.1),
            light: false,
            dwell: 0,
            fresh: true,
        };
        assert!(m.update(&gray(255, 64)));
        assert!(m.light);
        // Settling is back on for the frames after the open.
        let mut m = Material {
            luma: Some(0.54),
            light: false,
            dwell: 0,
            fresh: false,
        };
        assert!(!m.update(&gray(255, 64)), "dwell must still gate later flips");
        assert!(!m.light);
    }

    #[test]
    fn material_hysteresis_holds_between_thresholds() {
        // Mid-gray sits above the light-stay floor and below the
        // dark-to-light switch: neither material should flip on it.
        let mid = (((MATERIAL_SWITCH + MATERIAL_SWITCH - MATERIAL_HYSTERESIS) / 2.0) * 255.0) as u8;
        let mut m = Material {
            luma: None,
            light: false,
            dwell: 0,
            fresh: false,
        };
        assert_eq!(warm(&mut m, mid, 300), 0);
        assert!(!m.light);
        assert_eq!(warm(&mut m, 255, 300), 1);
        assert!(m.light);
        assert_eq!(warm(&mut m, mid, 300), 0, "hysteresis must hold the material");
        assert!(m.light);
        assert_eq!(warm(&mut m, 0, 300), 1);
        assert!(!m.light);
    }

    #[test]
    fn a_show_arms_one_held_represent() {
        // A flip swapchain paints nothing until the next Present, so a
        // show arms one held re-present. Exactly one: a refresh that is
        // never consumed re-presents the mirror every pass.
        let mut c = ShowCycle { refresh: false, swell: None };
        assert!(!c.take_refresh(), "nothing armed before a show");
        c.arm();
        assert!(c.take_refresh(), "the reveal needs the held frame");
        assert!(!c.take_refresh(), "one present per show, not a loop");
        c.arm();
        assert!(c.take_refresh(), "the next show arms it again");
    }

    #[test]
    fn the_swell_starts_flat_and_ends_exactly_once() {
        // The swell drives the lens from flat to full on a static desktop.
        // It must end: a swell that stays armed re-presents forever.
        let mut c = ShowCycle { refresh: false, swell: None };
        assert_eq!(c.bend(), (1.0, false), "no show means no swell");
        assert!(!c.swelling());
        c.arm();
        assert!(c.swelling());
        let (start, running) = c.bend_at(0.0);
        assert_eq!(start, 0.0);
        assert!(running, "still running at its start");
        let (end, running) = c.bend_at(SWELL_MS);
        assert_eq!(end, 1.0);
        assert!(!running, "a full bend ends the swell");
        assert!(!c.swelling(), "and it must not stay armed");
        // A pass after the swell reads full, and cannot restart it.
        assert_eq!(c.bend_at(SWELL_MS * 3.0), (1.0, false));
        assert_eq!(c.bend(), (1.0, false));
    }

    #[test]
    fn hidden_passes_poll_the_backdrop_on_an_interval() {
        // Hidden: the material must still track the backdrop, because the
        // host pushes the show state (palette included) before it reveals
        // the panel — but not at full rate, and never a present.
        assert_eq!(pass_plan(true, false, 0), Pass::Frame, "panel is up");
        assert_eq!(
            pass_plan(false, true, 0),
            Pass::Frame,
            "the startup prewarm still owes a present"
        );
        assert_eq!(pass_plan(false, false, 0), Pass::Wait, "just polled");
        assert_eq!(pass_plan(false, false, HIDDEN_POLL_MS - 1), Pass::Wait);
        assert_eq!(pass_plan(false, false, HIDDEN_POLL_MS), Pass::Poll);
    }

    #[test]
    fn swell_passes_are_frame_paced() {
        // Swell passes re-present the held mirror; a stalled desktop must
        // not spin the loop faster than a captured frame would.
        assert!(!swell_pass_due(0));
        assert!(!swell_pass_due(MIN_FRAME_MS as u64 - 1));
        assert!(swell_pass_due(MIN_FRAME_MS as u64));
    }

    #[test]
    fn moving_content_is_decided_by_frame_age_not_by_the_last_pass() {
        // The desktop hands over a frame every 16-33ms while this loop
        // turns over every 2-5ms, so most passes acquire nothing. Reading
        // "did the last pass get a frame" therefore says the desktop is
        // still while video plays, the swell re-presents the frozen
        // mirror, and the backdrop sits still for SWELL_MS.
        assert!(desktop_recent(Some(0)));
        assert!(desktop_recent(Some(MIN_FRAME_MS as u64)));
        assert!(desktop_recent(Some(DESKTOP_RECENT_MS - 1)));
        assert!(!desktop_recent(Some(DESKTOP_RECENT_MS)));
        assert!(!desktop_recent(Some(1000)));
        // Never saw a frame (startup, dead session): not moving.
        assert!(!desktop_recent(None));
    }

    #[test]
    fn a_swell_does_not_re_present_the_mirror_over_moving_content() {
        // Static desktop: no captured frame exists to paint the swell, so
        // the mirror does. Moving desktop: the captures paint it, and a
        // mirror re-present would drop every one of them (it resets the
        // same frame gate) and freeze the backdrop for the whole swell.
        assert!(
            swell_wants_mirror(true, false),
            "a static desktop needs the mirror to animate"
        );
        assert!(
            !swell_wants_mirror(true, true),
            "wrong choice here freezes a playing backdrop for SWELL_MS"
        );
        assert!(!swell_wants_mirror(false, false));
        assert!(!swell_wants_mirror(false, true));
    }

    #[test]
    fn crop_box_clamps_to_the_frame() {
        let frame = (1920u32, 1080u32);
        // Inside the frame: exactly the box the panel asked for.
        assert_eq!(
            clamp_panel_box(100, 200, 700, 441, 0, 0, frame.0, frame.1),
            (100, 200, 700, 441)
        );
        // Offset output origin subtracts (monitor not at 0,0).
        assert_eq!(
            clamp_panel_box(100, 200, 700, 441, 100, 200, frame.0, frame.1),
            (0, 0, 700, 441)
        );
        // Flush to the left/top edge: clamped origin, shorter box — the
        // caller treats a short box as "no exact crop" and holds the last
        // frame instead of showing a half-cropped one.
        assert_eq!(
            clamp_panel_box(-8, -8, 700, 441, 0, 0, frame.0, frame.1),
            (0, 0, 692, 433)
        );
        // Same on the far side.
        assert_eq!(
            clamp_panel_box(1800, 1000, 700, 441, 0, 0, frame.0, frame.1),
            (1800, 1000, 120, 80)
        );
        // Fully off the output: zero area, never a panic or a wrap.
        assert_eq!(
            clamp_panel_box(4000, 4000, 700, 441, 0, 0, frame.0, frame.1),
            (1920, 1080, 0, 0)
        );
    }

    #[test]
    fn fallback_stretch_keeps_channels_and_size() {
        // 2x2 BGRA source stretched to 4x4: every output pixel is a copy
        // of one source pixel. A stride or channel swap here paints the
        // fallback fill as garbage instead of the last frame.
        let src = [
            1u8, 2, 3, 255, /* (0,0) */ 4, 5, 6, 255, /* (1,0) */
            7, 8, 9, 255, /* (0,1) */ 10, 11, 12, 255, /* (1,1) */
        ];
        let out = stretch_bgra_nearest(&src, 2, 2, 4, 4);
        assert_eq!(out.len(), 4 * 4 * 4);
        let px = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        assert_eq!(px(0, 0), [1, 2, 3, 255]);
        assert_eq!(px(3, 0), [4, 5, 6, 255], "nearest neighbour, no filtering");
        assert_eq!(px(3, 3), [10, 11, 12, 255], "the far corner is the last source pixel");
        // A source that does not match its stated size yields a zero
        // buffer, never a partial or mis-strided copy.
        assert!(stretch_bgra_nearest(&src, 3, 2, 2, 2).iter().all(|b| *b == 0));
    }
}
