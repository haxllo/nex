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
    D2D1_CHANNEL_SELECTOR_G, D2D1_CHANNEL_SELECTOR_R,
    CLSID_D2D1DisplacementMap, CLSID_D2D1GaussianBlur, ID2D1Bitmap1,
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
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA,
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

/// Match the reference liquid-glass renderer at 96 DPI.
const BLUR_STDDEV: f32 = 3.0;
const BEND_SCALE: f32 = 10.0;
const BEZEL_MARGIN: f32 = 32.0;
const CORNER_RADIUS: f32 = 8.0;
/// Material follows the backdrop, like Apple's vibrancy: a bright scene
/// gets a light surface with dark text, a dark scene the reverse, so
/// the text colour never fights what is behind the glass. Alpha is the
/// wash at the material's own extreme; hysteresis stops flapping when
/// the backdrop sits near the switch.
const TINT_MIN_ALPHA: f32 = 0.13;
const TINT_MAX_ALPHA: f32 = 0.34;
const MATERIAL_SWITCH: f32 = 0.55;
const MATERIAL_HYSTERESIS: f32 = 0.10;
const TINT_DARK: (f32, f32, f32) = (12.0 / 255.0, 14.0 / 255.0, 20.0 / 255.0);
const TINT_LIGHT: (f32, f32, f32) = (255.0 / 255.0, 255.0 / 255.0, 255.0 / 255.0);
/// Per-frame exponential smoothing on backdrop luminance (~150ms at
/// 60fps) so a scene cut eases the tint instead of snapping it.
const LUMA_EMA: f32 = 0.10;
/// Extra desktop pixels around the panel keep the blur kernel away from
/// the capture boundary. Matches the reference renderer's 3σ + 2 pad.
const BLUR_PAD: u32 = (BLUR_STDDEV as u32) * 3 + 2;

