//! GDI+ antialiased rendering for selection highlights, text, icons, and panel background.
//! Uses direct FFI to gdiplus.dll (available on all Windows versions).

#[allow(non_snake_case)]
#[repr(C)]
struct GdiplusStartupInput {
    GdiplusVersion: u32,
    DebugEventCallback: *mut std::ffi::c_void,
    SuppressBackgroundThread: i32,
    SuppressExternalCodecs: i32,
}

#[repr(C)]
pub(crate) struct GpRectF {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[link(name = "gdiplus")]
extern "system" {
    fn GdiplusStartup(
        token: *mut usize,
        input: *const GdiplusStartupInput,
        output: *mut *mut std::ffi::c_void,
    ) -> i32;

    fn GdiplusShutdown(token: usize);

    fn GdipCreateFromHDC(hdc: isize, graphics: *mut isize) -> i32;
    fn GdipDeleteGraphics(graphics: isize) -> i32;
    fn GdipSetSmoothingMode(graphics: isize, smoothingMode: i32) -> i32;

    // Brushes
    fn GdipCreateSolidFill(color: u32, brush: *mut isize) -> i32;
    fn GdipDeleteBrush(brush: isize) -> i32;

    // Paths
    fn GdipCreatePath(fillMode: i32, path: *mut isize) -> i32;
    fn GdipAddPathLineI(path: isize, x1: i32, y1: i32, x2: i32, y2: i32) -> i32;
    fn GdipAddPathArcI(path: isize, x: i32, y: i32, width: i32, height: i32, startAngle: f32, sweepAngle: f32) -> i32;
    fn GdipFillPath(graphics: isize, brush: isize, path: isize) -> i32;
    fn GdipDeletePath(path: isize) -> i32;

    // Rect fill
    fn GdipFillRectangleI(graphics: isize, brush: isize, x: i32, y: i32, width: i32, height: i32) -> i32;

    // Font
    fn GdipCreateFontFromDC(hdc: isize, font: *mut isize) -> i32;
    fn GdipDeleteFont(font: isize) -> i32;
    fn GdipCreateFont(
        familyName: *const u16, emSize: f32, style: i32, unit: i32,
        font: *mut isize,
    ) -> i32;

    // String format
    fn GdipCreateStringFormat(formatAttributes: i32, language: u16, format: *mut isize) -> i32;
    fn GdipSetStringFormatAlign(format: isize, align: i32) -> i32;
    fn GdipSetStringFormatLineAlign(format: isize, align: i32) -> i32;
    fn GdipSetStringFormatTrimming(format: isize, trimming: i32) -> i32;
    fn GdipDeleteStringFormat(format: isize) -> i32;

    // Text
    fn GdipDrawString(
        graphics: isize, string: *const u16, length: i32,
        font: isize, layoutRect: *const GpRectF,
        stringFormat: isize, brush: isize,
    ) -> i32;
    fn GdipMeasureString(
        graphics: isize, string: *const u16, length: i32,
        font: isize, layoutRect: *const GpRectF,
        stringFormat: isize,
        boundingBox: *mut GpRectF, codepointsFitted: *mut i32, linesFilled: *mut i32,
    ) -> i32;

    // Icons
    fn GdipCreateBitmapFromHICON(hicon: isize, bitmap: *mut isize) -> i32;
    fn GdipDrawImageI(graphics: isize, image: isize, x: i32, y: i32) -> i32;
    fn GdipDisposeImage(image: isize) -> i32;

    // Pens and lines
    fn GdipCreatePen1(color: u32, width: f32, unit: i32, pen: *mut isize) -> i32;
    fn GdipDeletePen(pen: isize) -> i32;
    fn GdipDrawLineI(graphics: isize, pen: isize, x1: i32, y1: i32, x2: i32, y2: i32) -> i32;
}

const GDI_PLUS_OK: i32 = 0;
pub(crate) const SMOOTHING_MODE_ANTI_ALIAS: i32 = 4;
pub(crate) const SMOOTHING_MODE_HIGH_QUALITY: i32 = 4;
const FILL_MODE_ALTERNATE: i32 = 0;
const UNIT_PIXEL: i32 = 2;
const FONT_STYLE_REGULAR: i32 = 0;

// StringAlignment
const STRING_ALIGNMENT_NEAR: i32 = 0;
const STRING_ALIGNMENT_CENTER: i32 = 1;
const STRING_ALIGNMENT_FAR: i32 = 2;

// StringTrimming
const STRING_TRIMMING_ELLIPSIS_CHARACTER: i32 = 3;

// StringFormatFlags
const STRING_FORMAT_FLAGS_NO_WRAP: i32 = 0x1000;
const STRING_FORMAT_FLAGS_LINE_LIMIT: i32 = 0x2000;

pub(crate) struct GdiplusContext {
    token: usize,
    pub(crate) default_str_fmt: isize,
}

impl GdiplusContext {
    pub(crate) fn new() -> Option<Self> {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: std::ptr::null_mut(),
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };
        let mut token = 0usize;
        let result = unsafe {
            GdiplusStartup(&mut token, &input, std::ptr::null_mut())
        };
        if result != GDI_PLUS_OK {
            return None;
        }

