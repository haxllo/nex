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
    D2D1_COLORMATRIX_PROP_COLOR_MATRIX, D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT,
    D2D1_PROPERTY_TYPE_MATRIX_5X4, D2D1_PROPERTY_TYPE_BOOL,
    D2D1_COMPOSITE_PROP_MODE,
    D2D1_CHANNEL_SELECTOR_G, D2D1_CHANNEL_SELECTOR_R,
    CLSID_D2D1DisplacementMap, CLSID_D2D1GaussianBlur, CLSID_D2D1ColorMatrix,
    CLSID_D2D1AlphaMask, CLSID_D2D1Composite, ID2D1Bitmap1,
    ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Factory1, ID2D1Image,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_DESTINATION_IN, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    D2D1_COMPOSITE_MODE_PLUS,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
};
use windows_numerics::{Matrix3x2, Vector2};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter,
    IDXGIDevice, IDXGIFactory, IDXGIFactory2, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, RegisterClassW,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SW_SHOWNA,
    WDA_EXCLUDEFROMCAPTURE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Foundation::POINT;
use windows::{core::w, Win32::System::LibraryLoader::GetModuleHandleW};

/// Refraction is on by default; an explicit zero keeps the Acrylic fallback.
pub fn enabled() -> bool {
    !glass_failed()
        && std::env::var("NEX_REFRACT_LAB")
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true)
}

/// Set once the worker decides the D2D engine is not coming back (repeated
/// build/rebuild failures). The host then renders the page opaque and parks
/// the glass layer; the worker keeps retrying behind the scenes. Process
/// lifetime: recovery for a poisoned device needs a restart, which is
/// exactly what a persistent failure means.
static GLASS_FAILED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn glass_failed() -> bool {
    GLASS_FAILED.load(std::sync::atomic::Ordering::Acquire)
}

fn mark_glass_failed() {
    GLASS_FAILED.store(true, std::sync::atomic::Ordering::Release);
}

/// Wide diffusion branch; the center primarily uses the tight branch.
const BLUR_STDDEV: f32 = 8.0;
const CENTER_BLUR_STDDEV: f32 = 3.5;
/// The RGB sample positions differ by at most 0.4 px at rest. This is
/// dispersion in the lens, not a colored outline painted over the text.
const ABERRATION: f32 = 0.07;
/// Backdrop vibrancy: the refracted image is lifted slightly so it does
/// not read as greyed out behind the tint.
const SATURATION: f32 = 1.2;
/// Stronger than the original 14: the user wanted the backdrop to visibly
/// converge into the rim, not merely soften at it. The bend is still
/// confined to the bevel, so the interior stays a window.
const BEND_SCALE: f32 = 24.0;
/// Show spring settling deadline. No frame-count-dependent animation.
/// One shot per show; nothing animates while hidden.
const SWELL_MS: f32 = 420.0;
/// Lens strength on the first presented frame of a show. The reveal lands
/// ~40ms after the show is armed, so a spring starting at flat would read
/// as the edge bend arriving late over an already-visible panel — the
/// error was noticeable while the strip was still ramping in. Starting
/// most of the way open keeps the settle, drops the wait.
const SWELL_START: f32 = 0.85;
const BEZEL_MARGIN: f32 = 40.0;
/// Thickness tail: a real pane bends the backdrop across its whole body,
/// not only inside the 32 px bevel. The extra inward sample decays with
/// distance, so the rim stays the strong lens and the interior picks up a
/// sub-pixel draw toward the nearest edge (glass thickness, not a zoom).
const SLAB_BEND: f32 = 0.30;
const SLAB_DECAY: f32 = 35.0;
/// Device-pixel growth for the main window's native region. The page's
/// own clip-path is antialiased; letting the region sit a pixel outside
/// it keeps GDI's hard polygon edge from shaving that antialiasing off.
const SILHOUETTE_GROW_PX: f64 = 1.0;
pub(crate) const CORNER_RADIUS: f32 = 12.0;
/// Statistics downsample size: enough for the 4x4 tint grid and a stable
/// luminance average, small enough that the readback is noise.
const SAMPLE_W: u32 = 16;
const SAMPLE_H: u32 = 16;
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

/// Room outside the panel for the contact shadow. A pane can only cast one
/// if the surface it is drawn on reaches past its own edges, so the glass
/// window, its backbuffer and the visual all carry this margin; the material
/// still starts at `(SHADOW_MARGIN, SHADOW_MARGIN)` so the panel stays
/// aligned with the page's client area.
const SHADOW_MARGIN: u32 = 44;
/// Contact shadow falloff radius and peak alpha, symmetric on all four
/// sides. Physical px, like BLUR_PAD, so the cue reads the same at every
/// display scale.
const SHADOW_BLUR: f32 = 26.0;
const SHADOW_STRENGTH: f32 = 0.30;

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
    scale: Option<f64>,
}

/// Host-facing handle. All methods are UI-thread window ops plus a
/// config send; every D3D/D2D object lives on the worker.
pub struct GlassLayer {
    window: GlassWindow,
    tx: Sender<GlassConfig>,
    ready: Arc<AtomicBool>,
    main_sys: isize,
    visible: std::cell::Cell<bool>,
    /// The cloak is currently lifted. Reveals are idempotent (one per show)
    /// and re-armed by `set_visible(false)`.
    revealed: std::cell::Cell<bool>,
}

struct GlassWindow {
    hwnd: HWND,
    /// Physical HWND size. The glass lives at max-panel size permanently;
    /// the composition visual transform + backbuffer alpha mask track the
    /// panel, so DWM and the swapchain never see a resize in steady state
    /// (no flash, no lag).
    hwnd_size: std::cell::Cell<(u32, u32)>,
    /// Last panel size/scale applied to the main window's native region.
    last_region: std::cell::Cell<Option<(u32, u32, u64)>>,
    /// Size of the last frame the renderer presented. The reveal waits for
    /// this to match `last_region`: a frame for the previous panel size
    /// leaves the rows the page just grew into holding older pixels.
    last_frame: std::cell::Cell<Option<(u32, u32)>>,
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
                        Some(GlassLayer {
                            window,
                            tx,
                            ready,
                            main_sys,
                            visible: std::cell::Cell::new(false),
                            revealed: std::cell::Cell::new(false),
                        })
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
    /// HWND stays at max-panel size; the rounded silhouette is carried in
    /// the composition backbuffer alpha, so a resize is one content rebuild
    /// — no SetWindowPos, no swapchain resize, nothing to lag a frame.
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
        self.window.last_region.set(Some((w, h, scale.to_bits())));
        if self.visible.get() {
            apply_lens_region(main_sys, w, h, scale, SILHOUETTE_GROW_PX);
        }
        let _ = self.tx.send(GlassConfig {
            visible: None,
            main_sys,
            max_wh: Some((max_w_px, max_h_px)),
            panel_wh: Some((w, h)),
            theme_dark: None,
            scale: Some(scale),
        });
    }

    /// Show/hide with the panel. Cloaking remains immediate so launch and
    /// focus restoration never wait for a visual effect. Revealing does not:
    /// the cloak lifts only once the renderer has presented a frame at the
    /// panel's current size, so the first thing seen is this backdrop.
    pub fn set_visible(&self, show: bool) {
        let was_visible = self.visible.replace(show);
        if show {
            if let Some((w, h, bits)) = self.window.last_region.get() {
                apply_lens_region(self.main_sys, w, h, f64::from_bits(bits), SILHOUETTE_GROW_PX);
            }
        } else if was_visible {
            unsafe { windows_sys::Win32::Graphics::Gdi::SetWindowRgn(self.main_sys as _, std::ptr::null_mut(), 1); }
        }
        if !show {
            self.revealed.set(false);
            // A frame from the last session is not a frame for this show:
            // forgetting its size forces the reveal to wait for a present
            // made after the panel came back.
            self.window.last_frame.set(None);
            self.window.show(false);
        }
        let _ = self.tx.send(GlassConfig {
            visible: Some(show),
            main_sys: 0,
            max_wh: None,
            panel_wh: None,
            theme_dark: None,
            scale: None,
        });
    }

    /// Tell the renderer a show is coming without revealing the window:
    /// the panel is only revealed on its first paint, and by then the
    /// worker has re-presented the held mirror, so the uncloak carries
    /// current pixels instead of the last session's.
    pub fn prepare_show(&self) {
        if let Some((w, h, bits)) = self.window.last_region.get() {
            apply_lens_region(self.main_sys, w, h, f64::from_bits(bits), SILHOUETTE_GROW_PX);
        }
        let _ = self.tx.send(GlassConfig {
            visible: Some(true),
            main_sys: 0,
            max_wh: None,
            panel_wh: None,
            theme_dark: None,
            scale: None,
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
            scale: None,
        });
    }

    /// Record that the renderer presented a frame at `w`x`h`.
    pub fn note_frame(&self, w: u32, h: u32) {
        self.window.last_frame.set(Some((w, h)));
    }

    /// Lift the cloak once a frame for the panel's *current* size has been
    /// presented. A frame for the previous size leaves the rows the page
    /// just grew into holding older pixels, so this waits for the resize
    /// pass's own ack instead. Returns true only on the call that reveals;
    /// `set_visible(false)` re-arms it.
    pub fn reveal(&self) -> bool {
        if self.revealed.get() || !self.ready.load(Ordering::Acquire) {
            return false;
        }
        match self.window.last_region.get() {
            Some((w, h, bits)) if self.window.last_frame.get() == Some((w, h)) => {
                apply_lens_region(self.main_sys, w, h, f64::from_bits(bits), SILHOUETTE_GROW_PX);
                self.revealed.set(true);
                self.window.show(true);
                crate::logging::info(&format!("[nex][refract] glass revealed {w}x{h}"));
                true
            }
            // A frame for another size: the resize pass acks again.
            _ => false,
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
    /// shape is the backbuffer alpha mask (per panel resize); the window
    /// and swapchain buffers stay max-size so neither ever resizes.
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
        // Grown by the shadow margin on every side and anchored so the panel
        // box still lands on the main window's client origin.
        let (x, y) = (x - SHADOW_MARGIN as i32, y - SHADOW_MARGIN as i32);
        let (width, height) = (
            max_w_px + SHADOW_MARGIN * 2,
            max_h_px + SHADOW_MARGIN * 2,
        );

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_NOREDIRECTIONBITMAP,
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
        // The companion HWND must not add a second system border either.
        unsafe {
            let none = 0xFFFFFFFE_u32; // DWMWA_COLOR_NONE
            windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd.0, windows_sys::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR as u32,
                &none as *const _ as *const _, std::mem::size_of_val(&none) as u32,
            );
        }
        crate::logging::info(&format!(
            "[nex][refract] glass top-level hwnd={hwnd:?} {width}x{height}px at ({x},{y})"
        ));
        Ok(GlassWindow {
            hwnd,
            hwnd_size: std::cell::Cell::new((width, height)),
            last_region: std::cell::Cell::new(None),
            last_frame: std::cell::Cell::new(None),
        })
    }

    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Grow the HWND if the max panel outgrew it (DPI/scale change).
    /// Steady-state resizes never come here: the region clips the
    /// max-size window, so no SetWindowPos churn per resize.
    fn ensure_window_max(&self, main_sys: isize, max_w: u32, max_h: u32) {
        let (max_w, max_h) = (max_w + SHADOW_MARGIN * 2, max_h + SHADOW_MARGIN * 2);
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
                x - SHADOW_MARGIN as i32,
                y - SHADOW_MARGIN as i32,
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
}

