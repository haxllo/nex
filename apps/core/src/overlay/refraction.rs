//! Native glass spike: top-level glass window under the overlay.
//!
//! History: a transparent D3D11 *child* beneath the WebView presented
//! fine but never reached the screen (input worked, pixels didn't —
//! under flip, bitblt and GDI alike). Child surfaces don't compose
//! inside this parent, so the glass is a top-level window instead:
//! borderless, click-through, never activating, hidden from taskbar
//! and Alt-Tab, kept pixel-aligned below the main panel.
//!
//! Everything here is gated on `NEX_REFRACT_LAB=1`; flag off means
//! zero behavior change. Logs use the `[nex][refract]` prefix.

#![cfg(target_os = "windows")]

use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_BITMAP_OPTIONS,
    D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DISPLACEMENTMAP_PROP_SCALE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_FLOAT,
    CLSID_D2D1DisplacementMap, CLSID_D2D1GaussianBlur, ID2D1Bitmap1,
    ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory, ID2D1Factory1, ID2D1Image,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SCALING,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT,
    DXGI_SWAP_EFFECT_DISCARD, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_ALPHA_MODE_UNSPECIFIED,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetWindowRect, RegisterClassW,
    SetWindowPos, ShowWindow, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::Foundation::HMODULE;
use windows::{core::w, Win32::System::LibraryLoader::GetModuleHandleW};

