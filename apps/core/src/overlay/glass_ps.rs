//! The panel's material as one pixel shader, ported from the glass this one is
//! measured against (`glass.hlsl`, MIT, after tomacco/amazing-glass).
//!
//! Why a shader and not the effect graph: the previous path composited a
//! *bitmap mask* into the backdrop and left the window shape to a GDI region.
//! Both are quantised — a mask is only as fine as the bitmap it was baked from,
//! and a region has no anti-aliasing at all — so a corner could only ever be as
//! smooth as the mesh it was rasterised into, and every attempt to make the
//! radius larger made that steppiness more obvious. Here the coverage of the
//! silhouette is arithmetic done per pixel (`saturate(D * scale + 0.5)`, one
//! device pixel wide), so the corner is smooth at any radius, at any scale, and
//! costs nothing when the radius changes.
//!
//! The same pass does the whole look: the rim's bend (slope → incidence → Snell
//! → sideways shift over the thickness), dispersion, frost from a mip chain of
//! the backdrop, tint, the rim light, and the outer contact shadow. Nothing is
//! drawn by CSS and nothing is composited on the CPU.
//!
//! Passes, all full-screen triangles:
//! 1. `PSCrop` — copy the window plus padding out of the live desktop texture.
//! 2. `PSDown` ×n — 13-tap downsample, the frost pyramid.
//! 3. `PSGlass` — the material, into the composition swap chain's back buffer.

#![allow(dead_code)]

use std::ffi::c_void;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_FILTER, D3D11_FILTER_ANISOTROPIC,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_RENDER_TARGET_VIEW_DESC,
    D3D11_RENDER_TARGET_VIEW_DESC_0, D3D11_SAMPLER_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC,
    D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_TEX2D_RTV, D3D11_TEX2D_SRV, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
    D3D11_RTV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

mod embedded {
    // Written by `build.rs`. Empty when the build machine could not run the
    // compiler: `GlassPs::new` then reports that instead of drawing nothing.
    pub const VS_MAIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vs_main.dxbc"));
    pub const PS_CROP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps_crop.dxbc"));
    pub const PS_DOWN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps_down.dxbc"));
    pub const PS_GLASS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps_glass.dxbc"));
}

/// Highest mip of the frost pyramid the shader will ask for.
const MAX_LEVELS: u32 = 9;

/// The look, in the shader's own units (CSS px, and fractions of the shape).
/// The numbers are the reference's preset for a window, with the frost and
/// saturation of this app's material.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// Thickness of the glass as a share of the rim width: how far a ray is
    /// carried sideways at the edge.
    pub depth: f32,
    /// Rim width as a fraction of the short side.
    pub bezel: f32,
    /// ...capped, CSS px.
    pub max_bezel: f32,
    pub zoom: f32,
    /// Colour split at the rim, as a fraction of the shift.
    pub dispersion: f32,
    /// Frost, a Gaussian's standard deviation in CSS px.
    pub blur: f32,
    pub saturate: f32,
    pub lum: f32,
    pub contrast: f32,
    /// Rim light gain.
    pub rim: f32,
    /// Rim light line width, CSS px.
    pub rim_width: f32,
    /// Inner dark line on the side away from the light.
    pub shade: f32,
    /// Light direction, degrees (y down: -135 is the top left).
    pub light: f32,
    /// Share of the frost left on the outermost ring, so the bend stays visible.
    pub rim_frost: f32,
    pub tint: [f32; 4],
    /// Contact shadow: opacity at the outline, offset down, and reach, CSS px.
    pub shadow: [f32; 3],
}

impl Look {
    /// iOS-26-style clear glass: barely any frost, a thin lit rim, and the bend
    /// carried far enough in to be seen.
    pub fn clear(tint: [f32; 4]) -> Look {
        Look {
            depth: 2.4,
            bezel: 0.421,
            max_bezel: 44.0,
            zoom: 1.03,
            dispersion: 0.05,
            blur: 2.0,
            saturate: 1.32,
            lum: 1.0,
            contrast: 1.0,
            rim: 0.25,
            rim_width: 0.35,
            shade: 0.03,
            light: -135.0,
            rim_frost: 0.12,
            tint,
            shadow: [0.16, 2.0, 30.0],
        }
    }