/// Frame-independent renderer layers for one panel box. Nothing here is
/// derived from the backdrop, so a box can be rebuilt and reused verbatim.
#[derive(Clone)]
struct SizedGeometry {
    map: ID2D1Bitmap1,
    lights: Vec<ID2D1Bitmap1>,
    depth_grain: ID2D1Bitmap1,
    mask: ID2D1Bitmap1,
    shadow: ID2D1Bitmap1,
}

/// Cache depth. Two is enough: hidden presents the bar box, visible the
/// settled box, and each show promotes its own box, so that pair stays
/// resident without pinning memory for every height a live resize crosses.
const GEOMETRY_CACHE_DEPTH: usize = 2;

/// Most-recently-used bounding cache keyed by (panel width, height, scale).
/// A hit promotes, an insert evicts the oldest.
struct GeometryCache<T> {
    entries: Vec<((u32, u32, u64), T)>,
}

impl<T: Clone> GeometryCache<T> {
    fn new() -> Self {
        Self { entries: Vec::new() }
    }

    fn get(&mut self, key: (u32, u32, u64)) -> Option<T> {
        let index = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(index);
        let value = entry.1.clone();
        self.entries.insert(0, entry);
        Some(value)
    }

    fn put(&mut self, key: (u32, u32, u64), value: T) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, value));
        self.entries.truncate(GEOMETRY_CACHE_DEPTH);
    }
}