/// Env flag gating the whole spike. String compare keeps `=0`/unset off.
pub fn enabled() -> bool {
    std::env::var("NEX_REFRACT_LAB")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Step-2 effect knobs (logged at setup). Blur in px stddev, bend as
/// displacement scale, bezel margin in px, checker cell in px.
const BLUR_STDDEV: f32 = 16.0;
const BEND_SCALE: f32 = 20.0;
const BEZEL_MARGIN: usize = 28;
const CHECKER_CELL: usize = 32;

pub struct GlassLayer {
    hwnd: HWND,
    // Held for lifetime + step 3 (capture copies need both devices).
    _device: ID3D11Device,
    swapchain: IDXGISwapChain1,
    _context: ID3D11DeviceContext,
    d2d: ID2D1DeviceContext,
    blur: Option<ID2D1Effect>,
    displace: Option<ID2D1Effect>,
    source: Option<ID2D1Bitmap1>,
    map: Option<ID2D1Bitmap1>,
    width: u32,
    height: u32,
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

/// Create the glass top-level window for the overlay. `None` when the
/// flag is off (info-logged) or on any failure (warn-logged, caller
/// falls back to Acrylic). Starts hidden at the main window's rect;
/// the host shows it with the panel and keeps it aligned.
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
    match create_inner(main_sys, logical_w, logical_h, scale) {
        Ok(layer) => {
            crate::logging::info(&format!(
                "[nex][refract] glass window live hwnd={:?} {}x{}px",
                layer.hwnd, layer.width, layer.height
            ));
            Some(layer)
        }
        Err(error) => {
            crate::logging::warn(&format!("[nex][refract] disabled, acrylic fallback: {error}"));
            None
        }
    }
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

fn create_inner(
    main_sys: isize,
    logical_w: f64,
    logical_h: f64,
    scale: f64,
) -> Result<GlassLayer, String> {
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

    // Start at the main window's rect (physical px, no DPI math);
    // sync_to_main re-aligns on every show anyway.
    let (x, y, width, height) = main_rect(main_sys).map(|(x, y, w, h)| (x, y, w, h)).unwrap_or_else(|| {
        (0, 0, physical_px(logical_w, scale), physical_px(logical_h, scale))
    });

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
    crate::logging::info(&format!(
        "[nex][refract] glass top-level hwnd={hwnd:?} {width}x{height}px at ({x},{y})"
    ));

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
    crate::logging::info(&format!("[nex][refract] d3d11 device ok, feature level 0x{:x}", feature_level.0));

    let factory: IDXGIFactory2 =
        unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }
            .map_err(|e| format!("CreateDXGIFactory2 failed: {e:?}"))?;
    // Desc combos tried in order, first win kept. A top-level window
    // removes the child-composition suspect, so premultiplied flip
    // leads again; bitblt stays as the compatibility fallback.
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
            windows::Win32::Graphics::Dxgi::DXGI_SCALING_NONE,
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

    let mut layer = GlassLayer {
        hwnd,
        _device: device,
        swapchain,
        _context: context,
        d2d,
        blur: None,
        displace: None,
        source: None,
        map: None,
        width,
        height,
    };
    layer.rebuild_content()?;
    layer.present()?;
    Ok(layer)
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

/// Output image of an effect. Effects derive from ID2D1Properties, not
/// ID2D1Image, so chaining and drawing go through GetOutput.
fn blur_output(blur: &ID2D1Effect) -> Result<ID2D1Image, String> {
    unsafe {
        blur.GetOutput()
            .map_err(|e| format!("effect GetOutput failed: {e:?}"))
    }
}

fn bitmap_props(options: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

/// Procedural backdrop: gray checkerboard so blur + bend read clearly.
fn checker_bytes(w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
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

impl GlassLayer {
    /// Rebuild size-dependent content: procedural source + displacement
    /// map bitmaps and the blur→displacement effect pair wired between
    /// them. Called at creation and on resize.
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
            // No channel selects: X=R and Y=G are already the defaults,
            // and setting them explicitly fails E_INVALIDARG on this box.
            // The map is generated R-for-X, G-for-Y to match.
            displace.SetInput(0, &blur_output(&blur)?, true);
            displace.SetInput(1, &map, true);
        }
        self.source = Some(source);
        self.map = Some(map);
        self.blur = Some(blur);
        self.displace = Some(displace);
        Ok(())
    }

    /// Render one effect frame into the current backbuffer and present
    /// it. The target is rebound every frame: flip-model buffers rotate.
    /// Static for now — creation and resize only; step 3 drives it per
    /// capture frame.
    pub fn present(&self) -> Result<(), String> {
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
                .CreateBitmapFromDxgiSurface(&surface, Some(&bitmap_props(D2D1_BITMAP_OPTIONS_TARGET) as *const _))
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
        crate::logging::info(&format!(
            "[nex][refract] rendered effect frame {}x{}px",
            self.width, self.height
        ));
        Ok(())
    }

    /// Align to the main window: same rect, ordered directly below it,
    /// never activating. Called on show and resize.
    pub fn sync_to_main(&mut self, main_sys: isize) {
        let Some((x, y, width, height)) = main_rect(main_sys) else {
            crate::logging::warn("[nex][refract] sync_to_main: main rect unreadable");
            return;
        };
        let main = HWND(main_sys as *mut std::ffi::c_void);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(main),
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE,
            );
        }
        if width == self.width && height == self.height {
            return;
        }
        crate::logging::info(&format!(
            "[nex][refract] sync {}x{} -> {}x{}px",
            self.width, self.height, width, height
        ));
        // Backbuffer target rebinds every frame (flip buffers rotate);
        // content bitmaps only rebuild here on size change.
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
                let (old_w, old_h) = (self.width, self.height);
                self.width = width;
                self.height = height;
                if let Err(error) = self.rebuild_content().and_then(|_| self.present()) {
                    self.width = old_w;
                    self.height = old_h;
                    crate::logging::warn(&format!("[nex][refract] sync present failed: {error}"));
                }
            }
            Err(error) => crate::logging::warn(&format!("[nex][refract] ResizeBuffers failed: {error:?}")),
        }
    }

    /// Show/hide with the panel. No animation, no activation.
    pub fn set_visible(&self, show: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if show { SW_SHOWNA } else { SW_HIDE });
        }
        crate::logging::info(&format!("[nex][refract] glass visible={show}"));
    }
}