        let str_fmt = unsafe {
            let mut fmt = 0isize;
            if GdipCreateStringFormat(
                STRING_FORMAT_FLAGS_NO_WRAP | STRING_FORMAT_FLAGS_LINE_LIMIT,
                0, // neutral language
                &mut fmt,
            ) == GDI_PLUS_OK
            {
                GdipSetStringFormatAlign(fmt, STRING_ALIGNMENT_NEAR);
                GdipSetStringFormatLineAlign(fmt, STRING_ALIGNMENT_CENTER);
                GdipSetStringFormatTrimming(fmt, STRING_TRIMMING_ELLIPSIS_CHARACTER);
                fmt
            } else {
                0
            }
        };

        Some(Self { token, default_str_fmt: str_fmt })
    }

    // --- Graphics lifecycle ---

    pub(crate) fn create_graphics(&self, hdc: isize) -> Option<isize> {
        let mut graphics = 0isize;
        if unsafe { GdipCreateFromHDC(hdc, &mut graphics) } == GDI_PLUS_OK {
            Some(graphics)
        } else {
            None
        }
    }

    pub(crate) fn delete_graphics(graphics: isize) {
        unsafe { GdipDeleteGraphics(graphics); }
    }

    pub(crate) fn set_smoothing_mode(graphics: isize, mode: i32) {
        unsafe { GdipSetSmoothingMode(graphics, mode); }
    }

    // --- Brushes ---

    pub(crate) fn create_solid_brush(color: u32) -> Option<isize> {
        let mut brush = 0isize;
        if unsafe { GdipCreateSolidFill(color, &mut brush) } == GDI_PLUS_OK {
            Some(brush)
        } else {
            None
        }
    }

    pub(crate) fn delete_brush(brush: isize) {
        unsafe { GdipDeleteBrush(brush); }
    }

    // --- Rect fill ---

    pub(crate) fn fill_rect(&self, graphics: isize, x: i32, y: i32, w: i32, h: i32, color: u32) {
        if w <= 0 || h <= 0 { return; }
        if let Some(brush) = Self::create_solid_brush(color) {
            unsafe { GdipFillRectangleI(graphics, brush, x, y, w, h); }
            Self::delete_brush(brush);
        }
    }

    // --- Rounded rect (with own graphics create/delete) ---