/// Worker-owned renderer: every D3D/D2D object. Built once at max
/// panel size; panel-size changes rebuild content bitmaps only — the
/// buffers (and HWND) never resize, so presents never invalidate.
struct Engine {
    scale: f64,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain1,
    /// Composition tree: the backbuffer alpha carries the rounded panel
    /// silhouette, and the visual transform cancels DComp's DPI scale.
    dcomp: IDCompositionDevice,
    target: IDCompositionTarget,
    visual: IDCompositionVisual,
    d2d: ID2D1DeviceContext,
    /// Size-agnostic effect graph, built once. Resizes replace source,
    /// geometry and lighting bitmaps without rebuilding the graph.
    blur: ID2D1Effect,
    tight_blur: ID2D1Effect,
    scatter_mask: ID2D1Effect,
    masked_blur: ID2D1Effect,
    scatter: ID2D1Effect,
    /// Backdrop vibrancy: blur output lifted before the bend.
    saturate: ID2D1Effect,
    displace: ID2D1Effect,
    red_displace: ID2D1Effect,
    blue_displace: ID2D1Effect,
    channels: [ID2D1Effect; 3],
    optical_output: ID2D1Effect,
    /// Static geometry layers; pointer changes only their opacity.
    lights: Vec<ID2D1Bitmap1>,
    depth_grain: Option<ID2D1Bitmap1>,
    local_tint: Option<ID2D1Bitmap1>,
    /// Panel-sized premultiplied alpha mask: 1 inside the rounded shape,
    /// 0 outside, 1 px analytic AA. Composed with DESTINATION_IN.
    mask: Option<ID2D1Bitmap1>,
    /// Contact shadow around the panel: premultiplied black with the panel
    /// interior cleared, drawn after the mask so it only reaches outward.
    shadow: Option<ID2D1Bitmap1>,
    /// Box-keyed copies of the layer set above. Every show re-lays the panel
    /// at a height it has already used, and regenerating them costs ~40ms at
    /// 700x441 on a pass whose frame the reveal is waiting for — so a repeat
    /// box is a refcount bump, not work.
    geometry: GeometryCache<SizedGeometry>,
    tint_grid: std::cell::RefCell<Option<Vec<[f32; 4]>>>,
    light_direction: std::cell::Cell<(f32, f32, bool)>,
    source: Option<ID2D1Bitmap1>,
    /// GPU-resident backdrop crop the source bitmap wraps; captured
    /// frames are copied straight into it (no CPU round trip).
    source_tex: Option<ID3D11Texture2D>,
    /// 16x16 GPU downsample of the source, read back for material/tint
    /// statistics. One kilobyte per frame instead of the full panel.
    sample_tex: ID3D11Texture2D,
    sample_staging: ID3D11Texture2D,
    sample_target: ID2D1Bitmap1,
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
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: buf_w,
            Height: buf_h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: windows::Win32::Graphics::Dxgi::DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        // Composition, not HWND presentation: the backbuffer alpha is the
        // panel silhouette, so the rounded edge is antialiased by the
        // compositor instead of quantised by a GDI window region.
        let swapchain: IDXGISwapChain1 =
            unsafe { factory.CreateSwapChainForComposition(&device, &desc, None) }
                .map_err(|e| format!("composition swapchain failed: {e:?}"))?;
        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|e| format!("QI IDXGIDevice failed: {e:?}"))?;
        let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice(&dxgi_device) }
            .map_err(|e| format!("DCompositionCreateDevice failed: {e:?}"))?;
        let target: IDCompositionTarget = unsafe { dcomp.CreateTargetForHwnd(hwnd, true) }
            .map_err(|e| format!("composition target failed: {e:?}"))?;
        let visual: IDCompositionVisual = unsafe { dcomp.CreateVisual() }
            .map_err(|e| format!("composition visual failed: {e:?}"))?;
        unsafe {
            visual
                .SetContent(&swapchain)
                .map_err(|e| format!("composition content failed: {e:?}"))?;
            target
                .SetRoot(&visual)
                .map_err(|e| format!("composition root failed: {e:?}"))?;
            dcomp
                .Commit()
                .map_err(|e| format!("composition commit failed: {e:?}"))?;
        }
        crate::logging::info("[nex][refract] swapchain ok via composition+premultiplied");

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
            d2d.CreateEffect(&CLSID_D2D1ColorMatrix as *const _)
                .map_err(|e| format!("saturation CreateEffect failed: {e:?}"))?
        };
        set_color_matrix(&saturate, &saturation_matrix(SATURATION))?;
        unsafe {
            saturate
                .SetValue(
                    D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0 as u32,
                    D2D1_PROPERTY_TYPE_BOOL,
                    &1_i32.to_ne_bytes(),
                )
                .map_err(|e| format!("saturation SetValue failed: {e:?}"))?;
        }
        let displace = displacement_effect(&d2d)?;
        let tight_blur = create_effect(&d2d, &CLSID_D2D1GaussianBlur)?;
        set_float(&tight_blur, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0, CENTER_BLUR_STDDEV)?;
        // The map's blue channel carries a smooth wide-blur weight. RG
        // remain the signed lens normals; alpha remains fully opaque.
        let scatter_mask = create_effect(&d2d, &CLSID_D2D1ColorMatrix)?;
        let mut mask_matrix = [0.0; 20];
        // ColorMatrix premultiplies its output: use alpha only (RGB zero)
        // since AlphaMask consumes just alpha, not the mask's color.
        mask_matrix[11] = 1.0;
        set_color_matrix(&scatter_mask, &mask_matrix)?;
        let masked_blur = create_effect(&d2d, &CLSID_D2D1AlphaMask)?;
        let scatter = create_effect(&d2d, &CLSID_D2D1Composite)?;
        let red_displace = displacement_effect(&d2d)?;
        let blue_displace = displacement_effect(&d2d)?;
        let channels = [
            create_effect(&d2d, &CLSID_D2D1ColorMatrix)?,
            create_effect(&d2d, &CLSID_D2D1ColorMatrix)?,
            create_effect(&d2d, &CLSID_D2D1ColorMatrix)?,
        ];
        for (channel, effect) in channels.iter().enumerate() {
            let mut matrix = [0.0; 20];
            matrix[channel * 5] = 1.0;
            matrix[15] = 1.0;
            set_color_matrix(effect, &matrix)?;
        }
        let optical_output = create_effect(&d2d, &CLSID_D2D1Composite)?;
        unsafe {
            optical_output.SetInputCount(3).map_err(|e| format!("RGB inputs: {e:?}"))?;
            optical_output.SetValue(
                D2D1_COMPOSITE_PROP_MODE.0 as u32, D2D1_PROPERTY_TYPE_ENUM,
                &D2D1_COMPOSITE_MODE_PLUS.0.to_ne_bytes(),
            ).map_err(|e| format!("RGB composite: {e:?}"))?;
        }
        // Statistics surface: the source is box-downsampled to 16x16 on
        // the GPU, then only 1 KiB crosses to the CPU. The material and
        // tint grid never need the full captured panel.
        let sample_tex = frame_texture(
            &device,
            SAMPLE_W,
            SAMPLE_H,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
            0,
        )?;
        let sample_staging = frame_texture(
            &device,
            SAMPLE_W,
            SAMPLE_H,
            D3D11_USAGE_STAGING,
            0,
            D3D11_CPU_ACCESS_READ.0 as u32,
        )?;
        let sample_surface: IDXGISurface = sample_tex
            .cast()
            .map_err(|e| format!("sample QI surface failed: {e:?}"))?;
        let sample_target: ID2D1Bitmap1 = unsafe {
            d2d.CreateBitmapFromDxgiSurface(
                &sample_surface,
                Some(&bitmap_props(D2D1_BITMAP_OPTIONS_TARGET) as *const _),
            )
            .map_err(|e| format!("sample target failed: {e:?}"))?
        };
        let mut engine = Engine {
            scale: 1.0,
            device,
            context,
            swapchain,
            dcomp,
            target,
            visual,
            d2d,
            source: None,
            source_tex: None,
            sample_tex,
            sample_staging,
            sample_target,
            map: None,
            tint_brush,
            width: buf_w,
            height: buf_h,
            panel_w,
            panel_h,
            blur,
            tight_blur,
            scatter_mask,
            masked_blur,
            scatter,
            saturate,
            displace,
            red_displace,
            blue_displace,
            channels,
            optical_output,
            lights: Vec::new(),
            depth_grain: None,
            local_tint: None,
            mask: None,
            shadow: None,
            geometry: GeometryCache::new(),
            tint_grid: std::cell::RefCell::new(None),
            light_direction: std::cell::Cell::new((0.0, 0.0, true)),
        };
        engine.rebuild_content()?;
        Ok(engine)
    }

    /// Keep DComp in physical pixels: its visual tree is DPI-scaled by the
    /// host window, so a `1 / scale` visual transform cancels that scale.
    /// The rounded silhouette lives in the backbuffer alpha (mask bitmap),
    /// not in a hard visual clip.
    fn apply_visual(&self) -> Result<(), String> {
        let inv = 1.0 / self.scale.max(0.01) as f32;
        let matrix = Matrix3x2::scale(inv, inv);
        unsafe {
            self.visual
                .SetTransform2(&matrix)
                .map_err(|e| format!("composition transform failed: {e:?}"))?;
            self.dcomp
                .Commit()
                .map_err(|e| format!("composition commit failed: {e:?}"))?;
        }
        Ok(())
    }

    /// Rebuild the capture source and the frame-independent geometry at the
    /// current panel size. Live frames replace only the source and tint.
    fn rebuild_content(&mut self) -> Result<(), String> {
        let (w, h) = (
            self.panel_w as usize + BLUR_PAD as usize * 2,
            self.panel_h as usize + BLUR_PAD as usize * 2,
        );
        let props = bitmap_props(D2D1_BITMAP_OPTIONS_NONE);
        // Captured frames land in this texture on the GPU, so the effect
        // graph samples it directly. Clear it once: D3D leaves fresh
        // textures undefined, and a resize must not show that garbage.
        let source_tex = frame_texture(
            &self.device,
            w as u32,
            h as u32,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
            0,
        )?;
        let clear = vec![0u8; w * h * 4];
        let resource: ID3D11Resource = source_tex
            .cast()
            .map_err(|e| format!("source QI resource failed: {e:?}"))?;
        unsafe {
            self.context.UpdateSubresource(
                &resource,
                0,
                None,
                clear.as_ptr() as *const _,
                (w * 4) as u32,
                0,
            );
        }
        let source_surface: IDXGISurface = source_tex
            .cast()
            .map_err(|e| format!("source QI surface failed: {e:?}"))?;
        let source: ID2D1Bitmap1 = unsafe {
            self.d2d
                .CreateBitmapFromDxgiSurface(&source_surface, Some(&props as *const _))
                .map_err(|e| format!("source bitmap failed: {e:?}"))?
        };
        let key = (self.panel_w, self.panel_h, self.scale.to_bits());
        let geometry = match self.geometry.get(key) {
            Some(cached) => cached,
            None => {
                let built = self.build_geometry()?;
                self.geometry.put(key, built.clone());
                built
            }
        };
        let map = geometry.map.clone();
        // Re-point the shared effect graph at the new bitmaps. Effects
        // are size-agnostic; only the bitmaps track the panel.
        unsafe {
            self.blur.SetInput(0, &source, true);
            self.tight_blur.SetInput(0, &source, true);
            self.scatter_mask.SetInput(0, &map, true);
            self.masked_blur.SetInput(0, &effect_output(&self.blur)?, true);
            self.masked_blur.SetInput(1, &effect_output(&self.scatter_mask)?, true);
            // Composite consumes inputs in order: wide rim over tight center.
            self.scatter.SetInput(0, &effect_output(&self.tight_blur)?, true);
            self.scatter.SetInput(1, &effect_output(&self.masked_blur)?, true);
            self.saturate.SetInput(0, &effect_output(&self.scatter)?, true);
            for (index, lens) in [&self.red_displace, &self.displace, &self.blue_displace].iter().enumerate() {
                lens.SetInput(0, &effect_output(&self.saturate)?, true);
                lens.SetInput(1, &map, true);
                self.channels[index].SetInput(0, &effect_output(lens)?, true);
                self.optical_output.SetInput(index as u32, &effect_output(&self.channels[index])?, true);
            }
        }
        self.source = Some(source);
        self.source_tex = Some(source_tex);
        self.map = Some(map);
        self.lights = geometry.lights;
        self.depth_grain = Some(geometry.depth_grain);
        self.mask = Some(geometry.mask);
        self.shadow = Some(geometry.shadow);
        self.local_tint = Some(self.bitmap(4, 4, &[0; 64])?);
        *self.tint_grid.borrow_mut() = None;
        self.apply_visual()?;
        Ok(())
    }

    /// The frame-independent layers for the current panel box: refraction
    /// map, edge lights, depth grain, silhouette mask and contact shadow.
    /// Nothing here depends on the backdrop, so it is a pure function of
    /// (panel width, panel height, scale) and `rebuild_content` caches it.
    fn build_geometry(&self) -> Result<SizedGeometry, String> {
        let (w, h) = (
            self.panel_w as usize + BLUR_PAD as usize * 2,
            self.panel_h as usize + BLUR_PAD as usize * 2,
        );
        let mut map_bytes = vec![128u8; w * h * 4];
        for alpha in map_bytes.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        let panel_map = bezel_bytes_scaled(self.panel_w as usize, self.panel_h as usize, self.scale as f32);
        for row in 0..self.panel_h as usize {
            let dst = ((row + BLUR_PAD as usize) * w + BLUR_PAD as usize) * 4;
            let src = row * self.panel_w as usize * 4;
            map_bytes[dst..dst + self.panel_w as usize * 4]
                .copy_from_slice(&panel_map[src..src + self.panel_w as usize * 4]);
        }
        let props = bitmap_props(D2D1_BITMAP_OPTIONS_NONE);
        let map = unsafe {
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
        let layers = surface_layers(self.panel_w as usize, self.panel_h as usize, self.scale as f32);
        let mut lights = Vec::with_capacity(4);
        for bytes in &layers[..4] {
            lights.push(self.bitmap(self.panel_w, self.panel_h, bytes)?);
        }
        let depth_grain = self.bitmap(self.panel_w, self.panel_h, &layers[4])?;
        let mask = self.bitmap(
            self.panel_w,
            self.panel_h,
            &rounded_mask(self.panel_w, self.panel_h, self.scale as f32, CORNER_RADIUS),
        )?;
        let (sw, sh) = (
            self.panel_w + SHADOW_MARGIN * 2,
            self.panel_h + SHADOW_MARGIN * 2,
        );
        let shadow = self.bitmap(
            sw,
            sh,
            &shadow_bytes(
                sw as usize,
                sh as usize,
                self.panel_w as usize,
                self.panel_h as usize,
                SHADOW_MARGIN as usize,
                self.scale as f32,
            ),
        )?;
        Ok(SizedGeometry {
            map,
            lights,
            depth_grain,
            mask,
            shadow,
        })
    }

    fn bitmap(&self, w: u32, h: u32, bytes: &[u8]) -> Result<ID2D1Bitmap1, String> {
        unsafe { self.d2d.CreateBitmap(D2D_SIZE_U { width: w, height: h },
            Some(bytes.as_ptr() as *const _), w * 4, &bitmap_props(D2D1_BITMAP_OPTIONS_NONE)) }
            .map_err(|e| format!("surface bitmap: {e:?}"))
    }

    /// Copy a captured desktop crop straight into the GPU source texture.
    /// Nothing touches the CPU: the effect graph samples the result.
    /// A crop clamped by the monitor edge is copied at its true offset,
    /// leaving the off-screen remainder as the cleared pad.
    fn copy_crop(
        &self,
        src: &ID3D11Texture2D,
        bx: u32,
        by: u32,
        bw: u32,
        bh: u32,
        dx: u32,
        dy: u32,
    ) -> Result<(), String> {
        let dst = self.source_tex.as_ref().ok_or("no source texture")?;
        let (ew, eh) = (
            self.panel_w + BLUR_PAD * 2,
            self.panel_h + BLUR_PAD * 2,
        );
        if bw == 0
            || bh == 0
            || dx.checked_add(bw).map(|r| r > ew).unwrap_or(true)
            || dy.checked_add(bh).map(|r| r > eh).unwrap_or(true)
        {
            return Err(format!("crop {bw}x{bh} at {dx},{dy} outside source {ew}x{eh}"));
        }
        let right = bx.checked_add(bw).ok_or("crop x overflow")?;
        let bottom = by.checked_add(bh).ok_or("crop y overflow")?;
        let region = D3D11_BOX {
            left: bx,
            top: by,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        unsafe {
            self.context
                .CopySubresourceRegion(dst, 0, dx, dy, 0, src, 0, Some(&region));
        }
        Ok(())
    }

    /// Test-only CPU upload for the offscreen probe (production frames
    /// arrive by GPU copy in `copy_crop`).
    #[cfg(test)]
    fn upload_source(&self, pixels: &[u8], w: u32, h: u32) -> Result<(), String> {
        let tex = self.source_tex.as_ref().ok_or("no source texture")?;
        if w != self.panel_w + BLUR_PAD * 2 || h != self.panel_h + BLUR_PAD * 2 {
            return Err("probe size drift".to_string());
        }
        if pixels.len() != (w * h * 4) as usize {
            return Err("probe pixel size mismatch".to_string());
        }
        let resource: ID3D11Resource = tex
            .cast()
            .map_err(|e| format!("probe QI resource failed: {e:?}"))?;
        unsafe {
            self.context.UpdateSubresource(
                &resource,
                0,
                None,
                pixels.as_ptr() as *const _,
                w * 4,
                0,
            );
        }
        Ok(())
    }

    /// Box-downsample the source to 16x16 on the GPU and read back 1 KiB.
    /// This replaces the old full-panel Map, which was the single largest
    /// per-frame CPU cost.
    fn sample_backdrop(&self) -> Result<Vec<u8>, String> {
        let source = self.source.as_ref().ok_or("no source bitmap")?;
        let dest = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: SAMPLE_W as f32,
            bottom: SAMPLE_H as f32,
        };
        unsafe {
            // D2D's target must not be the swapchain while stats are
            // taken; render_frame rebinds its own target afterwards.
            self.d2d.SetTarget(&self.sample_target);
            self.d2d.BeginDraw();
            self.d2d
                .DrawBitmap(source, Some(&dest), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
            self.d2d
                .EndDraw(None, None)
                .map_err(|e| format!("sample EndDraw failed: {e:?}"))?;
            self.d2d.SetTarget(None);
            self.context
                .CopyResource(&self.sample_staging, &self.sample_tex);
        }
        let mut mapped: D3D11_MAPPED_SUBRESOURCE = unsafe { std::mem::zeroed() };
        unsafe {
            self.context
                .Map(
                    &self.sample_staging,
                    0,
                    D3D11_MAP_READ,
                    0,
                    Some(&mut mapped as *mut _),
                )
                .map_err(|e| format!("sample Map failed: {e:?}"))?;
        }
        let mut out = vec![0u8; (SAMPLE_W * SAMPLE_H * 4) as usize];
        for row in 0..SAMPLE_H as usize {
            let src = unsafe {
                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize)
            };
            let dst = row * SAMPLE_W as usize * 4;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    src,
                    out.as_mut_ptr().add(dst),
                    SAMPLE_W as usize * 4,
                );
            }
        }
        unsafe { self.context.Unmap(&self.sample_staging, 0) };
        Ok(out)
    }

    /// Present the held source with the current material. Callers update
    /// the source (via `copy_crop`) and its statistics first.
    fn present(&self, material: &Material, bend: f32) -> Result<(), String> {
        self.set_tint(material.wash());
        self.set_bend(bend);
        self.render_frame()
    }

    fn update_local_tint(&self, pixels: &[u8], w: u32, h: u32, material: &Material) -> Result<(), String> {
        let target = backdrop_grid(pixels, w as usize, h as usize, material.wash());
        let mut grid = self.tint_grid.borrow_mut();
        let smoothed = grid.get_or_insert_with(|| target.clone());
        for (cell, next) in smoothed.iter_mut().zip(target) {
            for channel in 0..4 { cell[channel] += (next[channel] - cell[channel]) * LUMA_EMA; }
        }
        let bytes: Vec<u8> = smoothed.iter().flat_map(|c| {
            let a = c[3].clamp(0.0, 1.0);
            [c[2] * a, c[1] * a, c[0] * a, a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        }).collect();
        if let Some(bitmap) = &self.local_tint {
            unsafe { bitmap.CopyFromMemory(None, bytes.as_ptr() as *const _, 16) }
                .map_err(|e| format!("local tint upload: {e:?}"))?;
        }
        Ok(())
    }

    /// Lens strength: 0 is flat, 1 is resting curvature. Small overshoot
    /// survives the clamp. Each RGB branch gets its own dispersion scale.
    fn set_bend(&self, bend: f32) {
        for (lens, scale) in [&self.red_displace, &self.displace, &self.blue_displace]
            .iter().zip(dispersion_scales(bend)) {
            let _ = set_float(lens, D2D1_DISPLACEMENTMAP_PROP_SCALE.0, scale);
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
        let displace = &self.optical_output;
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
            // Composition swapchain carries premultiplied alpha: start from
            // fully transparent so the mask below owns the silhouette.
            self.d2d.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let origin = Vector2 {
                X: SHADOW_MARGIN as f32,
                Y: SHADOW_MARGIN as f32,
            };
            let source_rect = D2D_RECT_F {
                left: BLUR_PAD as f32,
                top: BLUR_PAD as f32,
                right: (BLUR_PAD + self.panel_w) as f32,
                bottom: (BLUR_PAD + self.panel_h) as f32,
            };
            self.d2d.DrawImage(
                &effect_output(displace)?,
                Some(&origin as *const _),
                Some(&source_rect as *const _),
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            let panel = D2D_RECT_F {
                left: origin.X,
                top: origin.Y,
                right: origin.X + self.panel_w as f32,
                bottom: origin.Y + self.panel_h as f32,
            };
            self.d2d
                .FillRectangle(&panel as *const _, &self.tint_brush);
            if let Some(local) = &self.local_tint {
                self.d2d.DrawBitmap(local, Some(&panel), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
            }
            if let Some(depth) = &self.depth_grain {
                self.d2d.DrawBitmap(depth, Some(&panel), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
            }
            let (lx, ly, focused) = self.light_direction.get();
            // Ambient-dominant: every edge keeps a strong base so all four
            // corners match; the pointer adds a mild directional bias.
            let bias = [
                (-lx).clamp(0.0, 1.0),
                lx.clamp(0.0, 1.0),
                (-ly).clamp(0.0, 1.0),
                ly.clamp(0.0, 1.0),
            ];
            for (light, bias) in self.lights.iter().zip(bias) {
                let opacity = (0.75 + 0.45 * bias) * if focused { 1.0 } else { 0.6 };
                self.d2d.DrawBitmap(light, Some(&panel), opacity, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
            }
            if let Some(mask) = &self.mask {
                let image: ID2D1Image = mask
                    .cast()
                    .map_err(|e| format!("mask as image failed: {e:?}"))?;
                self.d2d.DrawImage(
                    &image,
                    Some(&origin as *const _),
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_DESTINATION_IN,
                );
            }
            // Outside the panel only: the bitmap clears the panel interior,
            // and drawing after the mask keeps it off the material.
            if let Some(shadow) = &self.shadow {
                self.d2d.DrawBitmap(
                    shadow,
                    None,
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            }
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

/// Composition swapchain backbuffers carry premultiplied alpha, so the
/// target declares PREMULTIPLIED and our rounded mask can punch the
/// silhouette into the alpha channel. CANNOT_DRAW is required for
/// swapchain-bound targets.
fn target_props() -> D2D1_BITMAP_PROPERTIES1 {
    bitmap_props_with_alpha(
        D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        D2D1_ALPHA_MODE_PREMULTIPLIED,
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

/// Create one BGRA texture for the capture/render path.
fn frame_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    usage: D3D11_USAGE,
    bind: u32,
    cpu: u32,
) -> Result<ID3D11Texture2D, String> {
    unsafe {
        let mut texture: Option<ID3D11Texture2D> = None;
        device
            .CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: usage,
                    BindFlags: bind,
                    CPUAccessFlags: cpu,
                    MiscFlags: 0,
                },
                None,
                Some(&mut texture as *mut _),
            )
            .map_err(|e| format!("texture {width}x{height} failed: {e:?}"))?;
        texture.ok_or_else(|| "texture came back empty".to_string())
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

fn create_effect(d2d: &ID2D1DeviceContext, id: &windows::core::GUID) -> Result<ID2D1Effect, String> {
    unsafe { d2d.CreateEffect(id as *const _) }.map_err(|e| format!("glass effect {id:?}: {e:?}"))
}

fn set_float(effect: &ID2D1Effect, property: i32, value: f32) -> Result<(), String> {
    unsafe { effect.SetValue(property as u32, D2D1_PROPERTY_TYPE_FLOAT, &value.to_ne_bytes()) }
        .map_err(|e| format!("glass property {property}: {e:?}"))
}

fn set_color_matrix(effect: &ID2D1Effect, matrix: &[f32; 20]) -> Result<(), String> {
    let bytes: Vec<u8> = matrix.iter().flat_map(|v| v.to_ne_bytes()).collect();
    unsafe { effect.SetValue(D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32, D2D1_PROPERTY_TYPE_MATRIX_5X4, &bytes) }
        .map_err(|e| format!("glass color matrix: {e:?}"))
}

fn displacement_effect(d2d: &ID2D1DeviceContext) -> Result<ID2D1Effect, String> {
    let effect = create_effect(d2d, &CLSID_D2D1DisplacementMap)?;
    for (property, channel) in [
        (D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT, D2D1_CHANNEL_SELECTOR_R),
        (D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT, D2D1_CHANNEL_SELECTOR_G),
    ] {
        unsafe { effect.SetValue(property.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &channel.0.to_ne_bytes()) }
            .map_err(|e| format!("lens channel: {e:?}"))?;
    }
    Ok(effect)
}

fn saturation_matrix(amount: f32) -> [f32; 20] {
    let mut matrix = [0.0; 20];
    for (input, weight) in [0.2126, 0.7152, 0.0722].iter().enumerate() {
        for output in 0..3 {
            matrix[input * 4 + output] = weight * (1.0 - amount)
                + if input == output { amount } else { 0.0 };
        }
    }
    matrix[15] = 1.0;
    matrix
}

fn dispersion_scales(bend: f32) -> [f32; 3] {
    let scale = BEND_SCALE * bend.clamp(0.0, 1.12);
    [scale * (1.0 + ABERRATION), scale, scale * (1.0 - ABERRATION)]
}

fn scatter_weight(depth: f32) -> f32 {
    let t = (depth / BEZEL_MARGIN).clamp(0.0, 1.0);
    // Modest diffusion even at the center; smooth derivative at both ends.
    0.10 + 0.45 * (1.0 - t * t * (3.0 - 2.0 * t))
}

/// Rounded-rectangle signed-distance field with a circular corner arc.
/// Returns approximate inward distance and outward unit normal. The same
/// circle is sampled for the GDI region and the CSS polygon, so the
/// refracted bevel, native cut and page clip share one silhouette.
fn lens_profile(x: f32, y: f32, w: f32, h: f32, radius: f32) -> (f32, f32, f32) {
    let px = x - w * 0.5;
    let py = y - h * 0.5;
    let radius = radius.min(w * 0.5).min(h * 0.5);
    let qx = px.abs() - (w * 0.5 - radius);
    let qy = py.abs() - (h * 0.5 - radius);
    if qx > 0.0 && qy > 0.0 {
        let length = qx.hypot(qy).max(0.00001);
        (radius - length, qx / length * px.signum(), qy / length * py.signum())
    } else if qx > qy {
        (radius - qx, px.signum(), 0.0)
    } else {
        (radius - qy, 0.0, py.signum())
    }
}

fn surface_layers(w: usize, h: usize, scale: f32) -> Vec<Vec<u8>> {
    let mut layers = vec![Vec::with_capacity(w * h * 4); 5];
    for y in 0..h {
        for x in 0..w {
            let (depth, nx, ny) = lens_profile(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, CORNER_RADIUS * scale);
            let depth = depth / scale;
            let ridge = rim_light(depth);
            // Uniform along each edge: directional variation is applied once
            // per frame from the pointer light, so all four corners read the
            // same brightness instead of two lit diagonals.
            let weights = [(-nx).max(0.0), nx.max(0.0), (-ny).max(0.0), ny.max(0.0)];
            for (layer, facing) in layers[..4].iter_mut().zip(weights) {
                let alpha = (ridge * facing * 255.0).clamp(0.0, 255.0) as u8;
                layer.extend_from_slice(&[alpha, alpha, alpha, alpha]);
            }
            // Fixed zero-mean grain: no shimmer, less than one 8-bit level
            // of average modulation. Inner bottom/right shade adds depth.
            let mut hash = (x as u32).wrapping_mul(0x9e3779b9) ^ (y as u32).wrapping_mul(0x85ebca6b);
            hash ^= hash >> 16;
            hash = hash.wrapping_mul(0x7feb352d);
            hash ^= hash >> 15;
            let white = hash & 1 != 0;
            let shade = if depth < BEZEL_MARGIN {
                ((nx + ny).max(0.0) * (-depth.max(0.0) / 6.0).exp() * 0.09 * 255.0) as u8
            } else { 0 };
            let alpha = shade.saturating_add(1);
            let color = if white { 1 } else { 0 };
            layers[4].extend_from_slice(&[color, color, color, alpha]);
        }
    }
    layers
}

fn rim_light(depth: f32) -> f32 {
    // Peak at the boundary, never an offset ridge/double outline. A
    // narrow specular hairline sits on a wider glow; the glow is what
    // makes the pane read as thick glass under a moving light.
    let depth = depth.max(0.0);
    if depth >= BEZEL_MARGIN { return 0.0; }
    0.34 * (-(depth / 0.9).powi(2)).exp() + 0.055 * (-depth / 14.0).exp()
}

fn backdrop_grid(pixels: &[u8], w: usize, h: usize, wash: (f32, f32, f32, f32)) -> Vec<[f32; 4]> {
    let mut grid = vec![[0.0; 4]; 16];
    if w == 0 || h == 0 || pixels.len() != w * h * 4 { return grid; }
    for gy in 0..4 {
        for gx in 0..4 {
            let mut rgb = [0.0; 3];
            let mut count = 0.0;
            // Statistics arrive as a 16x16 GPU downsample, so one cell is
            // 4x4 pixels: average all of them. (The old step of 8 sampled
            // a single corner of each cell.)
            for y in gy * h / 4..(gy + 1) * h / 4 {
                for x in gx * w / 4..(gx + 1) * w / 4 {
                    let i = (y * w + x) * 4;
                    for c in 0..3 { rgb[c] += pixels[i + 2 - c] as f32 / 255.0; }
                    count += 1.0;
                }
            }
            if count == 0.0 { continue; }
            for c in &mut rgb { *c /= count; }
            let luma = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
            // Follow the continuous wash, not the discrete page palette.
            let base = [wash.0, wash.1, wash.2];
            let polarity = (base[0] + base[1] + base[2]) / 3.0;
            let conflict = luma * (1.0 - polarity) + (1.0 - luma) * polarity;
            grid[gy * 4 + gx] = [
                (base[0] + (rgb[0] - luma) * 0.18).clamp(0.0, 1.0),
                (base[1] + (rgb[1] - luma) * 0.18).clamp(0.0, 1.0),
                (base[2] + (rgb[2] - luma) * 0.18).clamp(0.0, 1.0),
                0.008 + conflict * 0.035,
            ];
        }
    }
    grid
}

/// Rounded convex-rim displacement map: neutral gray center, bevel
/// channels bend samples inward (Source[p + Amount·(channel − 0.5)]).
#[cfg(test)]
fn bezel_bytes(w: usize, h: usize) -> Vec<u8> {
    bezel_bytes_scaled(w, h, 1.0)
}

fn bezel_bytes_scaled(w: usize, h: usize, scale: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let (width, height) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            // Rounded-rectangle signed-distance field from the lab:
            // the center remains undistorted while the curved rim eases
            // the captured image toward the pane's center.
            let (depth, nx, ny) = lens_profile(x as f32 + 0.5, y as f32 + 0.5, width, height, CORNER_RADIUS * scale);
            let depth = depth / scale;
            let t = (depth / BEZEL_MARGIN).clamp(0.0, 1.0);
            // Rim bevel plus a slowly decaying thickness tail. The tail
            // is continuous across the 32 px boundary (the rim term is
            // already zero there), so the pane reads as one slab rather
            // than a bevelled frame around a window. Its direction is
            // radial from the pane center, not the nearest-edge normal:
            // the latter flips across the medial axis and would leave a
            // one-pixel seam through the middle of the glass.
            let rim = if t >= 1.0 { 0.0 } else { (1.0 - t) * (1.0 - t) };
            let px = x as f32 + 0.5 - width * 0.5;
            let py = y as f32 + 0.5 - height * 0.5;
            // Optical centre stays neutral; the radial bend grows toward
            // the edges, like a slab whose thickness is felt at the rim.
            let span = (width.min(height) * 0.5).max(1.0);
            let reach = (px.hypot(py) / span).clamp(0.0, 1.0);
            let slab = SLAB_BEND * reach * (-depth.max(0.0) / SLAB_DECAY).exp();
            let radial = 1.0 / px.hypot(py).max(1.0);
            let (dx, dy) = (
                -nx * rim - px * radial * slab,
                -ny * rim - py * radial * slab,
            );
            let r = (128.0 + dx * 127.0).round().clamp(0.0, 255.0) as u8;
            let g = (128.0 + dy * 127.0).round().clamp(0.0, 255.0) as u8;
            let scatter = (scatter_weight(depth) * 255.0).round() as u8;
            out.extend_from_slice(&[scatter, g, r, 255]);
        }
    }
    out
}

/// Same sampled continuous corner feeds GDI regions and the CSS polygon.
/// Offsets are logical pixels from each corner; no independent CSS radius.
/// Circular arc: a quarter circle of radius R removes 0.293R at the
/// diagonal. The earlier sqrt-cos profile removed only 0.159R there,
/// which read as a nearly square corner despite the radius.
fn corner_offsets() -> Vec<(f32, f32)> {
    const SAMPLES: u32 = 24;
    (0..=SAMPLES).map(|i| {
        let angle = i as f32 / SAMPLES as f32 * std::f32::consts::FRAC_PI_2;
        let (sin, cos) = angle.sin_cos();
        (CORNER_RADIUS * (1.0 - cos), CORNER_RADIUS * (1.0 - sin))
    }).collect()
}

pub(crate) fn lens_clip_css() -> String {
    let offsets = corner_offsets();
    let mut points = Vec::new();
    for corner in 0..4 {
        for &(x, y) in &offsets {
            points.push(match corner {
                0 => format!("{x:.3}px {y:.3}px"),
                1 => format!("calc(100% - {y:.3}px) {x:.3}px"),
                2 => format!("calc(100% - {x:.3}px) calc(100% - {y:.3}px)"),
                _ => format!("{y:.3}px calc(100% - {x:.3}px)"),
            });
        }
    }
    format!("polygon({})", points.join(","))
}

/// Panel-sized premultiplied alpha mask for the rounded silhouette.
/// Analytic signed distance to a rounded rectangle, 1 px coverage ramp,
/// so the compositor blends a smooth arc instead of a binary GDI region.
fn rounded_mask(w: u32, h: u32, scale: f32, radius_logical: f32) -> Vec<u8> {
    let rw = w as f32;
    let rh = h as f32;
    let half_w = rw * 0.5;
    let half_h = rh * 0.5;
    let r = (radius_logical * scale).min(half_w).min(half_h).max(0.0);
    let inset_x = (half_w - r).max(0.0);
    let inset_y = (half_h - r).max(0.0);
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        let qy = (y as f32 + 0.5 - half_h).abs() - inset_y;
        for x in 0..w {
            let qx = (x as f32 + 0.5 - half_w).abs() - inset_x;
            let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
            let coverage = (0.5 - outside).clamp(0.0, 1.0);
            let a = (coverage * 255.0).round() as u8;
            let i = (y as usize * w as usize + x as usize) * 4;
            out[i..i + 4].copy_from_slice(&[a, a, a, a]);
        }
    }
    out
}

/// Contact shadow: premultiplied black, analytic rounded-rect falloff,
/// panel interior cleared so the material is never darkened. `w`/`h` cover
/// the panel plus `margin` on every side, the panel box starts at
/// `(margin, margin)`, and the falloff is centred on it — no offset, so the
/// pane reads as sitting on the desktop rather than lit from one side.
fn shadow_bytes(
    w: usize,
    h: usize,
    panel_w: usize,
    panel_h: usize,
    margin: usize,
    scale: f32,
) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    if panel_w == 0 || panel_h == 0 || margin == 0 {
        return out;
    }
    let radius = (CORNER_RADIUS * scale)
        .min(panel_w as f32 * 0.5)
        .min(panel_h as f32 * 0.5)
        .max(0.0);
    let blur = SHADOW_BLUR.max(1.0);
    let cx = margin as f32 + panel_w as f32 * 0.5;
    let cy = margin as f32 + panel_h as f32 * 0.5;
    let half_w = panel_w as f32 * 0.5;
    let half_h = panel_h as f32 * 0.5;
    // Signed distance to the rounded panel rect.
    let sdf = |x: f32, y: f32| {
        let qx = (x - cx).abs() - (half_w - radius);
        let qy = (y - cy).abs() - (half_h - radius);
        if qx > 0.0 && qy > 0.0 {
            qx.hypot(qy) - radius
        } else {
            qx.max(qy) - radius
        }
    };
    for y in 0..h {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            // The material covers the panel: leave it untouched.
            let d = sdf(px, py);
            if d <= 0.0 {
                continue;
            }
            let t = (d / blur).min(1.0);
            let falloff = 1.0 - t * t * (3.0 - 2.0 * t);
            let alpha = (SHADOW_STRENGTH * falloff * 255.0).round().clamp(0.0, 255.0) as u8;
            if alpha > 0 {
                out[(y * w + x) * 4 + 3] = alpha;
            }
        }
    }
    out
}

/// Polygon points for the lens silhouette in device pixels.
/// `grow_px` expands the circular corner radially from its centre, so the
/// cut stays a true circle. Growing x/y independently distorted the arc
/// (1.41 px at the diagonal, 1 px at the tangents); points may sit just
/// outside the window rect, where Windows clips them.
fn lens_region_points(w: u32, h: u32, scale: f64, grow_px: f64) -> Vec<(f64, f64)> {
    let radius = (CORNER_RADIUS as f64 * scale).max(1.0);
    let factor = (radius + grow_px) / radius;
    let mut points = Vec::new();
    for corner in 0..4 {
        for (x, y) in corner_offsets() {
            let ox = x as f64 * scale;
            let oy = y as f64 * scale;
            let x = radius + (ox - radius) * factor;
            let y = radius + (oy - radius) * factor;
            points.push(match corner {
                0 => (x, y),
                1 => (w as f64 - y, x),
                2 => (w as f64 - x, h as f64 - y),
                _ => (y, h as f64 - x),
            });
        }
    }
    points
}

/// Offset of the client area's origin inside the window's own coordinates.
///
/// `SetWindowRgn` works in window coordinates, and a borderless tao window
/// still carries an invisible resize frame, so the client does not start at
/// (0, 0). Handing the region a client-sized polygon anchored at the window
/// origin shaves the frame width (~8 px) off the page's right and bottom.
fn client_origin_in_window(hwnd: isize) -> (i32, i32) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    let mut window = RECT::default();
    let mut origin = POINT { x: 0, y: 0 };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd as _, &mut window);
        windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd as _, &mut origin);
    }
    (origin.x - window.left, origin.y - window.top)
}

