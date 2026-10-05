//! Native glass spike, step 1: composition proof.
//!
//! A transparent D3D11 child window under the WebView presenting a flat
//! translucent tint. If this rectangle shows through the page, a native
//! layer can live beneath WebView2 and later steps (effect, capture)
//! have somewhere to render. Everything here is gated on
//! `NEX_REFRACT_LAB=1`; flag off means zero behavior change.
//!
//! Logs use the `[nex][refract]` prefix: init decisions, HRESULTs on
//! failure, and every present/resize (temporary while proving this out).

#![cfg(target_os = "windows")]

use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_VIEWPORT, ID3D11Device,
    ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SCALING, DXGI_SCALING_NONE,
    DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_ALPHA_MODE_UNSPECIFIED,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, MoveWindow, SetWindowPos,
    HWND_BOTTOM, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE,
    WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows::{core::w, Win32::System::LibraryLoader::GetModuleHandleW};

/// Env flag gating the whole spike. String compare keeps `=0`/unset off.
pub fn enabled() -> bool {
    std::env::var("NEX_REFRACT_LAB")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Solid spike tint, premultiplied RGBA: visible teal, ~45% opaque.
const TINT: [f32; 4] = [0.045, 0.225, 0.2475, 0.45];
/// Test-hook tint: opaque red, unmistakable.
const TOP_TINT: [f32; 4] = [1.0, 0.12, 0.12, 1.0];

/// Temporary z-order probe (spike only): force the glass child ABOVE the
/// WebView. Clicks die there — look only. Decides whether the swapchain
/// reaches DWM at all (red visible) or never composites (still nothing).
fn force_top() -> bool {
    std::env::var("NEX_REFRACT_TOP")
        .map(|v| v == "1")
        .unwrap_or(false)
}

pub struct GlassLayer {
    hwnd: HWND,
    device: ID3D11Device,
    swapchain: IDXGISwapChain1,
    context: ID3D11DeviceContext,
    rtv: Option<ID3D11RenderTargetView>,
    width: u32,
    height: u32,
    tint: [f32; 4],
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

/// Create the glass child for a parent overlay window. `None` when the
/// flag is off (info-logged) or on any failure (warn-logged, caller
/// falls back to Acrylic). Created *before* the WebView so it sits
/// lower in z-order without ever needing the WebView's HWND.
pub fn create_for_window(parent_sys: isize, logical_w: f64, logical_h: f64, scale: f64) -> Option<GlassLayer> {
    if !enabled() {
        crate::logging::info("[nex][refract] lab flag off — acrylic path");
        return None;
    }
    match create_inner(parent_sys, logical_w, logical_h, scale) {
        Ok(layer) => {
            crate::logging::info(&format!(
                "[nex][refract] glass child live hwnd={:?} {}x{}px",
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

fn create_inner(parent_sys: isize, logical_w: f64, logical_h: f64, scale: f64) -> Result<GlassLayer, String> {
    let width = physical_px(logical_w, scale);
    let height = physical_px(logical_h, scale);

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
        // ERROR_CLASS_ALREADY_EXISTS (1410) just means a previous run
        // in this process registered it; carry on either way unless it
        // is some other failure... indistinguishable cheaply, so log
        // and continue — CreateWindowExW is the real test.
        crate::logging::info(&format!(
            "[nex][refract] RegisterClassW returned 0, continuing (last error {})",
            last_error_code()
        ));
    }

    let parent = HWND(parent_sys as *mut std::ffi::c_void);
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            width as i32,
            height as i32,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW child failed: {e:?} (last error {})", last_error_code()))?
    };
    // Creation order should already put us below the not-yet-created
    // WebView; pin it explicitly so z-order is evidence, not luck.
    // NEX_REFRACT_TOP=1 inverts this for the compositing probe.
    let top = force_top();
    let (anchor, anchor_name) = if top { (HWND_TOP, "TOP(test hook)") } else { (HWND_BOTTOM, "BOTTOM") };
    if let Err(error) = unsafe {
        SetWindowPos(
            hwnd,
            Some(anchor),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE,
        )
    } {
        crate::logging::warn(&format!("[nex][refract] SetWindowPos {anchor_name} failed: {error:?}"));
    } else {
        crate::logging::info(&format!("[nex][refract] glass z-order pinned {anchor_name}"));
    }
    if top {
        crate::logging::warn("[nex][refract] TEST HOOK ACTIVE: opaque red above WebView, clicks blocked — look only");
    }

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
    // The textbook desc fails on some machines with DXGI_ERROR_INVALID_CALL
    // and the OS won't say which field. Try combos in order, log every
    // HRESULT, keep the first that works. UNSPECIFIED alpha is opaque —
    // diagnostic only (proves device + HWND are fine); real glass needs
    // PREMULTIPLIED.
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
    let variants: [(&str, DXGI_SCALING, DXGI_ALPHA_MODE); 3] = [
        ("premultiplied+stretch", desc.Scaling, desc.AlphaMode),
        (
            "premultiplied+none",
            DXGI_SCALING_NONE,
            DXGI_ALPHA_MODE_PREMULTIPLIED,
        ),
        (
            "unspecified+stretch",
            DXGI_SCALING_STRETCH,
            DXGI_ALPHA_MODE_UNSPECIFIED,
        ),
    ];
    let mut swapchain: Option<IDXGISwapChain1> = None;
    let mut used_variant = "";
    for (name, scaling, alpha) in variants {
        desc.Scaling = scaling;
        desc.AlphaMode = alpha;
        match unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) } {
            Ok(chain) => {
                crate::logging::info(&format!("[nex][refract] swapchain ok via {name}"));
                swapchain = Some(chain);
                used_variant = name;
                break;
            }
            Err(error) => crate::logging::warn(&format!(
                "[nex][refract] swapchain {name} failed: {error:?}"
            )),
        }
    }
    let swapchain = swapchain.ok_or("all swapchain desc variants failed")?;
    if used_variant.starts_with("unspecified") {
        crate::logging::warn(
            "[nex][refract] running OPAQUE diagnostic fallback — transparency still unproven",
        );
    }

    let mut layer = GlassLayer {
        hwnd,
        device,
        swapchain,
        context,
        rtv: None,
        width,
        height,
        tint: if top { TOP_TINT } else { TINT },
    };
    layer.recreate_target()?;
    layer.present()?;
    Ok(layer)
}

impl GlassLayer {
    fn recreate_target(&mut self) -> Result<(), String> {
        let texture: ID3D11Texture2D = unsafe {
            self.swapchain
                .GetBuffer(0)
                .map_err(|e| format!("swapchain GetBuffer failed: {e:?}"))?
        };
        let mut rtv: Option<ID3D11RenderTargetView> = None;
        unsafe {
            self.device
                .CreateRenderTargetView(&texture, None, Some(&mut rtv as *mut _))
                .map_err(|e| format!("CreateRenderTargetView failed: {e:?}"))?;
        }
        self.rtv = rtv;
        Ok(())
    }

    /// Present the current tint. Surface loss (e.g. device reset) is an
    /// Err the caller logs; no recovery in step 1.
    pub fn present(&self) -> Result<(), String> {
        let rtv = self.rtv.as_ref().ok_or("glass has no render target")?;
        unsafe {
            self.context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            let viewport = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.width as f32,
                Height: self.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            self.context.RSSetViewports(Some(&[viewport]));
            self.context.ClearRenderTargetView(rtv, &self.tint);
            self.swapchain
                .Present(1, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("Present failed: {e:?}"))?;
        }
        crate::logging::info(&format!(
            "[nex][refract] presented {}x{}px tint",
            self.width, self.height
        ));
        Ok(())
    }

    /// Track the panel size (physical px). No-op when unchanged.
    pub fn resize_for_logical(&mut self, logical_w: f64, logical_h: f64, scale: f64) {
        let width = physical_px(logical_w, scale);
        let height = physical_px(logical_h, scale);
        if width == self.width && height == self.height {
            return;
        }
        crate::logging::info(&format!(
            "[nex][refract] resize {}x{} -> {}x{}px",
            self.width, self.height, width, height
        ));
        unsafe {
            let _ = MoveWindow(self.hwnd, 0, 0, width as i32, height as i32, true);
        }
        // RTV must be released before ResizeBuffers.
        self.rtv.take();
        let result = unsafe {
            self.swapchain
                .ResizeBuffers(0, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_FLAG(0))
        };
        match result {
            Ok(()) => {
                // Track the new size BEFORE recreating + presenting: both
                // use self.width/height for the viewport and the log line.
                // Reverted on failure so a later retry isn't a no-op.
                let (old_w, old_h) = (self.width, self.height);
                self.width = width;
                self.height = height;
                if let Err(error) = self.recreate_target().and_then(|_| self.present()) {
                    self.width = old_w;
                    self.height = old_h;
                    crate::logging::warn(&format!("[nex][refract] resize present failed: {error}"));
                }
            }
            Err(error) => crate::logging::warn(&format!("[nex][refract] ResizeBuffers failed: {error:?}")),
        }
    }
}