    /// Acrylic: the same backdrop, the same silhouette, no lens.
    ///
    /// The bend (`depth`), the colour split (`dispersion`) and the middle zoom
    /// are zero here, so the shader's optics come out flat and the panel reads
    /// as a frosted sheet of what is behind it rather than as glass. The frost
    /// is deliberately much wider than the lens ever asks for — that wide, even
    /// blur is the whole look.
    ///
    /// There is **no rim light at all**. The lens rim is directional: it is lit
    /// from one corner and fades around the outline, so on a lens it reads as a
    /// bevel. On a flat sheet it reads as a border that is missing in places.
    /// Non-glass themes draw an even 1 px outline on the page instead (see
    /// `html.bg-acrylic #panel::after`), which is what a specular bevel is not.
    ///
    /// `rim_frost` is 1.0, which means "the frost is the same at the edge as in
    /// the middle". The lens drops it to 0.12 so the outermost ring samples the
    /// backdrop sharp and the bend stays visible; on a sheet with no bend that
    /// same term paints a ring of unblurred wallpaper around a frosted panel,
    /// which reads as a transparent rim.
    pub fn acrylic(tint: [f32; 4]) -> Look {
        Look {
            depth: 0.0,
            bezel: 0.421,
            max_bezel: 44.0,
            zoom: 1.0,
            dispersion: 0.0,
            blur: ACRYLIC_BLUR,
            saturate: 1.12,
            lum: 1.0,
            contrast: 0.98,
            rim: 0.0,
            rim_width: 0.0,
            shade: 0.0,
            light: -135.0,
            rim_frost: 1.0,
            tint,
            shadow: [0.16, 2.0, 30.0],
        }
    }
}

/// Frost width for the acrylic material, CSS px. Wide enough to be the taskbar
/// kind of blur at any display scale; the pyramid clamps it if the panel is too
/// small to carry it.
pub const ACRYLIC_BLUR: f32 = 18.0;

/// The constant buffer, field for field with `cbuffer CB` in the shader.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Cb {
    win: [f32; 4],
    reg: [f32; 4],
    shape: [f32; 4],
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    tint: [f32; 4],
    pass: [f32; 4],
    ext: [f32; 4],
    sub: [f32; 4],
    opt: [f32; 4],
    lens_a: [f32; 4],
    lens_b: [f32; 4],
    lens_c: [f32; 4],
    lens_t: [f32; 4],
    lens: [[f32; 4]; 3 * MAX_LENSES],
}

const MAX_LENSES: usize = 8;

/// The backdrop around the panel plus its frost pyramid.
struct Region {
    w: u32,
    h: u32,
    levels: u32,
    tex: ID3D11Texture2D,
    rtv: Vec<ID3D11RenderTargetView>,
    srv: Vec<ID3D11ShaderResourceView>,
    /// All levels at once, which is what the glass samples.
    full: ID3D11ShaderResourceView,
}

impl Region {
    unsafe fn new(dev: &ID3D11Device, w: u32, h: u32) -> Result<Region, String> {
        let levels = MAX_LEVELS.min(32 - w.min(h).leading_zeros()).max(1);
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe {
            dev.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: levels,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                },
                None,
                Some(&mut tex as *mut _),
            )
        }
        .map_err(|e| format!("[nex][glass] region texture failed: {e:?}"))?;
        let tex = tex.ok_or("region texture came back empty")?;
        let mut rtv = Vec::with_capacity(levels as usize);
        let mut srv = Vec::with_capacity(levels as usize);
        for mip in 0..levels {
            let mut r: Option<ID3D11RenderTargetView> = None;
            unsafe {
                dev.CreateRenderTargetView(
                    &tex,
                    Some(&D3D11_RENDER_TARGET_VIEW_DESC {
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                            Texture2D: D3D11_TEX2D_RTV { MipSlice: mip },
                        },
                    } as *const _),
                    Some(&mut r as *mut _),
                )
            }
            .map_err(|e| format!("[nex][glass] region rtv {mip} failed: {e:?}"))?;
            rtv.push(r.ok_or("region rtv came back empty")?);
            srv.push(unsafe { srv_mip(dev, &tex, mip, 1)? });
        }
        let full = unsafe { srv_mip(dev, &tex, 0, levels)? };
        Ok(Region { w, h, levels, tex, rtv, srv, full })
    }
}

