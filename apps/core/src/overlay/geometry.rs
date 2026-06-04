//! Layout tokens that match the legacy `windows_overlay::types` constants
//! 1:1. They are used by the view to position the search input, divider,
//! result rows, and footer hint.
//!
//! Values are in logical pixels (1 px = 1 unit at 96 DPI). The view
//! scales them by the system DPI factor before handing them to Iced
//! widgets.

#![allow(dead_code)]

pub(crate) const WINDOW_WIDTH: f32 = 576.0;
pub(crate) const WINDOW_OFFSET_Y: f32 = 0.0;
pub(crate) const PANEL_MARGIN_X: f32 = 14.0;
pub(crate) const PANEL_MARGIN_BOTTOM: f32 = 8.0;

pub(crate) const INPUT_HEIGHT: f32 = 36.0;
pub(crate) const DIVIDER_HEIGHT: f32 = 1.0;
pub(crate) const DIVIDER_BOTTOM_SPACING: f32 = 5.0;
pub(crate) const INPUT_TO_LIST_GAP: f32 =
    DIVIDER_HEIGHT + DIVIDER_BOTTOM_SPACING;

pub(crate) const ROW_HEIGHT: f32 = 58.0;
pub(crate) const ROW_INSET_X: f32 = 10.0;
pub(crate) const ROW_ICON_SIZE: f32 = 34.0;
pub(crate) const ROW_ICON_DRAW_SIZE: f32 = 32.0;
pub(crate) const ROW_ICON_GAP: f32 = 10.0;
pub(crate) const ROW_VERTICAL_INSET: f32 = 2.0;
pub(crate) const ROW_TITLE_BLOCK_HEIGHT: f32 = 21.0;
pub(crate) const ROW_META_BLOCK_HEIGHT: f32 = 16.0;
pub(crate) const ROW_TEXT_LINE_GAP: f32 = 3.0;
pub(crate) const ROW_ACTIVE_RADIUS: f32 = 8.0;

pub(crate) const HEADER_ROW_LABEL_HEIGHT: f32 = 14.0;
pub(crate) const HEADER_ROW_LINE_GAP: f32 = 10.0;

pub(crate) const FOOTER_HINT_HEIGHT: f32 = 26.0;
pub(crate) const FOOTER_SEPARATOR_HEIGHT: f32 = 1.0;
pub(crate) const FOOTER_CONTENT_PAD_Y: f32 = 4.0;
pub(crate) const FOOTER_CONTENT_PAD_X: f32 = 14.0;
pub(crate) const FOOTER_SEPARATOR_TO_CONTENT_GAP: f32 = 10.0;

pub(crate) const MAX_VISIBLE_ROWS: usize = 8;

pub(crate) const FOOTER_HINT_TEXT: &str =
    "Enter Open  \u{2022}  \u{2191}\u{2193} Move  \u{2022}  Esc Close";

pub(crate) const NO_RESULTS_STATUS_TEXT: &str = "No results";
pub(crate) const INPUT_PLACEHOLDER_TEXT: &str = "Type to search";
pub(crate) const COMMAND_INPUT_PLACEHOLDER_TEXT: &str =
    "Search the web or run a command";

pub(crate) fn dpi_scale(dpi: u32) -> f32 {
    (dpi as f32) / 96.0
}
