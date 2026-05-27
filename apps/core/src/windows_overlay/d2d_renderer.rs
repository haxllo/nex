use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_LAYER_OPTIONS_NONE, D2D1_LAYER_PARAMETERS, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_HARDWARE, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1Bitmap, ID2D1Brush, ID2D1DCRenderTarget, ID2D1Factory,
    ID2D1Geometry, ID2D1HwndRenderTarget, ID2D1Layer, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    HGDIOBJ,
};
use windows::Win32::Graphics::Gdi::HDC as WindowsHDC;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetIconInfo, HICON, ICONINFO, DI_NORMAL,
};
use windows::core::HSTRING;

use crate::windows_overlay::types::{
    ICON_FONT_FAMILY_FALLBACK, ICON_FONT_FAMILY_PRIMARY, PRIMARY_FONT_FAMILY,
};

#[derive(Clone, Copy)]
pub(crate) enum FontRole {
    Input,
    Title,
    Meta,
    Status,
    Header,
    TopHit,
    Hint,
    HelpTip,
    HelpIcon,
    Footer,
    CommandPrefix,
    CommandBadge,
    CommandIcon,
    CommandIconFallback,
}

fn font_role_size(role: &FontRole) -> f32 {
    match role {
        FontRole::Input => 19.0,
        FontRole::Title => 15.0,
        FontRole::Meta => 13.0,
        FontRole::Status => 11.0,
        FontRole::Header => 12.0,
        FontRole::TopHit => 16.0,
        FontRole::Hint => 11.0,
        FontRole::HelpTip => 11.0,
        FontRole::HelpIcon => 14.0,
        FontRole::Footer => 13.0,
        FontRole::CommandPrefix => 22.0,
        FontRole::CommandBadge => 24.0,
        FontRole::CommandIcon => 24.0,
        FontRole::CommandIconFallback => 24.0,
    }
}

fn font_role_weight(role: &FontRole) -> DWRITE_FONT_WEIGHT {
    match role {
        FontRole::Input | FontRole::Status | FontRole::Hint | FontRole::HelpTip => {
            DWRITE_FONT_WEIGHT(400)
        }
        FontRole::Title | FontRole::Meta | FontRole::Header | FontRole::Footer => {
            DWRITE_FONT_WEIGHT(600)
        }
        FontRole::TopHit => DWRITE_FONT_WEIGHT(700),
        FontRole::CommandPrefix | FontRole::CommandBadge => DWRITE_FONT_WEIGHT(800),
        FontRole::HelpIcon | FontRole::CommandIcon | FontRole::CommandIconFallback => {
            DWRITE_FONT_WEIGHT(400)
        }
    }
}

pub(crate) struct D2dRenderer {
    _factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    render_target: Option<ID2D1HwndRenderTarget>,
    dc_render_target: Option<ID2D1DCRenderTarget>,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    dc_brushes: HashMap<u32, ID2D1SolidColorBrush>,
    text_formats: Vec<Option<IDWriteTextFormat>>,
    icon_font_formats: [Option<IDWriteTextFormat>; 2],
    bitmap_cache: HashMap<isize, ID2D1Bitmap>,
    #[allow(dead_code)]
    layer: Option<ID2D1Layer>,
}