fn apply_lens_region(hwnd: isize, w: u32, h: u32, scale: f64, grow_px: f64) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{CreatePolygonRgn, SetWindowRgn, DeleteObject, WINDING};
    let (ox, oy) = client_origin_in_window(hwnd);
    let points: Vec<POINT> = lens_region_points(w, h, scale, grow_px)
        .into_iter()
        .map(|(x, y)| POINT {
            x: ox + x.round() as i32,
            y: oy + y.round() as i32,
        })
        .collect();
    unsafe {
        let region = CreatePolygonRgn(points.as_ptr(), points.len() as i32, WINDING);
        if region.is_null() { return false; }
        if SetWindowRgn(hwnd as _, region, 1) == 0 { DeleteObject(region); false } else { true }
    }
}

/// Worker-owned capture: duplication session + full-frame GPU mirror.
/// The mirror is what makes exact re-crops free after a panel resize:
/// the desktop image never has to come back to the CPU.
struct Capture {
    duplication: IDXGIOutputDuplication,
    mirror: ID3D11Texture2D,
    frame_w: u32,
    frame_h: u32,
    origin_x: i32,
    origin_y: i32,
    monitor: HMONITOR,
    protected_logged: bool,
}

/// Damped show response with a small overshoot and a finite deadline.
fn swell_bend(elapsed_ms: f32) -> f32 {
    if elapsed_ms >= SWELL_MS { return 1.0; }
    let mut spring = Spring { value: SWELL_START, velocity: 0.0 };
    spring.step(1.0, elapsed_ms.max(0.0) / 1000.0);
    spring.value
}