unsafe fn srv_mip(
    dev: &ID3D11Device,
    tex: &ID3D11Texture2D,
    mip: u32,
    count: u32,
) -> Result<ID3D11ShaderResourceView, String> {
    let mut view: Option<ID3D11ShaderResourceView> = None;
    unsafe {
        dev.CreateShaderResourceView(
            tex,
            Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: mip,
                        MipLevels: count,
                    },
                },
            } as *const _),
            Some(&mut view as *mut _),
        )
    }
    .map_err(|e| format!("[nex][glass] region srv failed: {e:?}"))?;
    view.ok_or_else(|| "region srv came back empty".to_string())
}

pub struct GlassPs {
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps_crop: ID3D11PixelShader,
    ps_down: ID3D11PixelShader,
    ps_glass: ID3D11PixelShader,
    cb: ID3D11Buffer,
    smp: ID3D11SamplerState,
    smp_ani: ID3D11SamplerState,
    /// The frost pyramid, rebuilt when the crop's size changes. In a cell
    /// because the engine draws from a `&self`.
    region: std::cell::RefCell<Option<Region>>,
    /// What the last frame was drawn for, so a steady frame can say so.
    crop_key: std::cell::Cell<Option<(u32, u32, u32, u32, u32, u32)>>,
}

impl GlassPs {
    pub fn new(dev: &ID3D11Device, ctx: &ID3D11DeviceContext) -> Result<GlassPs, String> {
        for (name, bytes) in [
            ("PSGlass", embedded::PS_GLASS),
            ("PSCrop", embedded::PS_CROP),
            ("PSDown", embedded::PS_DOWN),
            ("VSMain", embedded::VS_MAIN),
        ] {
            if bytes.is_empty() {
                return Err(format!(
                    "[nex][glass] {name} was not compiled at build time; the shader path is off"
                ));
            }
        }
        let vs = unsafe { shader_vs(dev, embedded::VS_MAIN)? };
        let ps_crop = unsafe { shader_ps(dev, embedded::PS_CROP, "PSCrop")? };
        let ps_down = unsafe { shader_ps(dev, embedded::PS_DOWN, "PSDown")? };
        let ps_glass = unsafe { shader_ps(dev, embedded::PS_GLASS, "PSGlass")? };
        let mut cb: Option<ID3D11Buffer> = None;
        unsafe {
            dev.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<Cb>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                    StructureByteStride: 0,
                },
                None,
                Some(&mut cb as *mut _),
            )
        }
        .map_err(|e| format!("[nex][glass] constant buffer failed: {e:?}"))?;
        let sampler = |filter: D3D11_FILTER, aniso: u32| D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxAnisotropy: aniso,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let (mut smp, mut smp_ani): (Option<ID3D11SamplerState>, Option<ID3D11SamplerState>) =
            (None, None);
        unsafe {
            dev.CreateSamplerState(
                &sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR, 1),
                Some(&mut smp as *mut _),
            )
            .map_err(|e| format!("[nex][glass] sampler failed: {e:?}"))?;
            dev.CreateSamplerState(
                &sampler(D3D11_FILTER_ANISOTROPIC, 16),
                Some(&mut smp_ani as *mut _),
            )
            .map_err(|e| format!("[nex][glass] anisotropic sampler failed: {e:?}"))?;
        }
        Ok(GlassPs {
            dev: dev.clone(),
            ctx: ctx.clone(),
            vs,
            ps_crop,
            ps_down,
            ps_glass,
            cb: cb.ok_or("constant buffer came back empty")?,
            smp: smp.ok_or("sampler came back empty")?,
            smp_ani: smp_ani.ok_or("anisotropic sampler came back empty")?,
            region: std::cell::RefCell::new(None),
            crop_key: std::cell::Cell::new(None),
        })
    }

    /// Draw the material into `target` (the composition swap chain's buffer).
    ///
    /// `source` is the panel's crop of the live desktop, already on the GPU at
    /// `source_size` with the panel inset by `pad` px (its blur padding).
    ///
    /// `buffer` is the swap chain's size, which the window keeps at the largest
    /// panel it has shown, and `panel` is the size drawn now; both device px.
    /// The panel sits `margin` px inside the window on every side and the
    /// window's origin is the buffer's origin, so the material is offset from
    /// the buffer's centre by `sub` - the shader's "glass is a shape inside a
    /// larger window" case, which is what lets the shadow fall outside it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        source: &ID3D11Texture2D,
        source_size: (u32, u32),
        buffer: (u32, u32),
        panel: (u32, u32),
        margin: u32,
        pad: u32,
        radius_css: f32,
        scale: f32,
        look: &Look,
        target: &ID3D11RenderTargetView,
    ) -> Result<(), String> {
        let (rw, rh) = source_size;
        if rw == 0 || rh == 0 {
            return Err("[nex][glass] empty backdrop".into());
        }
        let mut slot = self.region.borrow_mut();
        let need_new = match slot.as_ref() {
            Some(r) => r.w != rw || r.h != rh,
            None => true,
        };
        if need_new {
            *slot = Some(unsafe { Region::new(&self.dev, rw, rh)? });
        }
        let region = slot.as_ref().ok_or("no region")?;

        unsafe {
            // The panel's crop arrives in `source` already, one-to-one.
            let ctx = &self.ctx;
            ctx.CopySubresourceRegion(&region.tex, 0, 0, 0, 0, source, 0, None);
            // Frost pyramid.
            let mut w = rw;
            let mut h = rh;
            for level in 1..region.levels {
                let cb = Cb {
                    pass: [
                        1.0 / w as f32,
                        1.0 / h as f32,
                        (w / 2).max(1) as f32,
                        (h / 2).max(1) as f32,
                    ],
                    ..Cb::default()
                };
                self.pass(
                    &self.ps_down,
                    &region.rtv[level as usize],
                    (w / 2).max(1),
                    (h / 2).max(1),
                    &region.srv[(level - 1) as usize],
                    &cb,
                );
                w = (w / 2).max(1);
                h = (h / 2).max(1);
            }

            // The buffer still holds the previous frame outside the panel, and
            // the material's own area is the whole buffer now: start clear, so
            // the shadow's margin is not painted over an old picture.
            ctx.ClearRenderTargetView(target, &[0.0, 0.0, 0.0, 0.0]);
            // `glass_cb` reads the region's size, and this borrow is live: pass
            // the size rather than start a second borrow inside it.
            let cb =
                self.glass_cb(buffer, panel, margin, pad, radius_css, scale, look, (rw, rh));
            self.pass(
                &self.ps_glass,
                target,
                buffer.0,
                buffer.1,
                &region.full,
                &cb,
            );
            self.crop_key.set(Some((rw, rh, buffer.0, buffer.1, panel.0, panel.1)));
        }
        Ok(())
    }

    fn glass_cb(
        &self,
        buffer: (u32, u32),
        panel: (u32, u32),
        margin: u32,
        pad: u32,
        radius_css: f32,
        scale: f32,
        look: &Look,
        region_size: (u32, u32),
    ) -> Cb {
        let (rw, rh) = region_size;
        let (half_w, half_h) = (panel.0 as f32 / (2.0 * scale), panel.1 as f32 / (2.0 * scale));
        Cb {
            win: [buffer.0 as f32, buffer.1 as f32, rw as f32, rh as f32],
            reg: [0.0, 0.0, rw as f32, rh as f32],
            // half sizes never quite zero: an empty shape covers no pixel
            shape: [half_w.max(1e-3), half_h.max(1e-3), radius_css, scale],
            a: [look.depth, look.bezel, look.max_bezel, look.zoom],
            b: [look.dispersion, look.blur, look.saturate, look.lum],
            c: [look.contrast, look.rim, look.rim_width, look.shade],
            d: [look.light, look.shadow[0], look.shadow[1], look.shadow[2]],
            tint: look.tint,
            pass: [0.0; 4],
            // x: NaN check (off), y: debug (off), z: pad that maps buffer px to
            // region px, w: smooth sampling
            ext: [0.0, 0.0, pad as f32 - margin as f32, 0.0],
            // Glass centre minus buffer centre: the panel's inset plus half its
            // own size, against half the buffer.
            sub: [
                margin as f32 + half_w * scale - buffer.0 as f32 * 0.5,
                margin as f32 + half_h * scale - buffer.1 as f32 * 0.5,
                0.0,
                0.0,
            ],
            // x: 0 = cut the corners here (the window itself does not round),
            // y: the share of the frost left at the edge
            opt: [0.0, look.rim_frost, 0.0, 0.0],
            ..Cb::default()
        }
    }

    unsafe fn pass(
        &self,
        ps: &ID3D11PixelShader,
        rtv: &ID3D11RenderTargetView,
        w: u32,
        h: u32,
        srv: &ID3D11ShaderResourceView,
        cb: &Cb,
    ) {
        let ctx = &self.ctx;
        unsafe {
            // Never bound for reading and writing at once.
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.UpdateSubresource(
                &self.cb,
                0,
                None,
                cb as *const Cb as *const c_void,
                0,
                0,
            );
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: w as f32,
                Height: h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone()), Some(self.smp_ani.clone())]));
            ctx.Draw(3, 0);
        }
    }

    /// Leave nothing bound, so a resize or a release of the swap chain is not
    /// held up by this context.
    pub fn unbind(&self) {
        unsafe {
            self.ctx.PSSetShaderResources(0, Some(&[None]));
            self.ctx.OMSetRenderTargets(None, None);
        }
    }
}

