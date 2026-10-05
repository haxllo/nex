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
//! Everything here is gated on `NEX_REFRACT_LAB=1`; flag off means
//! zero behavior change. Logs use the `[nex][refract]` prefix.

#![cfg(target_os = "windows")]

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DISPLACEMENTMAP_PROP_SCALE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_FLOAT,
    CLSID_D2D1DisplacementMap, CLSID_D2D1GaussianBlur, ID2D1Bitmap1,
    ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Factory1, ID2D1Image,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
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
    CreateWindowExW, DefWindowProcW, GetWindowRect, RegisterClassW, SetWindowDisplayAffinity,
    SetWindowPos, ShowWindow, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WDA_EXCLUDEFROMCAPTURE,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::{core::w, Win32::System::LibraryLoader::GetModuleHandleW};

/// Env flag gating the whole spike. String compare keeps `=0`/unset off.
pub fn enabled() -> bool {
    std::env::var("NEX_REFRACT_LAB")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Step-2 effect knobs (logged at setup). Blur in px stddev, bend as
/// displacement scale, bezel margin in px, checker cell in px.
const BLUR_STDDEV: f32 = 8.0;
const BEND_SCALE: f32 = 24.0;
const BEZEL_MARGIN: usize = 28;
const CHECKER_CELL: usize = 64;

/// Render cap: live frames present at most this often.
const MIN_FRAME_MS: u128 = 33;

/// Host -> worker config. `main_sys == 0` means unchanged.
struct GlassConfig {
    visible: bool,
    main_sys: isize,
}

/// Host-facing handle. All methods are UI-thread window ops plus a
/// config send; every D3D/D2D object lives on the worker.
pub struct GlassLayer {
    window: GlassWindow,
    tx: Sender<GlassConfig>,
}

struct GlassWindow {
    hwnd: HWND,
}

impl GlassLayer {
    /// Create the glass top-level window + worker. `None` when the flag
    /// is off (info-logged) or on any failure (warn-logged, caller falls
    /// back to Acrylic). Starts hidden; the host shows it with the panel.
    pub fn create_for_window(
        main_sys: isize,
        logical_w: f64,
        logical_h: f64,
        scale: f64,
    ) -> Option<GlassLayer> {
        if !enabled() {
            crate::logging::info("[nex][refract] lab flag off — acrylic path");
            return None;
        }
        match GlassWindow::create(main_sys, logical_w, logical_h, scale) {
            Ok(window) => {
                let (tx, rx) = channel();
                // HWND is a raw pointer (not Send): move it as isize and
                // rebuild it on the worker.
                let hwnd_sys = window.hwnd().0 as isize;
                match std::thread::Builder::new()
                    .name("glass-capture".into())
                    .spawn(move || worker_main(hwnd_sys, rx))
                {
                    Ok(_) => {
                        crate::logging::info("[nex][refract] capture worker spawned");
                        Some(GlassLayer { window, tx })
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
    pub fn sync_to_main(&self, main_sys: isize) {
        self.window.move_below(main_sys);
        let _ = self.tx.send(GlassConfig {
            visible: true,
            main_sys,
        });
    }

    /// Show/hide with the panel. No animation, no activation.
    pub fn set_visible(&self, show: bool) {
        self.window.show(show);
        let _ = self.tx.send(GlassConfig {
            visible: show,
            main_sys: 0,
        });
    }
}

impl GlassWindow {
    fn create(
        main_sys: isize,
        logical_w: f64,
        logical_h: f64,
        scale: f64,
    ) -> Result<GlassWindow, String> {
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
        let (x, y, width, height) = main_rect(main_sys)
            .map(|(x, y, w, h)| (x, y, w, h))
            .unwrap_or((0, 0, physical_px(logical_w, scale), physical_px(logical_h, scale)));

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
        crate::logging::info(&format!(
            "[nex][refract] glass top-level hwnd={hwnd:?} {width}x{height}px at ({x},{y})"
        ));
        Ok(GlassWindow { hwnd })
    }

    fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Same rect as main, ordered directly below it, never activating.
    fn move_below(&self, main_sys: isize) {
        let Some((x, y, width, height)) = main_rect(main_sys) else {
            crate::logging::warn("[nex][refract] move_below: main rect unreadable");
            return;
        };
        let main = HWND(main_sys as *mut std::ffi::c_void);
        if let Err(error) = unsafe {
            SetWindowPos(self.hwnd, Some(main), x, y, width as i32, height as i32, SWP_NOACTIVATE)
        } {
            crate::logging::warn(&format!("[nex][refract] move_below failed: {error:?}"));
        }
    }

    fn show(&self, show: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if show { SW_SHOWNA } else { SW_HIDE });
        }
        crate::logging::info(&format!("[nex][refract] glass visible={show}"));
    }
}

/// Worker-owned renderer: every D3D/D2D object. Built lazily on the
/// first visible frame; rebuilt on size change.
struct Engine {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain1,
    d2d: ID2D1DeviceContext,
    blur: Option<ID2D1Effect>,
    displace: Option<ID2D1Effect>,
    source: Option<ID2D1Bitmap1>,
    map: Option<ID2D1Bitmap1>,
    width: u32,
    height: u32,
}

impl Engine {
    /// Build everything for a panel size: device, swapchain (first
    /// working desc wins), D2D graph, checkerboard fallback content.
    fn build(hwnd: HWND, width: u32, height: u32) -> Result<Engine, String> {
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
            Width: width,
            Height: height,
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

        let mut engine = Engine {
            device,
            context,
            swapchain,
            d2d,
            blur: None,
            displace: None,
            source: None,
            map: None,
            width,
            height,
        };
        engine.rebuild_content()?;
        Ok(engine)
    }

    /// Rebuild size-dependent content: procedural checkerboard fallback
    /// + displacement map + the blur→displacement pair wired between
    /// them. Live frames overwrite the source via ingest_frame.
    fn rebuild_content(&mut self) -> Result<(), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        let src_bytes = checker_bytes(w, h);
        let map_bytes = bezel_bytes(w, h);
        let props = bitmap_props(D2D1_BITMAP_OPTIONS_NONE);
        let source: ID2D1Bitmap1 = unsafe {
            self.d2d
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: self.width,
                        height: self.height,
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
                        width: self.width,
                        height: self.height,
                    },
                    Some(map_bytes.as_ptr() as *const _),
                    (w * 4) as u32,
                    &props as *const _,
                )
                .map_err(|e| format!("map CreateBitmap failed: {e:?}"))?
        };
        let blur: ID2D1Effect = unsafe {
            self.d2d
                .CreateEffect(&CLSID_D2D1GaussianBlur as *const _)
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
            blur.SetInput(0, &source, true);
        }
        let displace: ID2D1Effect = unsafe {
            self.d2d
                .CreateEffect(&CLSID_D2D1DisplacementMap as *const _)
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
            // X=R and Y=G are the defaults; setting them explicitly
            // fails E_INVALIDARG on some boxes. The map matches.
            displace.SetInput(0, &blur_output(&blur)?, true);
            displace.SetInput(1, &map, true);
        }
        self.source = Some(source);
        self.map = Some(map);
        self.blur = Some(blur);
        self.displace = Some(displace);
        Ok(())
    }

    /// Upload one captured frame over the source bitmap. Size drift
    /// (panel resized mid-flight) drops the frame; the rebuild already
    /// queued covers the next one.
    fn ingest_frame(&self, pixels: &[u8], w: u32, h: u32) -> Result<(), String> {
        let source = self.source.as_ref().ok_or("no source bitmap")?;
        if w != self.width || h != self.height {
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

    /// Draw the effect into the current backbuffer and present it. The
    /// target rebinds every frame: flip-model buffers rotate.
    fn render_frame(&self) -> Result<(), String> {
        let displace = self.displace.as_ref().ok_or("glass effect not built")?;
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
            self.d2d.DrawImage(
                &blur_output(displace)?,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            self.d2d
                .EndDraw(None, None)
                .map_err(|e| format!("EndDraw failed: {e:?}"))?;
            self.swapchain
                .Present(1, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("Present failed: {e:?}"))?;
        }
        Ok(())
    }

    /// ResizeBuffers + content rebuild for a new panel size.
    fn resize_buffers(&mut self, width: u32, height: u32) -> Result<(), String> {
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

/// Procedural backdrop: gray checkerboard so blur + bend read clearly,
/// plus a bright frame inset in the bend zone — straight lines rendering
/// curved is the unmistakable bend proof.
fn checker_bytes(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            // Frame: 4px band, 6px inside every edge.
            let in_frame = x >= 6
                && x < w.saturating_sub(6)
                && y >= 6
                && y < h.saturating_sub(6)
                && (x < 10
                    || x >= w.saturating_sub(10)
                    || y < 10
                    || y >= h.saturating_sub(10));
            if in_frame {
                out.extend_from_slice(&[60, 60, 230, 255]);
                continue;
            }
            let cell = ((x / CHECKER_CELL) + (y / CHECKER_CELL)) % 2;
            let c = if cell == 0 { 200u8 } else { 110u8 };
            out.extend_from_slice(&[c, c, c, 255]);
        }
    }
    out
}

/// Convex-rim displacement map: neutral gray center, edges encoding an
/// outward push (Win2D samples Source[p + Amount·(channel − 0.5)]).
fn bezel_bytes(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let margin = BEZEL_MARGIN as f64;
    for y in 0..h {
        for x in 0..w {
            let mut ox = 0.0;
            let mut oy = 0.0;
            if (x as f64) < margin {
                ox = -((margin - x as f64) / margin);
            } else if x > w.saturating_sub(1 + BEZEL_MARGIN) {
                ox = ((x - (w - 1 - BEZEL_MARGIN)) as f64) / margin;
            }
            if (y as f64) < margin {
                oy = -((margin - y as f64) / margin);
            } else if y > h.saturating_sub(1 + BEZEL_MARGIN) {
                oy = ((y - (h - 1 - BEZEL_MARGIN)) as f64) / margin;
            }
            let r = (128.0 + ox * 127.0).clamp(0.0, 255.0) as u8;
            let g = (128.0 + oy * 127.0).clamp(0.0, 255.0) as u8;
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

fn worker_main(glass_sys: isize, rx: Receiver<GlassConfig>) {
    let hwnd = HWND(glass_sys as *mut std::ffi::c_void);
    crate::logging::info("[nex][refract] worker alive (D3D11/DXGI/D2D need no COM init here)");
    let mut engine: Option<Engine> = None;
    let mut capture: Option<Capture> = None;
    let mut visible = false;
    let mut main_sys: isize = 0;
    let mut last_present = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let mut stat_t = Instant::now();
    let (mut n_cap, mut n_pre, mut copy_us) = (0u64, 0u64, 0u64);

    loop {
        while let Ok(cfg) = rx.try_recv() {
            visible = cfg.visible;
            if cfg.main_sys != 0 {
                main_sys = cfg.main_sys;
            }
        }
        if !visible || main_sys == 0 {
            if capture.take().is_some() {
                crate::logging::info("[nex][refract] capture stopped (hidden)");
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let Some((mx, my, mw, mh)) = main_rect(main_sys) else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        // Engine ensure / resize (buffers only when one exists).
        if engine.is_none() {
            match Engine::build(hwnd, mw, mh) {
                Ok(e) => engine = Some(e),
                Err(error) => {
                    crate::logging::warn(&format!("[nex][refract] engine build failed: {error}"));
                    std::thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            }
        } else if let Some(e) = engine.as_mut() {
            if e.width != mw || e.height != mh {
                if let Err(error) = e.resize_buffers(mw, mh) {
                    crate::logging::warn(&format!("[nex][refract] engine resize failed: {error}"));
                    engine = None;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
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
                crate::logging::warn("[nex][refract] protected content masked (renders black)");
            }
        } else {
            cap.protected_logged = false;
        }
        // FPS cap: skip upload/draw but always release.
        if Instant::now().duration_since(last_present) < Duration::from_millis(MIN_FRAME_MS as u64) {
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let (bx, by, bw, bh) = clamp_panel_box(mx, my, mw, mh, cap.origin_x, cap.origin_y, cap.frame_w, cap.frame_h);
        if bw == 0 || bh == 0 {
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            continue;
        }
        let frame_ok = (|| -> Result<u64, String> {
            let tex: ID3D11Texture2D = resource
                .as_ref()
                .ok_or("empty frame resource")?
                .cast()
                .map_err(|e| format!("frame QI texture failed: {e:?}"))?;
            let srcbox = D3D11_BOX {
                left: bx,
                top: by,
                front: 0,
                right: bx + bw,
                bottom: by + bh,
                back: 1,
            };
            unsafe {
                eng.context.CopySubresourceRegion(
                    &cap.staging, 0, 0, 0, 0, &tex, 0, Some(&srcbox as *const _),
                );
            }
            let mut mapped: D3D11_MAPPED_SUBRESOURCE = unsafe { std::mem::zeroed() };
            unsafe {
                eng.context
                    .Map(&cap.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped as *mut _))
                    .map_err(|e| format!("staging Map failed: {e:?}"))?;
            }
            let t0 = Instant::now();
            let mut pixels = vec![0u8; (bw * bh * 4) as usize];
            for row in 0..bh {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (mapped.pData as *const u8).add((row * mapped.RowPitch) as usize),
                        pixels.as_mut_ptr().add((row * bw * 4) as usize),
                        (bw * 4) as usize,
                    );
                }
            }
            unsafe {
                eng.context.Unmap(&cap.staging, 0);
            }
            let us = t0.elapsed().as_micros() as u64;
            eng.ingest_frame(&pixels, bw, bh)?;
            eng.render_frame()?;
            let _ = unsafe { cap.duplication.ReleaseFrame() };
            Ok(us)
        })();
        match frame_ok {
            Ok(us) => {
                copy_us += us;
                n_pre += 1;
                last_present = Instant::now();
            }
            Err(error) => {
                let _ = unsafe { cap.duplication.ReleaseFrame() };
                crate::logging::warn(&format!("[nex][refract] frame failed: {error}"));
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        if stat_t.elapsed() >= Duration::from_secs(1) {
            let avg = if n_pre > 0 { copy_us / n_pre } else { 0 };
            crate::logging::info(&format!(
                "[nex][refract] captured/s={n_cap} presented/s={n_pre} copy_avg={avg}us"
            ));
            stat_t = Instant::now();
            n_cap = 0;
            n_pre = 0;
            copy_us = 0;
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

fn physical_px(logical: f64, scale: f64) -> u32 {
    ((logical * scale).round() as u32).max(1)
}

fn main_rect(main_sys: isize) -> Option<(i32, i32, u32, u32)> {
    let main = HWND(main_sys as *mut std::ffi::c_void);
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(main, &mut rect as *mut _).is_err() } {
        return None;
    }
    let (w, h) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
    Some((rect.left, rect.top, w as u32, h as u32))
}
