//! Dark + light theme palettes and system theme detection.
//!
//! The colour values mirror `windows_overlay::types::PALETTE_DARK` and
//! `PALETTE_LIGHT`. They are stored as RGBA u32 in the legacy module
//! (with the alpha channel always 0 because legacy GDI doesn't use
//! per-pixel alpha at this layer). Here we use `palette::Srgba` so
//! they can be passed directly to Iced.

use palette::Srgba;

/// Convert the palette's RGBA representation to Iced's `Color`. We
/// keep this in a free helper to avoid orphan-rule conflicts — both
/// `iced::Color` and `palette::Srgba` are foreign types, so we can't
/// implement `From` directly.
pub(crate) fn to_iced(c: Srgba) -> iced::Color {
    iced::Color::from_rgba(c.red, c.green, c.blue, c.alpha)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Theme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Palette {
    pub(crate) panel_bg: Srgba,
    pub(crate) panel_border: Srgba,
    pub(crate) input_bg: Srgba,
    pub(crate) results_bg: Srgba,
    pub(crate) text_primary: Srgba,
    pub(crate) text_secondary: Srgba,
    pub(crate) text_error: Srgba,
    pub(crate) text_highlight: Srgba,
    pub(crate) text_hint: Srgba,
    pub(crate) text_section: Srgba,
    pub(crate) text_hint_footer: Srgba,
    pub(crate) text_mode_strip: Srgba,
    pub(crate) selection: Srgba,
    pub(crate) selection_border: Srgba,
    pub(crate) row_hover: Srgba,
    pub(crate) row_separator: Srgba,
    pub(crate) selection_accent: Srgba,
    pub(crate) icon_bg: Srgba,
    pub(crate) icon_text: Srgba,
    pub(crate) help_icon: Srgba,
    pub(crate) help_icon_hover: Srgba,
    pub(crate) help_tip_bg: Srgba,
    pub(crate) help_tip_text: Srgba,
}

const fn hex(color: u32) -> Srgba {
    let r = ((color >> 16) & 0xFF) as u8;
    let g = ((color >> 8) & 0xFF) as u8;
    let b = (color & 0xFF) as u8;
    Srgba::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        1.0,
    )
}

pub(crate) const PALETTE_DARK: Palette = Palette {
    panel_bg: hex(0x00272727),
    panel_border: hex(0x00424242),
    input_bg: hex(0x00272727),
    results_bg: hex(0x00272727),
    text_primary: hex(0x00F5F5F5),
    text_secondary: hex(0x00C4C4C4),
    text_error: hex(0x00E8E8E8),
    text_highlight: hex(0x00FFFFFF),
    text_hint: hex(0x00BEBEBE),
    text_section: hex(0x009E9E9E),
    text_hint_footer: hex(0x009A9A9A),
    text_mode_strip: hex(0x00ABABAB),
    selection: hex(0x00262626),
    selection_border: hex(0x00383838),
    row_hover: hex(0x00313131),
    row_separator: hex(0x00161616),
    selection_accent: hex(0x00343434),
    icon_bg: hex(0x001D1D1D),
    icon_text: hex(0x00F0F0F0),
    help_icon: hex(0x00B5B5B5),
    help_icon_hover: hex(0x00F5F5F5),
    help_tip_bg: hex(0x00272727),
    help_tip_text: hex(0x00B5B5B5),
};

pub(crate) const PALETTE_LIGHT: Palette = Palette {
    panel_bg: hex(0x00F3F3F3),
    panel_border: hex(0x00C9C9C9),
    input_bg: hex(0x00F3F3F3),
    results_bg: hex(0x00F3F3F3),
    text_primary: hex(0x001A1A1A),
    text_secondary: hex(0x003F3F3F),
    text_error: hex(0x003E3E3E),
    text_highlight: hex(0x000D0D0D),
    text_hint: hex(0x00606060),
    text_section: hex(0x00606060),
    text_hint_footer: hex(0x00686868),
    text_mode_strip: hex(0x00626262),
    selection: hex(0x00E5E5E5),
    selection_border: hex(0x00D3D3D3),
    row_hover: hex(0x00ECECEC),
    row_separator: hex(0x00DCDCDC),
    selection_accent: hex(0x00D8D8D8),
    icon_bg: hex(0x00DFDFDF),
    icon_text: hex(0x00202020),
    help_icon: hex(0x00505050),
    help_icon_hover: hex(0x001A1A1A),
    help_tip_bg: hex(0x00F3F3F3),
    help_tip_text: hex(0x00505050),
};

pub(crate) fn palette_for_theme(theme: Theme) -> Palette {
    match theme {
        Theme::Dark => PALETTE_DARK,
        Theme::Light => PALETTE_LIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_for_theme_dark_returns_dark_palette() {
        assert_eq!(
            palette_for_theme(Theme::Dark).text_primary,
            PALETTE_DARK.text_primary
        );
    }

    #[test]
    fn palette_for_theme_light_returns_light_palette() {
        assert_eq!(
            palette_for_theme(Theme::Light).panel_bg,
            PALETTE_LIGHT.panel_bg
        );
    }

    #[test]
    fn dark_and_light_palettes_differ() {
        assert_ne!(PALETTE_DARK.panel_bg, PALETTE_LIGHT.panel_bg);
    }
}