unsafe fn shader_vs(dev: &ID3D11Device, bytes: &[u8]) -> Result<ID3D11VertexShader, String> {
    let mut vs: Option<ID3D11VertexShader> = None;
    unsafe { dev.CreateVertexShader(bytes, None, Some(&mut vs as *mut _)) }
        .map_err(|e| format!("[nex][glass] VSMain failed: {e:?}"))?;
    vs.ok_or_else(|| "[nex][glass] VSMain came back empty".to_string())
}

unsafe fn shader_ps(
    dev: &ID3D11Device,
    bytes: &[u8],
    entry: &str,
) -> Result<ID3D11PixelShader, String> {
    let mut ps: Option<ID3D11PixelShader> = None;
    unsafe { dev.CreatePixelShader(bytes, None, Some(&mut ps as *mut _)) }
        .map_err(|e| format!("[nex][glass] {entry} failed: {e:?}"))?;
    ps.ok_or_else(|| format!("[nex][glass] {entry} came back empty"))
}

#[cfg(test)]
mod tests {
    use super::{Look, ACRYLIC_BLUR};

    /// Acrylic is the lens with its optics taken out. If any of these come back
    /// non-zero the material bends or splits colour again, which is glass, not
    /// acrylic (CIQ-294).
    #[test]
    fn acrylic_has_no_lens_optics() {
        let look = Look::acrylic([0.0, 0.0, 0.0, 0.2]);
        assert_eq!(look.depth, 0.0, "no sideways ray carry");
        assert_eq!(look.dispersion, 0.0, "no colour split");
        assert_eq!(look.zoom, 1.0, "no middle zoom");
        assert_eq!(look.shade, 0.0, "no inner dark line");
        // The lens rim is lit from one corner and fades around the outline. On
        // a flat sheet that is a border missing in places, so it is switched
        // off entirely and the page draws an even outline instead.
        assert_eq!(look.rim, 0.0, "no directional rim light");
        // 1.0 = frost is the same at the edge as in the middle. Anything less
        // leaves a ring of unblurred wallpaper around the panel.
        assert_eq!(look.rim_frost, 1.0, "uniform frost to the edge");
    }