/// Render cap: live frames present at most this often.
const MIN_FRAME_MS: u128 = 16;

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
        unsafe {
            let _ = ShowWindow(self.hwnd, if show { SW_SHOWNA } else { SW_HIDE });
        }
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
            "[nex][refract] d2d effect ready blur={BLUR_STDDEV} bend={BEND_SCALE} margin={BEZEL_MARGIN}px"
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
            self.displace.SetInput(0, &blur_output(&self.blur)?, true);
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
    ) -> Result<(), String> {
        self.set_tint(material.alpha(), material.light);
        self.ingest_frame(pixels, w, h)?;
        self.render_frame()
    }

    /// Blend amount + colour for a backdrop luminance and text theme.
    fn set_tint(&self, alpha: f32, dark: bool) {
        let (r, g, b) = if dark { TINT_DARK } else { TINT_LIGHT };
        unsafe {
            self.tint_brush.SetColor(&D2D1_COLOR_F {
                r,
                g,
                b,
                a: alpha.clamp(0.0, 1.0),
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
                &blur_output(displace)?,
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
fn blur_output(blur: &ID2D1Effect) -> Result<ID2D1Image, String> {
    unsafe {
        blur.GetOutput()
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
    let mut stat_t = Instant::now();
    let (mut n_cap, mut n_pre, mut copy_us, mut render_us) = (0u64, 0u64, 0u64, 0u64);
    // Last panel position the glass was aligned to. Re-align if the
    // window position changes while the capture worker is active.
    let mut last_glass_at = (0i32, 0i32);
    // Full-frame staging mirror valid yet (fresh capture starts
    // uninitialized; first mirrored frame arms it).
    let mut staged = false;
    // Backdrop-driven material (surface appearance + page palette).
    // Seeded dark; the host sends the text theme on create and on each
    // show, and every captured frame re-decides from the backdrop.
    let mut material = Material {
        luma: None,
        light: false,
    };
    // Last material the page was told about; None forces a re-send.
    let mut palette_sent: Option<bool> = None;
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
                // Theme only seeds the material; the backdrop overrides
                // it per frame. A static desktop may not send another
                // frame, so apply the seed tint immediately.
                material.light = !dark;
                if let Some(e) = engine.as_ref() {
                    e.set_tint(material.alpha(), material.light);
                }
            }
        }
        if main_sys == 0 {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if !visible && !prewarm_pending {
            // Hidden: hold the duplication session + staging mirror (no
            // acquire/render) so the next show presents a fresh frame
            // within milliseconds instead of paying a capture restart.
            // ponytail: one idle session; no polling cost while hidden.
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
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
            if rebuilt {
                // Fresh zeros after rebuild: re-present immediately so a
                // static desktop (no new duplication frame) never leaves
                // the panel black. Exact crop from the staging mirror —
                // never a stretched old frame; stretch is only the
                // fallback when the mirror isn't armed yet.
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
                    match e.present_frame(&fresh, fw, fh, &material) {
                        Ok(()) => {
                            n_pre += 1;
                            last_present = Instant::now();
                            last_frame = Some((fresh, fw, fh));
                            crate::logging::info(&format!(
                                "[nex][refract] panel {pw}x{ph} rebuilt+re-presented in {}ms",
                                t0.elapsed().as_millis()
                            ));
                        }
                        Err(error) => crate::logging::warn(&format!(
                            "[nex][refract] resize re-present failed: {error}"
                        )),
                    }
                } else if let Some((prev, cw, ch)) = last_frame.as_ref() {
                    let (ew, eh) = (pw + BLUR_PAD * 2, ph + BLUR_PAD * 2);
                    if prev.len() == (*cw as usize) * (*ch as usize) * 4 {
                        let stretched = stretch_bgra_nearest(prev, *cw, *ch, ew, eh);
                        match e.present_frame(&stretched, ew, eh, &material) {
                            Ok(()) => {
                                n_pre += 1;
                                last_present = Instant::now();
                                last_frame = Some((stretched, ew, eh));
                                crate::logging::info(&format!(
                                    "[nex][refract] panel {pw}x{ph} rebuilt+re-presented in {}ms",
                                    t0.elapsed().as_millis()
                                ));
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
            Ok(()) => {}
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
            eng.present_frame(&pixels, bw, bh, &material)?;
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
                n_pre += 1;
                n_fail = 0;
                last_present = Instant::now();
                staged = true;
                last_frame = Some((pixels, bw, bh));
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
}

impl Material {
    /// Fold in one captured frame. Returns true when the material
    /// flipped — the caller tells the page, which swaps its palette.
    fn update(&mut self, pixels: &[u8]) -> bool {
        let luma = frame_luma(pixels);
        let ema = match self.luma {
            Some(prev) => prev + (luma - prev) * LUMA_EMA,
            None => luma,
        };
        self.luma = Some(ema);
        let before = self.light;
        let switch = if self.light {
            MATERIAL_SWITCH - MATERIAL_HYSTERESIS
        } else {
            MATERIAL_SWITCH
        };
        self.light = ema > switch;
        before != self.light
    }

    /// Tint blend for the current material.
    fn alpha(&self) -> f32 {
        let luma = self.luma.unwrap_or(0.5);
        let push = if self.light { luma } else { 1.0 - luma };
        TINT_MIN_ALPHA + (TINT_MAX_ALPHA - TINT_MIN_ALPHA) * push.clamp(0.0, 1.0)
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
        frame_luma, Material, MATERIAL_HYSTERESIS, MATERIAL_SWITCH, TINT_MAX_ALPHA,
        TINT_MIN_ALPHA,
    };

    fn bgra(b: u8, g: u8, r: u8, pixels: usize) -> Vec<u8> {
        (0..pixels).flat_map(|_| [b, g, r, 255]).collect()
    }

    fn gray(level: u8, pixels: usize) -> Vec<u8> {
        bgra(level, level, level, pixels)
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
        };
        // First frame decides immediately; the surface turns light over
        // a bright scene and reports the flip exactly once.
        assert!(m.update(&gray(255, 256)));
        assert!(m.light);
        let mut flips = 0;
        for _ in 0..200 {
            flips += usize::from(m.update(&gray(255, 256)));
        }
        assert_eq!(flips, 0, "steady backdrop must not re-report flips");
        // Dark scene takes it back (EMA needs a few frames to converge).
        for _ in 0..200 {
            m.update(&gray(0, 256));
        }
        assert!(!m.light);
        // Every material keeps the tint inside the tuned band.
        for level in [0u8, 64, 128, 200, 255] {
            for light in [true, false] {
                let mut m = Material {
                    luma: None,
                    light,
                };
                m.update(&gray(level, 64));
                let a = m.alpha();
                assert!(
                    (TINT_MIN_ALPHA..=TINT_MAX_ALPHA).contains(&a),
                    "alpha {a} out of band at level {level}"
                );
            }
        }
    }

    #[test]
    fn material_hysteresis_holds_between_thresholds() {
        // Mid-gray sits above the light-stay floor and below the
        // dark-to-light switch: neither material should flip on it.
        let mid = (((MATERIAL_SWITCH + MATERIAL_SWITCH - MATERIAL_HYSTERESIS) / 2.0) * 255.0) as u8;
        let mut m = Material {
            luma: None,
            light: false,
        };
        for _ in 0..300 {
            assert!(!m.update(&gray(mid, 64)));
        }
        assert!(!m.light);
        for _ in 0..300 {
            m.update(&gray(255, 64));
        }
        assert!(m.light);
        for _ in 0..300 {
            assert!(!m.update(&gray(mid, 64)), "hysteresis must hold the material");
        }
        assert!(m.light);
        for _ in 0..300 {
            m.update(&gray(0, 64));
        }
        assert!(!m.light);
    }
}
