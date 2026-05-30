//! Skia-based (tiny-skia) rendering for the overlay panel.
//! Provides a pure-Rust alternative to GDI+ for rounded rects, fills,
//! Gaussian blur, and compositing — all with per-pixel alpha.

use tiny_skia::*;
use windows_sys::Win32::Graphics::Gdi::{
    StretchDIBits, BITMAPV4HEADER, HDC,
};
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, DIB_RGB_COLORS, SRCCOPY};

pub(crate) const PANEL_RADIUS: f32 = 12.0;

pub(crate) struct SkiaRenderer {
    pub(crate) pixmap: Pixmap,
}

impl SkiaRenderer {
    pub(crate) fn new(width: u32, height: u32) -> Option<Self> {
        let pixmap = Pixmap::new(width, height)?;
        Some(Self { pixmap })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) -> bool {
        Pixmap::new(width, height).map(|p| self.pixmap = p).is_some()
    }

    pub(crate) fn clear(&mut self) {
        self.pixmap.fill(Color::TRANSPARENT);
    }

    pub(crate) fn fill_rounded_rect(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        radius: f32,
        color: Color,
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = radius.max(0.0).min(w.min(h) / 2.0);
        if r <= 0.0 {
            self.fill_rect(x, y, w, h, color);
            return;
        }

        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;

        let path = rounded_rect_path(x, y, w, h, r);

        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    pub(crate) fn fill_rect(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        color: Color,
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let rect = Rect::from_xywh(x, y, w, h).unwrap();
        let mut path = PathBuilder::new();
        path.push_rect(rect);
        let path = path.finish().unwrap();

        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    pub(crate) fn draw_line(
        &mut self,
        x1: f32, y1: f32, x2: f32, y2: f32,
        color: Color,
        width: f32,
    ) {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = width;
        stroke.line_cap = LineCap::Round;
        let mut pb = PathBuilder::new();
        pb.move_to(x1, y1);
        pb.line_to(x2, y2);
        let path = pb.finish().unwrap();
        self.pixmap.stroke_path(
            &path,
            &paint,
            &stroke,
            Transform::identity(),
            None,
        );
    }

    /// Apply a simple box blur to the pixmap.
    /// This is a poor man's Gaussian blur approximation.
    pub(crate) fn apply_box_blur(&mut self, radius: u32) {
        if radius == 0 {
            return;
        }
        let w = self.pixmap.width() as usize;
        let h = self.pixmap.height() as usize;
        let r = radius as usize;
        let src = self.pixmap.data().to_vec();
        let mut dst = vec![0u8; src.len()];

        // Horizontal pass
        for y in 0..h {
            for x in 0..w {
                let mut ra = 0u32;
                let mut ga = 0u32;
                let mut ba = 0u32;
                let mut aa = 0u32;
                let mut count = 0u32;
                let x0 = if x > r { x - r } else { 0 };
                let x1 = (x + r).min(w - 1);
                for kx in x0..=x1 {
                    let idx = (y * w + kx) * 4;
                    ra += src[idx] as u32;
                    ga += src[idx + 1] as u32;
                    ba += src[idx + 2] as u32;
                    aa += src[idx + 3] as u32;
                    count += 1;
                }
                let idx = (y * w + x) * 4;
                dst[idx] = (ra / count) as u8;
                dst[idx + 1] = (ga / count) as u8;
                dst[idx + 2] = (ba / count) as u8;
                dst[idx + 3] = (aa / count) as u8;
            }
        }

        // Vertical pass
        for y in 0..h {
            for x in 0..w {
                let mut ra = 0u32;
                let mut ga = 0u32;
                let mut ba = 0u32;
                let mut aa = 0u32;
                let mut count = 0u32;
                let y0 = if y > r { y - r } else { 0 };
                let y1 = (y + r).min(h - 1);
                for ky in y0..=y1 {
                    let idx = (ky * w + x) * 4;
                    ra += dst[idx] as u32;
                    ga += dst[idx + 1] as u32;
                    ba += dst[idx + 2] as u32;
                    aa += dst[idx + 3] as u32;
                    count += 1;
                }
                let idx = (y * w + x) * 4;
                self.pixmap.data_mut()[idx] = (ra / count) as u8;
                self.pixmap.data_mut()[idx + 1] = (ga / count) as u8;
                self.pixmap.data_mut()[idx + 2] = (ba / count) as u8;
                self.pixmap.data_mut()[idx + 3] = (aa / count) as u8;
            }
        }
    }

    pub(crate) fn pixel_data(&self) -> &[u8] {
        self.pixmap.data()
    }

    pub(crate) fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub(crate) fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// Composite a source pixmap onto this pixmap at (dest_x, dest_y).
    /// Both pixmaps are premultiplied RGBA; uses standard over compositing.
    pub(crate) fn composite_from(&mut self, src: &Pixmap, dest_x: i32, dest_y: i32) {
        let sw = src.width();
        let sh = src.height();
        let dw = self.pixmap.width();
        let dh = self.pixmap.height();
        let src_data = src.data();
        let dst_data = self.pixmap.data();
        if dw == 0 || dh == 0 {
            return;
        }

        let mut converted = Vec::with_capacity((sw * sh) as usize * 4);

        for sy in 0..sh {
            for sx in 0..sw {
                let dx = dest_x + sx as i32;
                let dy = dest_y + sy as i32;
                if dx < 0 || dx >= dw as i32 || dy < 0 || dy >= dh as i32 {
                    converted.extend_from_slice(&[0, 0, 0, 0]);
                    continue;
                }

                let si = (sy * sw + sx) as usize * 4;
                let di = (dy as u32 * dw + dx as u32) as usize * 4;

                let sa = src_data[si + 3] as u32;
                if sa == 0 {
                    converted.extend_from_slice(&dst_data[di..di + 4]);
                    continue;
                }

                let da = dst_data[di + 3] as u32;
                if sa == 255 || da == 0 {
                    converted.extend_from_slice(&src_data[si..si + 4]);
                    continue;
                }

                // Premultiplied over: result = src + dst * (1 - src_a)
                let inv_sa = 255 - sa as u32;
                let out_a = sa + (da * inv_sa / 255);
                let out_r = src_data[si] as u32 + (dst_data[di] as u32 * inv_sa / 255);
                let out_g = src_data[si + 1] as u32 + (dst_data[di + 1] as u32 * inv_sa / 255);
                let out_b = src_data[si + 2] as u32 + (dst_data[di + 2] as u32 * inv_sa / 255);

                converted.push(out_r.min(255) as u8);
                converted.push(out_g.min(255) as u8);
                converted.push(out_b.min(255) as u8);
                converted.push(out_a.min(255) as u8);
            }
        }

        // Write converted pixels back
        let dst_data_mut = self.pixmap.data_mut();
        for sy in 0..sh {
            for sx in 0..sw {
                let dx = dest_x + sx as i32;
                let dy = dest_y + sy as i32;
                if dx < 0 || dx >= dw as i32 || dy < 0 || dy >= dh as i32 {
                    continue;
                }
                let ci = (sy * sw + sx) as usize * 4;
                let di = (dy as u32 * dw + dx as u32) as usize * 4;
                dst_data_mut[di..di + 4].copy_from_slice(&converted[ci..ci + 4]);
            }
        }
    }

    /// Render a glow line: creates a temp pixmap, draws a filled rect,
    /// box-blurs it, and composites onto the main pixmap.
    pub(crate) fn render_glow_line(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        color: Color,
        blur_radius: u32,
    ) {
        if w <= 0.0 || h <= 0.0 || blur_radius == 0 {
            return;
        }
        let extra = blur_radius * 2;
        let gw = w as u32 + extra;
        let gh = h as u32 + extra;
        let Some(mut glow) = Pixmap::new(gw, gh) else {
            return;
        };
        glow.fill(Color::TRANSPARENT);

        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let rect = Rect::from_xywh(blur_radius as f32, blur_radius as f32, w, h).unwrap();
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        if let Some(path) = pb.finish() {
            glow.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }

        // Apply box blur to the glow pixmap
        if blur_radius > 0 {
            let r = blur_radius as usize;
            let glow_w = gw as usize;
            let glow_h = gh as usize;
            let src = glow.data().to_vec();
            let mut dst = vec![0u8; src.len()];

            // Horizontal
            for gy in 0..glow_h {
                for gx in 0..glow_w {
                    let mut ra = 0u32; let mut ga = 0u32;
                    let mut ba = 0u32; let mut aa = 0u32;
                    let mut count = 0u32;
                    let x0 = if gx > r { gx - r } else { 0 };
                    let x1 = (gx + r).min(glow_w - 1);
                    for kx in x0..=x1 {
                        let idx = (gy * glow_w + kx) * 4;
                        ra += src[idx] as u32;
                        ga += src[idx + 1] as u32;
                        ba += src[idx + 2] as u32;
                        aa += src[idx + 3] as u32;
                        count += 1;
                    }
                    let idx = (gy * glow_w + gx) * 4;
                    dst[idx] = (ra / count) as u8;
                    dst[idx + 1] = (ga / count) as u8;
                    dst[idx + 2] = (ba / count) as u8;
                    dst[idx + 3] = (aa / count) as u8;
                }
            }

            // Vertical
            for gy in 0..glow_h {
                for gx in 0..glow_w {
                    let mut ra = 0u32; let mut ga = 0u32;
                    let mut ba = 0u32; let mut aa = 0u32;
                    let mut count = 0u32;
                    let y0 = if gy > r { gy - r } else { 0 };
                    let y1 = (gy + r).min(glow_h - 1);
                    for ky in y0..=y1 {
                        let idx = (ky * glow_w + gx) * 4;
                        ra += dst[idx] as u32;
                        ga += dst[idx + 1] as u32;
                        ba += dst[idx + 2] as u32;
                        aa += dst[idx + 3] as u32;
                        count += 1;
                    }
                    let idx = (gy * glow_w + gx) * 4;
                    glow.data_mut()[idx] = (ra / count) as u8;
                    glow.data_mut()[idx + 1] = (ga / count) as u8;
                    glow.data_mut()[idx + 2] = (ba / count) as u8;
                    glow.data_mut()[idx + 3] = (aa / count) as u8;
                }
            }
        }

        self.composite_from(&glow, x as i32 - blur_radius as i32, y as i32 - blur_radius as i32);
    }

    /// Blit the pixmap onto a GDI HDC at (x, y) with (w, h) dimensions.
    /// Converts premultiplied RGBA (tiny-skia format) to BGRA (GDI format).
    pub(crate) fn blit_to_hdc(&self, hdc: HDC, x: i32, y: i32, w: i32, h: i32) {
        let pw = self.pixmap.width() as i32;
        let ph = self.pixmap.height() as i32;
        let bw = w.min(pw);
        let bh = h.min(ph);
        if bw <= 0 || bh <= 0 {
            return;
        }

        let src = self.pixmap.data();
        let row = bw as usize * 4;
        let mut bgra = vec![0u8; row * bh as usize];
        for row_y in 0..bh as usize {
            for col in 0..bw as usize {
                let si = row_y * pw as usize * 4 + col * 4;
                let di = row_y * bw as usize * 4 + col * 4;
                bgra[di] = src[si + 2];     // B
                bgra[di + 1] = src[si + 1]; // G
                bgra[di + 2] = src[si];     // R
                bgra[di + 3] = src[si + 3]; // A
            }
        }

        let header = BITMAPV4HEADER {
            bV4Size: std::mem::size_of::<BITMAPV4HEADER>() as u32,
            bV4Width: bw,
            bV4Height: -bh, // top-down
            bV4Planes: 1,
            bV4BitCount: 32,
            bV4V4Compression: BI_RGB,
            bV4SizeImage: (bw * bh * 4) as u32,
            bV4XPelsPerMeter: 0,
            bV4YPelsPerMeter: 0,
            bV4ClrUsed: 0,
            bV4ClrImportant: 0,
            bV4RedMask: 0,
            bV4GreenMask: 0,
            bV4BlueMask: 0,
            bV4AlphaMask: 0,
            bV4CSType: 0,
            bV4Endpoints: unsafe { std::mem::zeroed() },
            bV4GammaRed: 0,
            bV4GammaGreen: 0,
            bV4GammaBlue: 0,
        };

        unsafe {
            StretchDIBits(
                hdc,
                x, y, bw, bh,
                0, 0, bw, bh,
                bgra.as_ptr() as *const _,
                &header as *const _ as *const _,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }
}

/// Convert a GDI 32-bit color (0x00BBGGRR) to a tiny-skia Color (sRGBA, A=255).
pub(crate) fn gdi_color_to_skia_color(gdi_color: u32) -> Color {
    let r = gdi_color & 0xFF;
    let g = (gdi_color >> 8) & 0xFF;
    let b = (gdi_color >> 16) & 0xFF;
    Color::from_rgba8(r as u8, g as u8, b as u8, 255)
}

/// Convert a GDI 32-bit color to a tiny-skia Color with custom alpha (0-255).
pub(crate) fn gdi_color_to_skia_color_alpha(gdi_color: u32, alpha: u8) -> Color {
    let r = gdi_color & 0xFF;
    let g = (gdi_color >> 8) & 0xFF;
    let b = (gdi_color >> 16) & 0xFF;
    Color::from_rgba8(r as u8, g as u8, b as u8, alpha)
}

/// Build a rounded rectangle path using cubic bezier approximations for the corners.
fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let kappa = 0.5522847498; // cubic approx of 90° arc
    let k = r * kappa;
    let mut pb = PathBuilder::with_capacity(8, 12);

    // Top edge
    pb.move_to(x + r, y);
    // Top-right arc
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    // Right edge
    pb.line_to(x + w, y + h - r);
    // Bottom-right arc
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    // Bottom edge
    pb.line_to(x + r, y + h);
    // Bottom-left arc
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    // Left edge
    pb.line_to(x, y + r);
    // Top-left arc
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);

    pb.close();
    pb.finish().unwrap()
}