    /// Pins the glass look. These are the values `Look::clear` has carried all
    /// along; the acrylic work was explicitly not allowed to move any of them,
    /// so this test is the guard: change a number here and glass changed too.
    #[test]
    fn lens_look_is_pinned() {
        let look = Look::clear([0.1, 0.2, 0.3, 0.4]);
        assert_eq!(look.depth, 2.4);
        assert_eq!(look.bezel, 0.421);
        assert_eq!(look.max_bezel, 44.0);
        assert_eq!(look.zoom, 1.03);
        assert_eq!(look.dispersion, 0.05);
        assert_eq!(look.blur, 2.0);
        assert_eq!(look.saturate, 1.32);
        assert_eq!(look.lum, 1.0);
        assert_eq!(look.contrast, 1.0);
        assert_eq!(look.rim, 0.25);
        assert_eq!(look.rim_width, 0.35);
        assert_eq!(look.shade, 0.03);
        assert_eq!(look.light, -135.0);
        assert_eq!(look.rim_frost, 0.12);
        assert_eq!(look.shadow, [0.16, 2.0, 30.0]);
        assert_eq!(look.tint, [0.1, 0.2, 0.3, 0.4]);
    }

    /// The look asked for is a wide, even frost — much wider than the lens
    /// ever blurs — and it must stay inside what the frost pyramid can express.
    #[test]
    fn acrylic_frost_is_wider_than_the_lens_and_within_range() {
        let lens = Look::clear([0.0, 0.0, 0.0, 0.0]);
        let acrylic = Look::acrylic([0.0, 0.0, 0.0, 0.0]);
        assert!(acrylic.blur > lens.blur * 4.0);
        assert!(acrylic.blur >= 8.0 && acrylic.blur <= 32.0);
        assert_eq!(acrylic.blur, ACRYLIC_BLUR);
    }
}
