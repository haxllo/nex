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
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT,
    DXGI_SWAP_EFFECT_DISCARD, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_ALPHA_MODE_UNSPECIFIED,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, EnumChildWindows, GetClassNameW, GetClientRect,
    GetParent, GetWindowLongW, IsWindowVisible,
    RegisterClassW, MoveWindow, SetWindowPos,
    GWL_EXSTYLE, HWND_BOTTOM, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE,
    WNDCLASSW, WNDENUMPROC, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP, WS_VISIBLE,
};
use windows::Win32::Foundation::RECT;
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

unsafe extern "system" fn enum_child_probe(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows_core::BOOL {
    let out = &mut *(lparam.0 as *mut Vec<String>);
    let mut class: [u16; 64] = [0; 64];
    let len = GetClassNameW(hwnd, &mut class);
    let name = String::from_utf16_lossy(&class[..len.max(0) as usize]);
    let mut rect: RECT = std::mem::zeroed();
    let _ = GetClientRect(hwnd, &mut rect as *mut _);
    let visible = IsWindowVisible(hwnd);
    out.push(format!(
        "hwnd={hwnd:?} class={name} visible={} rect={},{},{},{}",
        visible.0 != 0,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom
    ));
    windows_core::BOOL(1)
}

/// DWM cloak flag for an HWND (`"1"` cloaked, `"0"` live, `"err"` on failure).
fn dwm_cloaked(hwnd: HWND) -> String {
    let mut cloaked: u32 = 0;
    let hr = unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd.0,
            windows_sys::Win32::Graphics::Dwm::DWMWA_CLOAKED as u32,
            &mut cloaked as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    if hr == 0 {
        format!("{cloaked}")
    } else {
        format!("err({hr})")
    }
}

/// Fill the whole child client area solid blue via GDI. Returns what
/// happened; the caller logs it next to the swapchain verdict.
fn gdi_fill_probe(hwnd: HWND) -> &'static str {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC,
    };
    use windows_sys::Win32::Foundation::RECT as SysRect;
    unsafe {
        let hdc = GetDC(hwnd.0);
        if hdc.is_null() {
            return "getdc-failed";
        }
        // Client size is tracked by the layer; a generous fixed rect
        // covers it (overpaint outside clips harmlessly).
        let rect = SysRect {
            left: 0,
            top: 0,
            right: 4096,
            bottom: 4096,
        };
        // 0x00BBGGRR: pure blue, unmistakable next to swapchain red.
        let brush = CreateSolidBrush(0x00FF0000);
        FillRect(hdc, &rect, brush);
        DeleteObject(brush);
        ReleaseDC(hwnd.0, hdc);
    }
    "painted-blue"
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
    // Record what tao actually built: LAYERED vs NOREDIRECTIONBITMAP
    // decides which swapchain models can ever compose here.
    let ex_style = unsafe { GetWindowLongW(parent, GWL_EXSTYLE) } as u32;
    crate::logging::info(&format!(
        "[nex][refract] parent exstyle=0x{ex_style:08x} layered={} no_redirection_bitmap={}",
        ex_style & WS_EX_LAYERED.0 != 0,
        ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0,
    ));
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
    let mut used_variant = "";
    for (name, scaling, alpha, buffer_count, effect) in variants {
        desc.Scaling = scaling;
        desc.AlphaMode = alpha;
        desc.BufferCount = buffer_count;
        desc.SwapEffect = effect;
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

    /// Spike probe only: re-assert TOP ordering at show time, re-present,
    /// and log live HWND state (visibility + client rect). Creation-time
    /// pinning is worthless on its own — the WebView child is created
    /// after us and later siblings paint above. No-op unless NEX_REFRACT_TOP=1.
    pub fn repin_top_for_probe(&self) {
        if !force_top() {
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        // Spy++-style dump: every child of our parent (class, rect,
        // visibility) — answers whether the WebView is even a sibling
        // HWND and where our child sits among them.
        unsafe {
            if let Ok(parent) = GetParent(self.hwnd) {
                let mut kids: Vec<String> = Vec::new();
                let lparam = LPARAM(&mut kids as *mut Vec<String> as isize);
                let _ = EnumChildWindows(Some(parent), Some(enum_child_probe), lparam);
                for (index, info) in kids.iter().take(8).enumerate() {
                    crate::logging::warn(&format!("[nex][refract] child[{index}] {info}"));
                }
                crate::logging::warn(&format!("[nex][refract] child count={}", kids.len()));
            }
        }
        // DWM cloaked state: a cloaked window presents into the void.
        let cloaked = dwm_cloaked(self.hwnd);
        // GDI paint test (blue): if blue shows but swapchain red never
        // does, the HWND composes fine and only the DXGI binding is dead.
        let gdi = gdi_fill_probe(self.hwnd);
        let visible = unsafe { IsWindowVisible(self.hwnd) };
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let rect_ok = unsafe { GetClientRect(self.hwnd, &mut rect as *mut _).is_ok() };
        crate::logging::warn(&format!(
            "[nex][refract] probe repin TOP visible={} rect_ok={rect_ok} rect={},{},{},{} cloaked={cloaked} gdi={gdi}",
            visible.0 != 0,
            rect.left, rect.top, rect.right, rect.bottom
        ));
        if let Err(error) = self.present() {
            crate::logging::warn(&format!("[nex][refract] probe re-present failed: {error}"));
        }
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