/// Exact solution of an underdamped spring. Retarget without throwing away
/// velocity; elapsed wall time, not frame count, determines motion.
#[derive(Clone, Copy)]
struct Spring { value: f32, velocity: f32 }

impl Spring {
    fn step(&mut self, target: f32, seconds: f32) {
        let seconds = seconds.clamp(0.0, 1.0);
        let decay = 22.0_f32;
        let frequency = 22.0_f32;
        let offset = self.value - target;
        let sine_weight = (self.velocity + decay * offset) / frequency;
        let (sin, cos) = (frequency * seconds).sin_cos();
        let envelope = (-decay * seconds).exp();
        let wave = offset * cos + sine_weight * sin;
        self.value = target + envelope * wave;
        self.velocity = envelope * (-decay * wave + frequency * (-offset * sin + sine_weight * cos));
        if (self.value - target).abs() < 0.0001 && self.velocity.abs() < 0.002 {
            self.value = target;
            self.velocity = 0.0;
        }
    }
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
    /// without a clock. Clears only at the deadline, not the first spring
    /// crossing of 1 (which would cut off the rebound).
    fn bend_at(&mut self, elapsed_ms: f32) -> (f32, bool) {
        let value = swell_bend(elapsed_ms);
        if elapsed_ms >= SWELL_MS {
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

/// Pull the desktop's newest frame into the GPU mirror, if the desktop
/// has produced one since the last acquire. Returns whether the mirror
/// moved. The copy stays on the GPU so a large desktop frame never crosses
/// the bus just to be cropped.
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
fn refresh_mirror(cap: &mut Capture, context: &ID3D11DeviceContext) -> bool {
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
        unsafe { context.CopySubresourceRegion(&cap.mirror, 0, 0, 0, 0, &texture, 0, None) };
        Ok(())
    })()
    .is_ok();
    let _ = unsafe { cap.duplication.ReleaseFrame() };
    copied
}

/// Count a failed engine build/rebuild. Three straight is a poisoned graph
/// rather than a transient: tell the host to go opaque, once. The worker
/// still retries, so a lucky recovery wins the pixels back without a restart
/// (the host only needs the flag for the page surface).
fn note_engine_failure(n: &mut u32, proxy: &EventLoopProxy<UiCommand>) {
    *n += 1;
    if *n >= 3 && !glass_failed() {
        mark_glass_failed();
        crate::logging::warn(
            "[nex][refract] engine failed repeatedly — requesting opaque fallback",
        );
        let _ = proxy.send_event(UiCommand::GlassFailed);
    }
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
    let mut scale = 1.0;
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
    // Straight failures in a row: a poisoned device/capture never heals
    // by retrying the same calls, so rebuild everything after a few.
    let mut n_fail = 0u32;
    // Engine build/rebuild failures in a row. Kept separate from capture
    // failures: one is a poisoned GPU graph, the other a stale session.
    let mut n_engine_fail = 0u32;
    let mut motion = crate::overlay::platform::animations_enabled();
    let mut interaction = Spring { value: 1.0, velocity: 0.0 };
    let mut interaction_at = Instant::now();
    let mut interaction_dirty = false;
    let mut previous_panel = (0, 0);

    loop {
        while let Ok(cfg) = rx.try_recv() {
            if let Some(v) = cfg.visible {
                if v && !visible {
                    motion = crate::overlay::platform::animations_enabled();
                    interaction = Spring { value: 1.0, velocity: 0.0 };
                    interaction_at = Instant::now();
                    // Fresh show: re-announce the material, the page may
                    // have re-seeded its palette to the system theme.
                    palette_sent = None;
                    // And let the first frame decide outright, so the
                    // page palette lands with the glass instead of
                    // waiting for the desktop to change eight times.
                    material.fresh = true;
                    material.dwell = 0;
                    if let Some(e) = &engine { *e.tint_grid.borrow_mut() = None; }
                    // Present the held mirror before the reveal and swell
                    // the lens in: a pane of glass settling, not a surface
                    // snapping on.
                    cycle.arm();
                    if !motion { cycle.swell = None; }
                }
                if !v {
                    cycle.swell = None;
                    cycle.refresh = false;
                    interaction_dirty = false;
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
            if let Some(s) = cfg.scale { scale = s; }
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
        let now = Instant::now();
        let elapsed = now.duration_since(interaction_at).as_secs_f32();
        interaction_at = now;
        let mut pointer = POINT { x: 0, y: 0 };
        let inside = visible && unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pointer) }.is_ok()
            && pointer.x >= mx && pointer.x < mx + pw as i32
            && pointer.y >= my && pointer.y < my + ph as i32;
        let pressed = inside && unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(1)
        } < 0;
        let target = if pressed { 0.88 } else { 1.0 };
        let foreground = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        let focused = foreground.0 as isize == main_sys;
        // Resting light is neutral: no edge wins until the pointer moves
        // over the panel, so all four sides stay equally lit at rest.
        let light = if inside && motion {
            (((pointer.x - mx) as f32 / pw.max(1) as f32 - 0.5) * 1.4,
             ((pointer.y - my) as f32 / ph.max(1) as f32 - 0.5) * 1.4, focused)
        } else { (0.0, 0.0, focused) };
        if let Some(e) = &engine {
            if e.light_direction.get() != light {
                e.light_direction.set(light);
                interaction_dirty |= visible;
            }
        }
        if motion && visible {
            if previous_panel != (0, 0) && previous_panel != (pw, ph) {
                interaction.velocity += if ph > previous_panel.1 { -1.5 } else { 1.5 };
            }
            let before = interaction.value;
            interaction.step(target, elapsed);
            interaction_dirty |= (before - interaction.value).abs() > 0.00001;
        } else {
            interaction = Spring { value: 1.0, velocity: 0.0 };
        }
        previous_panel = (pw, ph);
        let bend = cycle.bend().0 * interaction.value;
        // Backbuffer covers the max panel plus the shadow margin on every side.
        let (buf_w, buf_h) = (
            max_wh.0 + SHADOW_MARGIN * 2,
            max_wh.1 + SHADOW_MARGIN * 2,
        );
        if engine.is_none() {
            match Engine::build(hwnd, buf_w, buf_h, pw, ph) {
                Ok(mut e) => {
                    if e.scale != scale {
                        e.scale = scale;
                        if let Err(error) = e.rebuild_content() {
                            crate::logging::warn(&format!("[nex][refract] scale rebuild failed: {error}"));
                            note_engine_failure(&mut n_engine_fail, &proxy);
                            continue;
                        }
                    }
                    e.light_direction.set(light);
                    engine = Some(e);
                    n_engine_fail = 0;
                },
                Err(error) => {
                    crate::logging::warn(&format!("[nex][refract] engine build failed: {error}"));
                    note_engine_failure(&mut n_engine_fail, &proxy);
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
            if buf_w > e.width || buf_h > e.height {
                // Rare (DPI/scale change): grow buffers (rebuilds inside).
                e.panel_w = pw;
                e.panel_h = ph;
                e.scale = scale;
                if let Err(error) = e.resize_buffers(buf_w, buf_h) {
                    crate::logging::warn(&format!("[nex][refract] engine resize failed: {error}"));
                    note_engine_failure(&mut n_engine_fail, &proxy);
                    engine = None;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                rebuilt = true;
            } else if e.panel_w != pw || e.panel_h != ph || e.scale != scale {
                e.panel_w = pw;
                e.panel_h = ph;
                e.scale = scale;
                if let Err(error) = e.rebuild_content() {
                    crate::logging::warn(&format!("[nex][refract] content rebuild failed: {error}"));
                    note_engine_failure(&mut n_engine_fail, &proxy);
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
            let swell_due = (cycle.swelling() || interaction_dirty)
                && swell_pass_due(last_present.elapsed().as_millis() as u64);
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
                    .is_some_and(|cap| refresh_mirror(cap, &e.context));
            if live {
                // A frame this fresh counts as the desktop moving: it *is*
                // the desktop, taken seconds later than any pass could.
                last_desktop_frame = Some(Instant::now());
            }
            if !warm && (rebuilt || refresh || swelling) {
                // Re-present the held desktop image. The crop runs on the
                // GPU, so a resize never needs the pixels on the CPU: the
                // mirror still holds the full desktop, and the new panel
                // box is cut out of it exactly.
                if staged {
                    if let Some(cap) = capture.as_ref() {
                        let pad = BLUR_PAD as i32;
                        let want_x = mx - cap.origin_x - pad;
                        let want_y = my - cap.origin_y - pad;
                        let (bx, by, bw, bh) = clamp_panel_box(
                            want_x,
                            want_y,
                            pw + BLUR_PAD * 2,
                            ph + BLUR_PAD * 2,
                            0,
                            0,
                            cap.frame_w,
                            cap.frame_h,
                        );
                        let dx = (bx as i32 - want_x.max(0)).max(0) as u32;
                        let dy = (by as i32 - want_y.max(0)).max(0) as u32;
                        let cropped = e
                            .copy_crop(&cap.mirror, bx, by, bw, bh, dx, dy)
                            .inspect_err(|error| {
                                crate::logging::warn(&format!(
                                    "[nex][refract] re-crop failed: {error}"
                                ));
                            })
                            .is_ok();
                        if cropped {
                            match e.sample_backdrop() {
                                Ok(sample) => {
                                    if material.fresh {
                                        material.update(&sample);
                                        palette_sent = Some(material.light);
                                        let _ = proxy
                                            .send_event(UiCommand::GlassPalette(material.light));
                                    }
                                    if let Err(error) = e.update_local_tint(
                                        &sample,
                                        SAMPLE_W,
                                        SAMPLE_H,
                                        &material,
                                    ) {
                                        crate::logging::warn(&format!(
                                            "[nex][refract] tint grid failed: {error}"
                                        ));
                                    }
                                }
                                Err(error) => crate::logging::warn(&format!(
                                    "[nex][refract] stats sample failed: {error}"
                                )),
                            }
                            match e.present(&material, bend) {
                                Ok(()) => {
                                    n_pre += 1;
                                    interaction_dirty = false;
                                    last_present = Instant::now();
                                    if announce {
                                        // live: the desktop had a newer frame
                                        // than the hidden poll's. mirror: it
                                        // had none, so the mirror already was
                                        // the current desktop.
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
        let want_x = mx - cap.origin_x - pad;
        let want_y = my - cap.origin_y - pad;
        let (bx, by, bw, bh) = clamp_panel_box(
            want_x,
            want_y,
            pw + BLUR_PAD * 2,
            ph + BLUR_PAD * 2,
            0,
            0,
            cap.frame_w,
            cap.frame_h,
        );
        if bw == 0 || bh == 0 {
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            continue;
        }
        let dx = (bx as i32 - want_x.max(0)).max(0) as u32;
        let dy = (by as i32 - want_y.max(0)).max(0) as u32;
        let frame_ok = (|| -> Result<(u64, u64), String> {
            let tex: ID3D11Texture2D = resource
                .as_ref()
                .ok_or("empty frame resource")?
                .cast()
                .map_err(|e| format!("frame QI texture failed: {e:?}"))?;
            // Keep the full desktop on the GPU: a later resize cuts a new
            // panel box out of it for free, even if the desktop never
            // changes again. Only the panel crop is fed to the effects;
            // the mirror itself is never read on the CPU.
            unsafe {
                eng.context.CopySubresourceRegion(
                    &cap.mirror, 0, 0, 0, 0, &tex, 0, None,
                );
            }
            let t0 = Instant::now();
            eng.copy_crop(&tex, bx, by, bw, bh, dx, dy)?;
            let us = t0.elapsed().as_micros() as u64;
            let render_t = Instant::now();
            // Material and tint statistics need pixels, but only a 16x16
            // downsample of them. This keeps the per-frame CPU work at
            // 1 KiB instead of a full-panel Map + memcpy + upload.
            let sample = eng.sample_backdrop()?;
            let flipped = material.update(&sample);
            // A hidden poll decides the material only: no draw.
            if !warm {
                eng.update_local_tint(&sample, SAMPLE_W, SAMPLE_H, &material)?;
                eng.present(&material, bend)?;
                // Reveal handshake: the host lifts the glass cloak only on a
                // frame that matches the panel size it is about to show, so
                // the first thing seen is this backdrop rather than the last
                // session's held buffer. Cheap to repeat; the host ignores
                // every ack once it is revealed.
                let _ = proxy.send_event(UiCommand::GlassPresented { w: pw, h: ph });
            }
            if flipped || palette_sent != Some(material.light) {
                palette_sent = Some(material.light);
                let _ = proxy.send_event(UiCommand::GlassPalette(material.light));
            }
            let render_us = render_t.elapsed().as_micros() as u64;
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            Ok((us, render_us))
        })();
        match frame_ok {
            Ok((us, frame_render_us)) => {
                copy_us += us;
                render_us += frame_render_us;
                n_fail = 0;
                staged = true;
                if !warm {
                    n_pre += 1;
                    interaction_dirty = false;
                    last_present = Instant::now();
                }
                // Whether the panel is revealed at all is the host's call;
                // this only marks a frame as having been presented ever.
                ready.store(true, Ordering::Release);
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
        let mirror = frame_texture(
            device,
            fw,
            fh,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
            0,
        )
        .map_err(|e| format!("mirror texture failed: {e}"))?;
        Ok(Capture {
            duplication,
            mirror,
            frame_w: fw,
            frame_h: fh,
            origin_x: mi.rcMonitor.left,
            origin_y: mi.rcMonitor.top,
            monitor,
            protected_logged: false,
        })
    }
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
    // Full-panel frames are sampled sparsely; the 16x16 statistics
    // surface is small enough to average completely (a 64-byte stride
    // there would sample one column only).
    let step = if pixels.len() <= (SAMPLE_W * SAMPLE_H * 4) as usize {
        4
    } else {
        64
    };
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
        i += step;
    }
    if n == 0.0 {
        // No pixels (shouldn't happen): mid gray keeps the tint neutral.
        0.5
    } else {
        (sum / n / 255.0).clamp(0.0, 1.0)
    }
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
    let (w, h) = (w + SHADOW_MARGIN * 2, h + SHADOW_MARGIN * 2);
    let main = HWND(main_sys as *mut std::ffi::c_void);
    if let Err(error) = unsafe {
        SetWindowPos(
            glass,
            Some(main),
            x - SHADOW_MARGIN as i32,
            y - SHADOW_MARGIN as i32,
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
        clamp_panel_box, desktop_recent, frame_luma, pass_plan, swell_bend,
        swell_pass_due, swell_wants_mirror, GeometryCache, Material, Pass, ShowCycle,
        DESKTOP_RECENT_MS, GEOMETRY_CACHE_DEPTH,
        HIDDEN_POLL_MS, MATERIAL_HYSTERESIS, MATERIAL_SWITCH, MIN_FRAME_MS,
        SWELL_MS, SWELL_START, TINT_MAX_ALPHA, TINT_MIN_ALPHA,
    };

    fn bgra(b: u8, g: u8, r: u8, pixels: usize) -> Vec<u8> {
        (0..pixels).flat_map(|_| [b, g, r, 255]).collect()
    }

    fn gray(level: u8, pixels: usize) -> Vec<u8> {
        bgra(level, level, level, pixels)
    }

    #[test]
    fn optics_keep_center_neutral_and_dispersion_at_the_rim() {
        let (w, h) = (160, 96);
        let map = super::bezel_bytes(w, h);
        let pixel = |x: usize, y: usize| &map[(y * w + x) * 4..(y * w + x) * 4 + 4];
        // The thickness tail shifts the center by a fraction of a pixel,
        // but must stay neutral (no RGB split) and within one level.
        let centre = pixel(w / 2, h / 2);
        assert!((centre[1] as i32 - 128).abs() <= 1, "center green: {centre:?}");
        assert_eq!(centre[1], centre[2], "center must not split channels");
        assert_eq!(centre[3], 255);
        assert!(pixel(1, h / 2)[2] > 240, "left edge samples inward");
        assert!(pixel(w - 2, h / 2)[2] < 16, "right edge samples inward");
        assert!(pixel(1, h / 2)[0] > pixel(w / 2, h / 2)[0]);
        let scales = super::dispersion_scales(1.0);
        assert!(scales[0] > scales[1] && scales[1] > scales[2]);
        // Dispersion belongs to the lens, so it scales with the bend. 7%
        // each side of a 24 px rim sample: under 1.7 px at the outermost
        // pixel, under 1 px one bevel in — a chromatic edge, not a painted
        // fringe.
        let spread = (scales[0] - scales[2]) * 0.5;
        assert!(spread > 0.5 && spread < 2.0, "rim dispersion {spread}px");
        assert_eq!(super::dispersion_scales(0.0), [0.0; 3]);
        let matrix = super::saturation_matrix(super::SATURATION);
        for output in 0..3 {
            let neutral: f32 = (0..3).map(|input| matrix[input * 4 + output] * 0.5).sum();
            assert!((neutral - 0.5).abs() < 0.0001, "vibrancy must preserve gray");
        }
        for depth in 0..32 {
            assert!(super::scatter_weight(depth as f32) >= super::scatter_weight(depth as f32 + 1.0));
        }
    }

    /// Real effect-graph readback. Explicit opt-in: needs a Windows GPU and
    /// creates a cloaked test HWND, never captures or reads the desktop.
    #[test]
    #[ignore = "requires Windows GPU; run explicitly with --ignored"]
    fn optical_graph_gpu_readback() {
        use super::*;
        use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_MAP_OPTIONS_READ};
        let window = GlassWindow::create(0, 160, 96).expect("cloaked probe window");
        let result = (|| -> Result<(), String> {
            let mut engine = Engine::build(
                window.hwnd(),
                160 + SHADOW_MARGIN * 2,
                96 + SHADOW_MARGIN * 2,
                160,
                96,
            )?;
            let (w, h) = (160 + BLUR_PAD * 2, 96 + BLUR_PAD * 2);
            // GPU frame path: copy a known desktop crop into the source
            // texture, then verify the statistics readback sees it. This
            // is the production route (no CPU upload between capture and
            // effects), not just the offscreen optical draw.
            {
                let mut desktop_px = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    for x in 0..w {
                        let x_hi = x >= w / 2;
                        let y_hi = y >= h / 2;
                        let px = match (x_hi, y_hi) {
                            (false, false) => [0u8, 0, 255, 255],
                            (true, false) => [0, 255, 0, 255],
                            (false, true) => [255, 0, 0, 255],
                            (true, true) => [255, 255, 255, 255],
                        };
                        desktop_px.extend_from_slice(&px);
                    }
                }
                let desktop = frame_texture(
                    &engine.device,
                    w,
                    h,
                    D3D11_USAGE_DEFAULT,
                    D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    0,
                )?;
                let resource: ID3D11Resource = desktop
                    .cast()
                    .map_err(|e| format!("probe desktop QI failed: {e:?}"))?;
                unsafe {
                    engine.context.UpdateSubresource(
                        &resource,
                        0,
                        None,
                        desktop_px.as_ptr() as *const _,
                        w * 4,
                        0,
                    );
                }
                engine.copy_crop(&desktop, 0, 0, w, h, 0, 0)?;
                let sample = engine.sample_backdrop()?;
                let at = |x: u32, y: u32| {
                    let i = ((y * SAMPLE_W + x) * 4) as usize;
                    [sample[i], sample[i + 1], sample[i + 2], sample[i + 3]]
                };
                // Orientation matters: the statistics grid must line up
                // with the page. TL red, TR green, BL blue, BR white.
                let tl = at(1, 1);
                let tr = at(SAMPLE_W - 2, 1);
                let bl = at(1, SAMPLE_H - 2);
                let br = at(SAMPLE_W - 2, SAMPLE_H - 2);
                assert!(tl[2] > 200 && tl[0] < 40, "TL must be red: {tl:?}");
                assert!(tr[1] > 200, "TR must be green: {tr:?}");
                assert!(bl[0] > 200, "BL must be blue: {bl:?}");
                assert!(
                    br.iter().take(3).all(|c| *c > 200),
                    "BR must be white: {br:?}"
                );
            }
            let target = unsafe { engine.d2d.CreateBitmap(
                D2D_SIZE_U { width: 160, height: 96 }, None, 0,
                &bitmap_props(D2D1_BITMAP_OPTIONS_TARGET),
            ) }.map_err(|e| format!("probe target: {e:?}"))?;
            let readback = unsafe { engine.d2d.CreateBitmap(
                D2D_SIZE_U { width: 160, height: 96 }, None, 0,
                &bitmap_props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
            ) }.map_err(|e| format!("probe readback: {e:?}"))?;
            let draw = |pixels: &[u8], bend: f32| -> Result<Vec<u8>, String> {
                engine.upload_source(pixels, w, h)?;
                engine.set_bend(bend);
                unsafe {
                    engine.d2d.SetTarget(&target);
                    engine.d2d.BeginDraw();
                    engine.d2d.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
                    let rect = D2D_RECT_F {
                        left: BLUR_PAD as f32, top: BLUR_PAD as f32,
                        right: (BLUR_PAD + 160) as f32, bottom: (BLUR_PAD + 96) as f32,
                    };
                    engine.d2d.DrawImage(&effect_output(&engine.optical_output)?, None, Some(&rect),
                        D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                    engine.d2d.EndDraw(None, None).map_err(|e| format!("probe draw: {e:?}"))?;
                    engine.d2d.SetTarget(None);
                    readback.CopyFromBitmap(None, &target, None).map_err(|e| format!("probe copy: {e:?}"))?;
                    let mapped = readback.Map(D2D1_MAP_OPTIONS_READ).map_err(|e| format!("probe map: {e:?}"))?;
                    let mut out = Vec::with_capacity(160 * 96 * 4);
                    for y in 0..96 {
                        out.extend_from_slice(std::slice::from_raw_parts(mapped.bits.add(y * mapped.pitch as usize), 160 * 4));
                    }
                    readback.Unmap().map_err(|e| format!("probe unmap: {e:?}"))?;
                    Ok(out)
                }
            };
            let flat = draw(&gray(128, (w * h) as usize), 1.0)?;
            for point in [(80, 48), (3, 48), (156, 48)] {
                let i = (point.1 * 160 + point.0) * 4;
                assert!(flat[i..i + 3].iter().all(|c| (126..=130).contains(c)), "neutral graph: {:?}", &flat[i..i + 4]);
                assert_eq!(flat[i + 3], 255, "RGB composition must stay opaque");
            }
            let mut ramp = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..h {
                for x in 0..w {
                    let level = ((x as f32 - BLUR_PAD as f32) * 4.0 + 48.0).clamp(0.0, 255.0).round() as u8;
                    ramp.extend_from_slice(&[level, level, level, 255]);
                }
            }
            let bent = draw(&ramp, 1.0)?;
            let flat = draw(&ramp, 0.0)?;
            let edge = (48 * 160 + 3) * 4;
            let center = (48 * 160 + 80) * 4;
            assert!(bent[edge + 2] > flat[edge + 2], "left edge must sample inward");
            assert!(bent[edge + 2] > bent[edge], "red bends further than blue: {:?}", &bent[edge..edge + 4]);
            assert!(bent[center + 2].abs_diff(bent[center]) <= 1, "center must not split");
            let mut stripes = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..h { for x in 0..w {
                let level = if x % 16 < 8 { 32 } else { 224 };
                stripes.extend_from_slice(&[level, level, level, 255]);
            } }
            let blurred = draw(&stripes, 0.0)?;
            let contrast = |y: usize| {
                let row: Vec<i32> = (48..112).map(|x| blurred[(y * 160 + x) * 4] as i32).collect();
                row.iter().max().unwrap() - row.iter().min().unwrap()
            };
            assert!(contrast(48) > contrast(2) + 20, "center must retain more detail than rim");
            let start = Instant::now();
            for _ in 0..30 { draw(&stripes, 1.0)?; }
            eprintln!("optical graph probe 160x96: {:.2}ms/frame including GPU readback", start.elapsed().as_secs_f64() * 1000.0 / 30.0);
            // Panel-size content rebuild: the show path pays this on the pass
            // that resizes the panel (60 -> settled height), and the reveal
            // waits for the frame it produces.
            let start = Instant::now();
            engine.panel_w = 700;
            engine.panel_h = 60;
            engine.rebuild_content()?;
            let small = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            engine.panel_h = 441;
            engine.rebuild_content()?;
            eprintln!(
                "content rebuild: 700x60 {small:.2}ms, 700x441 {:.2}ms",
                start.elapsed().as_secs_f64() * 1000.0
            );
            // The show cycle re-lays both boxes, and the reveal waits for the
            // frame that comes out of the second one. With the geometry cache
            // a repeat box is the capture source only.
            let start = Instant::now();
            engine.panel_h = 60;
            engine.rebuild_content()?;
            let warm_bar = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            engine.panel_h = 441;
            engine.rebuild_content()?;
            eprintln!(
                "cached content rebuild: 700x60 {warm_bar:.2}ms, 700x441 {:.2}ms",
                start.elapsed().as_secs_f64() * 1000.0
            );
            let start = Instant::now();
            engine.panel_w = 700;
            engine.panel_h = 441;
            engine.resize_buffers(700 + SHADOW_MARGIN * 2, 441 + SHADOW_MARGIN * 2)?;
            eprintln!("full-panel content rebuild+buffer growth: {:.2}ms", start.elapsed().as_secs_f64() * 1000.0);
            // Production per-frame path: frames already live on the GPU
            // (copy_crop); only the 16x16 statistics cross back.
            let pixels = gray(128, ((700 + BLUR_PAD * 2) * (441 + BLUR_PAD * 2)) as usize);
            engine.upload_source(&pixels, 700 + BLUR_PAD * 2, 441 + BLUR_PAD * 2)?;
            let material = Material { luma: Some(0.5), light: false, dwell: 0, fresh: false };
            let start = Instant::now();
            for _ in 0..30 {
                let sample = engine.sample_backdrop()?;
                engine.update_local_tint(&sample, SAMPLE_W, SAMPLE_H, &material)?;
                engine.present(&material, 1.0)?;
            }
            eprintln!("full material 700x441: {:.2}ms/frame CPU submission+Present (not GPU completion)", start.elapsed().as_secs_f64() * 1000.0 / 30.0);
            Ok(())
        })();
        unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(window.hwnd()); }
        result.expect("optical GPU probe");
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
    fn geometry_cache_keeps_the_two_hottest_boxes() {
        // A show cycle is bar box, then settled box. Both must survive it:
        // regenerating either is ~40ms of work the reveal waits on.
        let mut cache: GeometryCache<u32> = GeometryCache::new();
        cache.put((700, 60, 1), 1);
        cache.put((700, 441, 1), 2);
        assert_eq!(cache.get((700, 60, 1)), Some(1));
        assert_eq!(cache.get((700, 441, 1)), Some(2));
        // A third box evicts the coldest; the settled box lives on.
        cache.put((700, 300, 1), 3);
        assert_eq!(cache.get((700, 60, 1)), None, "coldest box is evicted");
        assert_eq!(cache.get((700, 441, 1)), Some(2), "settled box survives a hide");
        assert_eq!(cache.get((700, 300, 1)), Some(3));
        assert_eq!(cache.entries.len(), GEOMETRY_CACHE_DEPTH);
        // Identity includes scale: a DPI change must never reuse pixels.
        assert_eq!(cache.get((700, 300, 2)), None);
        // Re-inserting a box replaces it rather than deepening the cache.
        cache.put((700, 300, 1), 4);
        assert_eq!(cache.get((700, 300, 1)), Some(4));
        assert_eq!(cache.entries.len(), GEOMETRY_CACHE_DEPTH);
    }

    #[test]
    fn spring_overshoots_slightly_then_settles_independent_of_frame_rate() {
        // Leaves most of the way open, then overshoots and settles.
        assert_eq!(swell_bend(0.0), SWELL_START);
        let peak = (0..420).map(|ms| swell_bend(ms as f32)).fold(0.0_f32, f32::max);
        // Overshoot is proportional to the distance left to travel: with the
        // spring leaving from SWELL_START it is a hair, never a bounce.
        assert!(peak > 1.001 && peak < 1.06, "overshoot out of band: {peak}");
        assert_eq!(swell_bend(SWELL_MS), 1.0);
        assert_eq!(swell_bend(SWELL_MS * 5.0), 1.0);
        let mut a = super::Spring { value: 0.0, velocity: 0.0 };
        let mut b = a;
        a.step(1.0, 0.1);
        for _ in 0..10 { b.step(1.0, 0.01); }
        assert!((a.value - b.value).abs() < 0.0001);
        assert!((a.velocity - b.velocity).abs() < 0.001);
        for _ in 0..100 { b.step(0.88, 0.016); }
        assert_eq!(b.value, 0.88);
        for _ in 0..100 { b.step(1.0, 0.016); }
        assert_eq!(b.value, 1.0);
    }

    #[test]
    fn continuous_corner_points_match_lens_boundary() {
        for (x, y) in super::corner_offsets() {
            let (depth, nx, ny) = super::lens_profile(x, y, 160.0, 96.0, super::CORNER_RADIUS);
            assert!(depth.abs() < 0.001, "corner mismatch: {x},{y} -> {depth}");
            assert!(nx.is_finite() && ny.is_finite());
            assert!((nx.hypot(ny) - 1.0).abs() < 0.0001);
        }
        let (d1, _, _) = super::lens_profile(8.0, 0.0, 160.0, 96.0, 8.0);
        let (d2, _, _) = super::lens_profile(16.0, 0.0, 320.0, 192.0, 16.0);
        assert_eq!(d1, d2);
        assert!(super::lens_clip_css().starts_with("polygon("));
    }

    #[test]
    fn rounded_mask_is_transparent_outside_and_opaque_inside() {
        let (w, h) = (40u32, 24u32);
        let mask = super::rounded_mask(w, h, 1.0, 8.0);
        let alpha = |x: u32, y: u32| mask[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(w / 2, h / 2), 255, "centre must be opaque");
        assert_eq!(alpha(0, 0), 0, "corner must be transparent");
        assert_eq!(alpha(w - 1, h - 1), 0, "corner must be transparent");
        assert_eq!(alpha(w / 2, 0), 255, "straight edge must stay opaque");
        assert_eq!(alpha(0, h / 2), 255, "straight edge must stay opaque");
    }

    #[test]
    fn thickness_tail_bends_past_the_bevel_but_not_the_center() {
        let (w, h) = (160usize, 96usize);
        let map = super::bezel_bytes(w, h);
        let red = |x: usize, y: usize| map[(y * w + x) * 4 + 2] as i32;
        let mid = h / 2;
        // 40 px in, the 32 px rim is already zero; the slab tail must
        // still bend toward the pane, and monotonically fall off inward.
        let rim = red(4, mid);
        let tail = red(40, mid);
        let centre = red(w / 2, mid);
        assert!(rim > tail, "rim {rim} must bend more than tail {tail}");
        assert!(tail > centre + 2, "slab tail is gone at 40 px ({tail})");
        assert!(
            (centre - 128).abs() <= 1,
            "center must stay neutral, got {centre}"
        );
    }

    #[test]
    fn silhouette_growth_only_expands_the_cut() {
        let base = super::lens_region_points(700, 441, 1.0, 0.0);
        let grown = super::lens_region_points(700, 441, 1.0, super::SILHOUETTE_GROW_PX);
        assert_eq!(base.len(), grown.len());
        let radius = super::CORNER_RADIUS as f64;
        let per_corner = super::corner_offsets().len();
        for (i, ((bx, by), (gx, gy))) in base.iter().zip(&grown).enumerate() {
            let (cx, cy) = match i / per_corner {
                0 => (radius, radius),
                1 => (700.0 - radius, radius),
                2 => (700.0 - radius, 441.0 - radius),
                _ => (radius, 441.0 - radius),
            };
            let before = (bx - cx).hypot(by - cy);
            let after = (gx - cx).hypot(gy - cy);
            assert!(
                (after - before - super::SILHOUETTE_GROW_PX).abs() < 0.001,
                "corner {i} grew {before} -> {after}"
            );
            assert!(*gx >= -1.0 && *gy >= -1.0 && *gx <= 701.0 && *gy <= 442.0);
        }
    }

    /// Contact shadow stays clear under the panel and fades outward, so it
    /// reads as a pane sitting on the desktop instead of a dark frame.
    #[test]
    fn shadow_is_clear_under_the_panel_and_fades_outward() {
        let (pw, ph, margin) = (200usize, 120usize, super::SHADOW_MARGIN as usize);
        let (w, h) = (pw + margin * 2, ph + margin * 2);
        let bytes = super::shadow_bytes(w, h, pw, ph, margin, 1.0);
        let alpha = |x: usize, y: usize| bytes[(y * w + x) * 4 + 3];
        assert_eq!(alpha(margin + pw / 2, margin + ph / 2), 0, "panel interior darkened");
        let near = alpha(margin + pw / 2, margin + ph + 2);
        assert!(near > 0, "no shadow under the panel");
        assert_eq!(alpha(margin + pw / 2, h - 1), 0, "shadow never reaches zero");
        let far = alpha(margin + pw / 2, margin + ph + 18);
        assert!(near > far, "shadow must fade outward: {near} vs {far}");
        // Symmetric: a pane sitting flat casts the same on all four sides.
        let (mx, my) = (margin + pw / 2, margin + ph / 2);
        assert_eq!(alpha(mx - pw / 2 - 2, my), alpha(mx + pw / 2 + 1, my), "left/right differ");
        assert_eq!(alpha(mx, my - ph / 2 - 2), alpha(mx, my + ph / 2 + 1), "top/bottom differ");
    }

    /// The region polygon spans the whole panel box, so translating it onto
    /// the client origin (`client_origin_in_window`) lands its straight
    /// edges on the client's far edges. Anchored at the window origin
    /// instead, the region shaved the invisible frame width off the page's
    /// right and bottom.
    #[test]
    fn region_polygon_spans_the_full_panel_box() {
        let (w, h) = (700u32, 441u32);
        let points = super::lens_region_points(w, h, 1.0, 0.0);
        let (min_x, max_x) = points
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
        let (min_y, max_y) = points
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
        assert!(min_x <= 0.0 && min_y <= 0.0, "box origin not covered");
        assert!(max_x >= f64::from(w), "right edge short: {max_x}");
        assert!(max_y >= f64::from(h), "bottom edge short: {max_y}");
    }

    #[test]
    fn local_tint_is_bounded_and_preserves_backdrop_hue() {
        let grid = super::backdrop_grid(&bgra(30, 60, 180, 64 * 64), 64, 64, (0.4, 0.4, 0.4, 0.2));
        for c in grid {
            assert!(c[0] > c[1] && c[1] > c[2]);
            assert!(c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
            assert!(c[3] < 0.09);
        }
        let mut split = gray(0, 64 * 64);
        for y in 0..64 { for x in 32..64 { split[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3].fill(255); } }
        let grid = super::backdrop_grid(&split, 64, 64, (0.05, 0.05, 0.05, 0.2));
        assert!(grid[3][3] > grid[0][3], "bright region needs stronger dark scrim");
    }

    #[test]
    fn rim_is_a_fading_glint_not_an_inset_outline() {
        let mut previous = super::rim_light(0.0);
        for step in 1..=320 {
            let current = super::rim_light(step as f32 / 10.0);
            assert!(current <= previous, "secondary ridge at depth {}", step as f32 / 10.0);
            previous = current;
        }
        assert!(super::rim_light(2.0) < 0.055);
        // Diagonal of a circular corner removes 0.293R, not the
        // 0.159R a sqrt-cos profile did (which read as square).
        let mid = super::corner_offsets();
        let (dx, dy) = mid[mid.len() / 2];
        let expected = super::CORNER_RADIUS * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
        assert!((dx - expected).abs() < 0.01 && (dy - expected).abs() < 0.01, "corner is not circular: {dx},{dy}");
        let layers = super::surface_layers(160, 96, 1.0);
        let alpha = |x: usize| layers[2][(160 + x) * 4 + 3];
        assert!(
            alpha(16) == alpha(140),
            "top highlight must be uniform along the edge: {} vs {}",
            alpha(16),
            alpha(140)
        );
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
    fn the_swell_opens_most_of_the_way_and_ends_exactly_once() {
        // The swell drives the lens from nearly open to full on a static
        // desktop. It must end: a swell that stays armed re-presents forever.
        let mut c = ShowCycle { refresh: false, swell: None };
        assert_eq!(c.bend(), (1.0, false), "no show means no swell");
        assert!(!c.swelling());
        c.arm();
        assert!(c.swelling());
        let (start, running) = c.bend_at(0.0);
        // Opens most of the way on the first frame: the panel is already on
        // screen when this runs, so a flat start reads as a late bend.
        assert_eq!(start, SWELL_START);
        assert!(start > 0.8, "the edge bend must be there on the reveal frame");
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

}