    pub(crate) fn fill_rounded_rect(
        &self, hdc: isize,
        x: i32, y: i32, w: i32, h: i32,
        radius: i32, color: u32,
    ) {
        if w <= 0 || h <= 0 { return; }
        let r = radius.max(0).min(w.min(h) / 2);
        let d = r * 2;

        let saved = unsafe {
            windows_sys::Win32::Graphics::Gdi::SaveDC(hdc as _)
        };
        if saved == 0 { return; }

        let mut graphics = 0isize;
        if unsafe { GdipCreateFromHDC(hdc, &mut graphics) } != GDI_PLUS_OK {
            unsafe { windows_sys::Win32::Graphics::Gdi::RestoreDC(hdc as _, saved); }
            return;
        }
        unsafe { GdipSetSmoothingMode(graphics, SMOOTHING_MODE_ANTI_ALIAS); }

        let mut brush = 0isize;
        if unsafe { GdipCreateSolidFill(color, &mut brush) } != GDI_PLUS_OK {
            unsafe { GdipDeleteGraphics(graphics); }
            unsafe { windows_sys::Win32::Graphics::Gdi::RestoreDC(hdc as _, saved); }
            return;
        }

        if r == 0 {
            unsafe {
                GdipFillRectangleI(graphics, brush, x, y, w, h);
                GdipDeleteBrush(brush);
                GdipDeleteGraphics(graphics);
                windows_sys::Win32::Graphics::Gdi::RestoreDC(hdc as _, saved);
            }
            return;
        }

        let mut path = 0isize;
        if unsafe { GdipCreatePath(FILL_MODE_ALTERNATE, &mut path) } != GDI_PLUS_OK {
            unsafe { GdipDeleteBrush(brush); GdipDeleteGraphics(graphics); }
            unsafe { windows_sys::Win32::Graphics::Gdi::RestoreDC(hdc as _, saved); }
            return;
        }

        unsafe {
            GdipAddPathArcI(path, x, y, d, d, 180.0, 90.0);
            GdipAddPathLineI(path, x + r, y, x + w - r, y);
            GdipAddPathArcI(path, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathLineI(path, x + w, y + r, x + w, y + h - r);
            GdipAddPathArcI(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathLineI(path, x + w - r, y + h, x + r, y + h);
            GdipAddPathArcI(path, x, y + h - d, d, d, 90.0, 90.0);
            GdipAddPathLineI(path, x, y + h - r, x, y + r);
        }

        unsafe {
            GdipFillPath(graphics, brush, path);
        }

        unsafe {
            GdipDeletePath(path);
            GdipDeleteBrush(brush);
            GdipDeleteGraphics(graphics);
            windows_sys::Win32::Graphics::Gdi::RestoreDC(hdc as _, saved);
        }
    }

    // --- Rounded rect on existing graphics ---

    pub(crate) fn fill_rounded_rect_on_graphics(
        &self, graphics: isize, x: i32, y: i32, w: i32, h: i32,
        radius: i32, color: u32,
    ) {
        if w <= 0 || h <= 0 { return; }
        let r = radius.max(0).min(w.min(h) / 2);
        let d = r * 2;

        let Some(brush) = Self::create_solid_brush(color) else { return; };

        if r == 0 {
            unsafe { GdipFillRectangleI(graphics, brush, x, y, w, h); }
            Self::delete_brush(brush);
            return;
        }

        let mut path = 0isize;
        if unsafe { GdipCreatePath(FILL_MODE_ALTERNATE, &mut path) } != GDI_PLUS_OK {
            Self::delete_brush(brush);
            return;
        }

        unsafe {
            GdipAddPathArcI(path, x, y, d, d, 180.0, 90.0);
            GdipAddPathLineI(path, x + r, y, x + w - r, y);
            GdipAddPathArcI(path, x + w - d, y, d, d, 270.0, 90.0);
            GdipAddPathLineI(path, x + w, y + r, x + w, y + h - r);
            GdipAddPathArcI(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
            GdipAddPathLineI(path, x + w - r, y + h, x + r, y + h);
            GdipAddPathArcI(path, x, y + h - d, d, d, 90.0, 90.0);
            GdipAddPathLineI(path, x, y + h - r, x, y + r);
        }

        unsafe { GdipFillPath(graphics, brush, path); }

        unsafe { GdipDeletePath(path); }
        Self::delete_brush(brush);
    }

    // --- Font ---

    pub(crate) fn create_font_from_hdc(hdc: isize) -> Option<isize> {
        let mut font = 0isize;
        if unsafe { GdipCreateFontFromDC(hdc, &mut font) } == GDI_PLUS_OK {
            Some(font)
        } else {
            None
        }
    }

    pub(crate) fn delete_font(font: isize) {
        unsafe { GdipDeleteFont(font); }
    }

    // --- Text ---

    pub(crate) fn draw_string(
        &self, graphics: isize, text: &[u16],
        font: isize, rect: &GpRectF, color: u32,
    ) {
        if text.is_empty() || self.default_str_fmt == 0 { return; }
        if let Some(brush) = Self::create_solid_brush(color) {
            unsafe {
                GdipDrawString(
                    graphics, text.as_ptr(), text.len() as i32,
                    font, rect as *const GpRectF,
                    self.default_str_fmt, brush,
                );
            }
            Self::delete_brush(brush);
        }
    }

    // --- Measure text ---

    pub(crate) fn measure_string(
        &self, graphics: isize, text: &[u16],
        font: isize, rect: &GpRectF,
    ) -> Option<GpRectF> {
        if text.is_empty() || self.default_str_fmt == 0 { return None; }
        let mut bounds = GpRectF { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
        let result = unsafe {
            GdipMeasureString(
                graphics, text.as_ptr(), text.len() as i32,
                font, rect as *const GpRectF,
                self.default_str_fmt,
                &mut bounds, std::ptr::null_mut(), std::ptr::null_mut(),
            )
        };
        if result == GDI_PLUS_OK { Some(bounds) } else { None }
    }

    // --- Icons ---

    pub(crate) fn draw_icon(graphics: isize, hicon: isize, x: i32, y: i32, size: i32) {
        let mut bitmap = 0isize;
        if unsafe { GdipCreateBitmapFromHICON(hicon, &mut bitmap) } != GDI_PLUS_OK {
            return;
        }
        unsafe {
            GdipDrawImageI(graphics, bitmap, x, y);
            GdipDisposeImage(bitmap);
        }
    }

    // --- Pen and line ---

    pub(crate) fn create_pen(color: u32, width: f32) -> Option<isize> {
        let mut pen = 0isize;
        if unsafe { GdipCreatePen1(color, width, UNIT_PIXEL, &mut pen) } == GDI_PLUS_OK {
            Some(pen)
        } else {
            None
        }
    }

    pub(crate) fn delete_pen(pen: isize) {
        unsafe { GdipDeletePen(pen); }
    }

    pub(crate) fn draw_line(graphics: isize, x1: i32, y1: i32, x2: i32, y2: i32, color: u32, width: f32) {
        if let Some(pen) = Self::create_pen(color, width) {
            unsafe { GdipDrawLineI(graphics, pen, x1, y1, x2, y2); }
            Self::delete_pen(pen);
        }
    }

    // --- Color conversion ---

    pub(crate) fn gdi_color_to_argb(color: u32) -> u32 {
        0xFF000000
            | ((color & 0x0000FF) << 16)
            | (color & 0x00FF00)
            | ((color & 0xFF0000) >> 16)
    }
}

impl Drop for GdiplusContext {
    fn drop(&mut self) {
        if self.default_str_fmt != 0 {
            unsafe { GdipDeleteStringFormat(self.default_str_fmt); }
        }
        unsafe { GdiplusShutdown(self.token); }
    }
}