impl D2dRenderer {
    pub(crate) fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| format!("CoInitializeEx failed: {:?}", e))?;
        }

        let factory: ID2D1Factory = unsafe {
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))
                .map_err(|e| format!("D2D1CreateFactory failed: {:?}", e))?
        };

        let dwrite_factory: IDWriteFactory = unsafe {
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|e| format!("DWriteCreateFactory failed: {:?}", e))?
        };

        let mut renderer = Self {
            _factory: factory,
            dwrite_factory,
            render_target: None,
            dc_render_target: None,
            brushes: HashMap::new(),
            dc_brushes: HashMap::new(),
            text_formats: (0..14).map(|_| None).collect(),
            icon_font_formats: [None, None],
            bitmap_cache: HashMap::new(),
            layer: None,
        };

        renderer.create_render_target(hwnd, width, height)?;
        Ok(renderer)
    }

    fn create_render_target(&mut self, hwnd: HWND, width: u32, height: u32) -> Result<(), String> {
        let render_target_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_HARDWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 0.0,
            dpiY: 0.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };

        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width, height },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };

        let target: ID2D1HwndRenderTarget = unsafe {
            self._factory
                .CreateHwndRenderTarget(&render_target_props, &hwnd_props)
                .map_err(|e| format!("CreateHwndRenderTarget failed: {:?}", e))?
        };

        unsafe {
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }
        self.render_target = Some(target);
        Ok(())
    }

    pub(crate) fn resize(&mut self, hwnd: HWND, width: u32, height: u32) {
        if let Some(ref rt) = self.render_target {
            unsafe {
                let _ = rt.Resize(&D2D_SIZE_U { width, height });
            }
        } else {
            let _ = self.create_render_target(hwnd, width, height);
        }
    }

    pub(crate) fn begin_draw(&mut self) -> bool {
        if let Some(ref rt) = self.render_target {
            unsafe { rt.BeginDraw(); }
            true
        } else {
            false
        }
    }

    pub(crate) fn end_draw(&mut self) {
        if let Some(ref rt) = self.render_target {
            unsafe {
                let _ = rt.EndDraw(None, None);
            }
        }
    }

    pub(crate) fn clear(&mut self, color: u32) {
        let Some(ref rt) = self.render_target else { return };
        let r = ((color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((color >> 8) & 0xFF) as f32 / 255.0;
        let b = (color & 0xFF) as f32 / 255.0;
        let c = D2D1_COLOR_F { r, g, b, a: 1.0 };
        unsafe {
            rt.Clear(Some(&c as *const D2D1_COLOR_F));
        }
    }

    fn make_brush(&self, color: u32) -> Option<ID2D1SolidColorBrush> {
        let rt = self.render_target.as_ref()?;
        let r = ((color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((color >> 8) & 0xFF) as f32 / 255.0;
        let b = (color & 0xFF) as f32 / 255.0;
        let alpha = ((color >> 24) & 0xFF) as f32 / 255.0;
        let c = D2D1_COLOR_F {
            r,
            g,
            b,
            a: if alpha == 0.0 { 1.0 } else { alpha },
        };
        unsafe { rt.CreateSolidColorBrush(&c as *const D2D1_COLOR_F, None).ok() }
    }

    #[allow(dead_code)]
    pub(crate) fn brush(&mut self, color: u32) -> &ID2D1SolidColorBrush {
        if !self.brushes.contains_key(&color) {
            if let Some(brush) = self.make_brush(color) {
                self.brushes.insert(color, brush);
            }
        }
        if !self.brushes.contains_key(&color) {
            panic!("failed to create brush for color {:#08x}", color);
        }
        &self.brushes[&color]
    }

    pub(crate) fn fill_rounded_rectangle(
        &mut self,
        rect: &D2D_RECT_F,
        radius: f32,
        color: u32,
    ) {
        if !self.brushes.contains_key(&color) {
            if let Some(brush) = self.make_brush(color) {
                self.brushes.insert(color, brush);
            }
        }
        let Some(ref rt) = self.render_target else { return };
        if let Some(brush) = self.brushes.get(&color) {
            use windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT;
            let rounded = D2D1_ROUNDED_RECT {
                rect: *rect,
                radiusX: radius,
                radiusY: radius,
            };
            unsafe {
                rt.FillRoundedRectangle(&rounded, brush);
            }
        }
    }

    pub(crate) fn draw_line(
        &mut self,
        x1: f32, y1: f32, x2: f32, y2: f32,
        color: u32, stroke_width: f32,
    ) {
        if !self.brushes.contains_key(&color) {
            if let Some(brush) = self.make_brush(color) {
                self.brushes.insert(color, brush);
            }
        }
        let Some(ref rt) = self.render_target else { return };
        if let Some(brush) = self.brushes.get(&color) {
            use windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F;
            unsafe {
                rt.DrawLine(
                    D2D_POINT_2F { x: x1, y: y1 },
                    D2D_POINT_2F { x: x2, y: y2 },
                    brush,
                    stroke_width,
                    None,
                );
            }
        }
    }

    pub(crate) fn fill_rectangle(&mut self, rect: &D2D_RECT_F, color: u32) {
        if !self.brushes.contains_key(&color) {
            if let Some(brush) = self.make_brush(color) {
                self.brushes.insert(color, brush);
            }
        }
        let Some(ref rt) = self.render_target else { return };
        if let Some(brush) = self.brushes.get(&color) {
            unsafe {
                rt.FillRectangle(rect as *const D2D_RECT_F, brush);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn draw_text(
        &mut self,
        text: &str,
        rect: &D2D_RECT_F,
        color: u32,
        format: &IDWriteTextFormat,
    ) {
        if !self.brushes.contains_key(&color) {
            if let Some(brush) = self.make_brush(color) {
                self.brushes.insert(color, brush);
            }
        }
        let Some(ref rt) = self.render_target else { return };
        let wide: Vec<u16> = text.encode_utf16().collect();
        if let Some(brush) = self.brushes.get(&color) {
            unsafe {
                rt.DrawText(
                    &wide,
                    format,
                    rect as *const D2D_RECT_F,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }

    pub(crate) fn precreate_all_text_formats(&mut self) {
        for role_idx in 0..self.text_formats.len() {
            if self.text_formats[role_idx].is_some() {
                continue;
            }
            let role = match role_idx {
                0 => FontRole::Input,
                1 => FontRole::Title,
                2 => FontRole::Meta,
                3 => FontRole::Status,
                4 => FontRole::Header,
                5 => FontRole::TopHit,
                6 => FontRole::Hint,
                7 => FontRole::HelpTip,
                8 => FontRole::HelpIcon,
                9 => FontRole::Footer,
                10 => FontRole::CommandPrefix,
                11 => FontRole::CommandBadge,
                12 => FontRole::CommandIcon,
                13 => FontRole::CommandIconFallback,
                _ => continue,
            };
            let family = HSTRING::from(PRIMARY_FONT_FAMILY);
            let size = font_role_size(&role);
            let weight = font_role_weight(&role);
            let locale = HSTRING::from("en-US");
            if let Ok(format) = unsafe {
                self.dwrite_factory.CreateTextFormat(
                    &family, None, weight,
                    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
                    size, &locale,
                )
            } {
                self.text_formats[role_idx] = Some(format);
            } else {
                crate::logging::warn(&format!(
                    "[d2d] failed to precreate text format for role {}", role_idx
                ));
            }
        }

        let locale = HSTRING::from("en-US");
        let icon_size = 14.0;
        for (idx, family_str) in [ICON_FONT_FAMILY_PRIMARY, ICON_FONT_FAMILY_FALLBACK].iter().enumerate() {
            if self.icon_font_formats[idx].is_some() {
                continue;
            }
            let family = HSTRING::from(*family_str);
            if let Ok(format) = unsafe {
                self.dwrite_factory.CreateTextFormat(
                    &family, None, DWRITE_FONT_WEIGHT(400),
                    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
                    icon_size, &locale,
                )
            } {
                self.icon_font_formats[idx] = Some(format);
            } else {
                crate::logging::warn(&format!(
                    "[d2d] failed to precreate icon font format {} ('{}')", idx, family_str
                ));
            }
        }
    }

    pub(crate) fn text_format(&self, role: FontRole) -> Option<&IDWriteTextFormat> {
        let idx = role as usize;
        self.text_formats.get(idx)?.as_ref()
    }

    pub(crate) fn icon_text_format(&self, primary: bool) -> Option<&IDWriteTextFormat> {
        let idx = if primary { 0 } else { 1 };
        self.icon_font_formats.get(idx)?.as_ref()
    }

    pub(crate) fn measure_text_width(
        &self,
        format: &IDWriteTextFormat,
        text: &str,
    ) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        match unsafe {
            self.dwrite_factory.CreateTextLayout(
                &wide, format,
                10000.0, 10000.0,
            )
        } {
            Ok(layout) => {
                let mut metrics: DWRITE_TEXT_METRICS = unsafe { std::mem::zeroed() };
                unsafe {
                    let _ = layout.GetMetrics(&mut metrics);
                }
                metrics.widthIncludingTrailingWhitespace
            }
            Err(e) => {
                crate::logging::warn(&format!(
                    "[d2d] CreateTextLayout failed for measurement: {:?}", e
                ));
                0.0
            }
        }
    }

    pub(crate) fn measure_text_size(
        &self,
        format: &IDWriteTextFormat,
        text: &str,
    ) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        match unsafe {
            self.dwrite_factory.CreateTextLayout(
                &wide, format,
                10000.0, 10000.0,
            )
        } {
            Ok(layout) => {
                let mut metrics: DWRITE_TEXT_METRICS = unsafe { std::mem::zeroed() };
                unsafe {
                    let _ = layout.GetMetrics(&mut metrics);
                }
                (metrics.widthIncludingTrailingWhitespace, metrics.height)
            }
            Err(e) => {
                crate::logging::warn(&format!(
                    "[d2d] CreateTextLayout failed for measurement: {:?}", e
                ));
                (0.0, 0.0)
            }
        }
    }

    pub(crate) fn ensure_dc_render_target(&mut self) -> &ID2D1DCRenderTarget {
        if self.dc_render_target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            if let Ok(rt) = unsafe { self._factory.CreateDCRenderTarget(&props) } {
                unsafe {
                    rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                }
                self.dc_render_target = Some(rt);
            }
        }
        self.dc_render_target.as_ref().expect("failed to create DC render target")
    }

    pub(crate) fn begin_draw_on_dc(
        &mut self,
        hdc: *mut std::ffi::c_void,
        rect: &RECT,
    ) -> bool {
        self.dc_render_target = None;
        self.dc_brushes.clear();

        self.ensure_dc_render_target();
        let dc_rt = match self.dc_render_target.as_ref() {
            Some(rt) => rt,
            None => return false,
        };

        unsafe {
            let _ = dc_rt.BindDC(WindowsHDC(hdc), rect as *const RECT);
        }
        unsafe {
            dc_rt.BeginDraw();
        }
        true
    }

    pub(crate) fn end_draw_on_dc(&mut self) {
        if let Some(ref rt) = self.dc_render_target {
            unsafe {
                let _ = rt.EndDraw(None, None);
            }
        }
    }

    fn dc_make_brush(&self, color: u32) -> Option<ID2D1SolidColorBrush> {
        let rt = self.dc_render_target.as_ref()?;
        let r = ((color >> 16) & 0xFF) as f32 / 255.0;
        let g = ((color >> 8) & 0xFF) as f32 / 255.0;
        let b = (color & 0xFF) as f32 / 255.0;
        let alpha = ((color >> 24) & 0xFF) as f32 / 255.0;
        let c = D2D1_COLOR_F {
            r,
            g,
            b,
            a: if alpha == 0.0 { 1.0 } else { alpha },
        };
        unsafe { rt.CreateSolidColorBrush(&c as *const D2D1_COLOR_F, None).ok() }
    }

    fn dc_brush(&mut self, color: u32) -> Option<ID2D1SolidColorBrush> {
        if !self.dc_brushes.contains_key(&color) {
            if let Some(brush) = self.dc_make_brush(color) {
                self.dc_brushes.insert(color, brush);
            }
        }
        self.dc_brushes.get(&color).cloned()
    }

    pub(crate) fn dc_fill_rectangle(&mut self, rect: &D2D_RECT_F, color: u32) {
        let brush = self.dc_brush(color);
        let Some(ref dc_rt) = self.dc_render_target else { return };
        let Some(brush) = brush else { return };
        unsafe {
            dc_rt.FillRectangle(rect as *const D2D_RECT_F, &brush);
        }
    }

    pub(crate) fn dc_fill_rounded_rectangle(
        &mut self, rect: &D2D_RECT_F, radius: f32, color: u32,
    ) {
        let brush = self.dc_brush(color);
        let Some(ref dc_rt) = self.dc_render_target else {
            crate::logging::warn("[d2d] dc_fill_rounded_rectangle: rt is None");
            return;
        };
        let Some(brush) = brush else {
            crate::logging::warn("[d2d] dc_fill_rounded_rectangle: brush is None");
            return;
        };
        use windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT;
        let rounded = D2D1_ROUNDED_RECT {
            rect: *rect, radiusX: radius, radiusY: radius,
        };
        unsafe {
            crate::logging::info(&format!(
                "[d2d] FillRoundedRect ({},{})-({},{}) r={}",
                rect.left, rect.top, rect.right, rect.bottom, radius,
            ));
            dc_rt.FillRoundedRectangle(&rounded, &brush);
        }
    }

    pub(crate) fn dc_draw_text(
        &mut self, text: &str, rect: &D2D_RECT_F, color: u32, format: &IDWriteTextFormat,
    ) {
        let layout = self.create_text_layout(text, format, rect.right - rect.left, rect.bottom - rect.top);
        let brush = self.dc_brush(color);
        let Some(layout) = layout else { return };
        let Some(ref dc_rt) = self.dc_render_target else { return };
        let Some(brush) = brush else { return };
        let origin = D2D_POINT_2F { x: rect.left, y: rect.top };
        unsafe {
            dc_rt.DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    pub(crate) fn dc_draw_text_layout(
        &mut self, origin: D2D_POINT_2F, layout: &IDWriteTextLayout, color: u32,
    ) {
        let brush = self.dc_brush(color);
        let Some(ref dc_rt) = self.dc_render_target else { return };
        let Some(brush) = brush else { return };
        unsafe {
            dc_rt.DrawTextLayout(origin, layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    pub(crate) fn create_text_layout(
        &self, text: &str, format: &IDWriteTextFormat, max_width: f32, max_height: f32,
    ) -> Option<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.dwrite_factory.CreateTextLayout(
                &wide,
                format,
                max_width,
                max_height,
            ).ok()
        }
    }

    fn layer_params(opacity: f32) -> D2D1_LAYER_PARAMETERS {
        D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F { left: 0.0, top: 0.0, right: f32::MAX, bottom: f32::MAX },
            geometricMask: core::mem::ManuallyDrop::new(None::<ID2D1Geometry>),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 },
            opacity,
            opacityBrush: core::mem::ManuallyDrop::new(None::<ID2D1Brush>),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn begin_layer(&mut self, opacity: f32) {
        let Some(ref rt) = self.render_target else { return };
        if opacity >= 1.0 { return; }
        if self.layer.is_none() {
            if let Ok(layer) = unsafe { rt.CreateLayer(None) } {
                self.layer = Some(layer);
            }
        }
        let Some(ref layer) = self.layer else { return };
        let params = Self::layer_params(opacity);
        unsafe { rt.PushLayer(&params, layer); }
    }

    #[allow(dead_code)]
    pub(crate) fn end_layer(&mut self) {
        let Some(ref rt) = self.render_target else { return };
        unsafe { rt.PopLayer(); }
    }

    #[allow(dead_code)]
    pub(crate) fn dc_begin_layer(&mut self, opacity: f32) -> bool {
        let Some(ref dc_rt) = self.dc_render_target else { return false };
        if opacity >= 1.0 { return false; }
        let layer = match unsafe { dc_rt.CreateLayer(None) } {
            Ok(layer) => layer,
            Err(_) => return false,
        };
        self.layer = Some(layer);
        let Some(ref layer) = self.layer else { return false };
        let params = Self::layer_params(opacity);
        unsafe { dc_rt.PushLayer(&params, layer); }
        true
    }

    #[allow(dead_code)]
    pub(crate) fn dc_end_layer(&mut self) {
        let Some(ref dc_rt) = self.dc_render_target else { return };
        unsafe { dc_rt.PopLayer(); }
        self.layer = None;
    }

    pub(crate) fn get_or_create_hicon_bitmap(&mut self, hicon: isize, size: u32) -> Option<&ID2D1Bitmap> {
        if !self.bitmap_cache.contains_key(&hicon) {
            let Some(bitmap) = self.create_bitmap_from_hicon(hicon, size) else {
                return None;
            };
            self.bitmap_cache.insert(hicon, bitmap);
        }
        self.bitmap_cache.get(&hicon)
    }

    fn create_bitmap_from_hicon(&mut self, hicon: isize, size: u32) -> Option<ID2D1Bitmap> {
        let dc_rt = self.dc_render_target.as_ref()?;
        let size_i = size as i32;
        unsafe {
            let mut icon_info: ICONINFO = std::mem::zeroed();
            if GetIconInfo(HICON(hicon as *mut std::ffi::c_void), &mut icon_info).is_err() {
                return None;
            }

            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = size_i;
            bmi.bmiHeader.biHeight = -size_i;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = 0;

            let screen_dc = GetDC(None);
            let dib_dc = CreateCompatibleDC(Some(screen_dc));

            let mut pixel_data: *mut std::ffi::c_void = std::ptr::null_mut();
            let dib_bitmap = CreateDIBSection(
                Some(dib_dc), &bmi, DIB_RGB_COLORS, &mut pixel_data, None, 0,
            ).ok()?;

            let old_bmp = SelectObject(dib_dc, HGDIOBJ(dib_bitmap.0));

            let _ = DrawIconEx(
                dib_dc, 0, 0,
                HICON(hicon as *mut std::ffi::c_void),
                size_i, size_i, 0, None, DI_NORMAL,
            );

            // BI_RGB 32bpp DIBs from DrawIconEx don't have valid alpha channel.
            // Fix pixel data: non-zero pixels get alpha=255 (fully opaque),
            // zero (black) pixels stay transparent. Then premultiply for D2D.
            let total_pixels = (size * size) as usize;
            let pixels = std::slice::from_raw_parts_mut(pixel_data as *mut u32, total_pixels);
            for px in pixels.iter_mut() {
                if *px & 0x00FFFFFF != 0 {
                    let b = *px & 0xFF;
                    let g = (*px >> 8) & 0xFF;
                    let r = (*px >> 16) & 0xFF;
                    *px = b | (g << 8) | (r << 16) | (0xFF << 24);
                } else {
                    *px = 0;
                }
            }

            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };

            let bitmap = dc_rt.CreateBitmap(
                D2D_SIZE_U { width: size, height: size },
                Some(pixel_data as *const std::ffi::c_void),
                (size * 4) as u32,
                &props,
            ).ok();

            SelectObject(dib_dc, old_bmp);
            let _ = DeleteDC(dib_dc);
            let _ = DeleteObject(HGDIOBJ(dib_bitmap.0));
            let _ = ReleaseDC(None, screen_dc);
            if !icon_info.hbmColor.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(icon_info.hbmColor.0));
            }
            if !icon_info.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(icon_info.hbmMask.0));
            }

            bitmap
        }
    }

    pub(crate) fn dc_draw_bitmap(&mut self, rect: &D2D_RECT_F, bitmap: &ID2D1Bitmap) {
        let Some(ref dc_rt) = self.dc_render_target else { return };
        unsafe {
            let _ = dc_rt.DrawBitmap(
                bitmap,
                Some(rect as *const D2D_RECT_F),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
    }

    pub(crate) fn destroy(&mut self) {
        self.render_target = None;
        self.dc_render_target = None;
        self.brushes.clear();
        self.dc_brushes.clear();
        self.text_formats.clear();
        self.icon_font_formats = [None, None];
        self.bitmap_cache.clear();
    }
}

impl Drop for D2dRenderer {
    fn drop(&mut self) {
        self.destroy();
    }
}
