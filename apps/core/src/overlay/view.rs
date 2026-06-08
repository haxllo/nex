//! Pure widget tree built from the [`Model`]. No side effects, no
//! state reads — every value is a property of the model.

use iced::widget::{column, container, rule, text, text_input, tooltip, Column, Row};
use iced::{Alignment, Background, Border, Color, Length, Padding, Theme};

use crate::overlay::geometry::*;
use crate::overlay::icons::IconCache;
use crate::overlay::model::{Model, OverlayRow, OverlayRowRole, Message};
use crate::overlay::theme::{palette_for_theme, to_iced, Palette};

const FONT_INPUT_PT: f32 = 15.0;
const FONT_TITLE_PT: f32 = 13.0;
const FONT_META_PT: f32 = 11.0;
const FONT_SECTION_PT: f32 = 11.0;
const FONT_FOOTER_PT: f32 = 11.0;
const FONT_MODE_STRIP_PT: f32 = 10.5;
const INPUT_TEXT_LEFT_INSET: f32 = 8.0;
const INPUT_TEXT_RIGHT_INSET: f32 = 10.0;
const SEARCH_ICON_SIZE: f32 = 16.0;
const HELP_ICON_SIZE: f32 = 14.0;
const FOOTER_HINT_FILL_WIDTH: f32 = WINDOW_WIDTH - 2.0 * FOOTER_CONTENT_PAD_X;

pub fn view<'a>(model: &'a Model, icon_cache: &'a IconCache) -> iced::Element<'a, Message> {
    if !model.visible {
        return Column::new()
            .width(Length::Fixed(WINDOW_WIDTH))
            .into();
    }

    let palette = palette_for_theme(model.theme);

    // ── P4: Mode strip ──────────────────────────────────────────
    let mode_strip = mode_strip_row(&model.mode_strip_text, palette);

    // ── P1 + P3: Search input row with search icon and help icon ─
    let search_icon = text("\u{1F50D}")
        .size(SEARCH_ICON_SIZE)
        .color(to_iced(palette.text_hint));

    let input = text_input(INPUT_PLACEHOLDER_TEXT, &model.query)
        .on_input(Message::QueryInputChanged)
        .on_submit(Message::SubmitRequested)
        .padding(Padding {
            top: 0.0,
            right: INPUT_TEXT_RIGHT_INSET,
            bottom: 0.0,
            left: INPUT_TEXT_LEFT_INSET,
        })
        .size(FONT_INPUT_PT)
        .style(input_style(palette));

    let help_tip = help_tooltip(&model.help_config_path, palette);

    let input_row = Row::new()
        .push(
            container(search_icon)
                .padding(Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: SEARCH_ICON_LEFT_PAD,
                })
                .align_y(Alignment::Center),
        )
        .push(input)
        .push(
            container(help_tip)
                .padding(Padding {
                    top: 0.0,
                    right: HELP_ICON_RIGHT_PAD,
                    bottom: 0.0,
                    left: 0.0,
                })
                .align_y(Alignment::Center),
        )
        .align_y(Alignment::Center)
        .height(Length::Fixed(INPUT_HEIGHT));

    let divider = rule::horizontal(1).style(divider_style(palette));

    let rows: iced::Element<'a, Message> = {
        let mut col = Column::new().spacing(0.0);
        for (i, r) in model.rows.iter().enumerate() {
            col = col.push(row_view(r, i, palette, icon_cache));
        }
        col.into()
    };

    // ── P2: Styled keycap footer ────────────────────────────────
    let footer = keycap_footer(palette);

    let mut body_col = Column::new().spacing(0.0);

    // Only show mode strip when there's text.
    if !model.mode_strip_text.is_empty() {
        body_col = body_col.push(mode_strip);
    }
    body_col = body_col.push(input_row);
    body_col = body_col.push(divider);
    body_col = body_col.push(rows);
    body_col = body_col.push(footer);

    let body = body_col
        .spacing(0.0)
        .width(Length::Fixed(WINDOW_WIDTH));

    container(body).style(panel_style(palette)).into()
}

// ─────────────────────────────────────────────────────────────────
// P4: Mode strip
// ─────────────────────────────────────────────────────────────────

fn mode_strip_row<'a>(label: &'a str, palette: Palette) -> iced::Element<'a, Message> {
    container(
        text(label)
            .size(FONT_MODE_STRIP_PT)
            .color(to_iced(palette.text_mode_strip)),
    )
    .padding(Padding {
        top: 12.0,
        right: ROW_INSET_X,
        bottom: 4.0,
        left: ROW_INSET_X,
    })
    .width(Length::Fill)
    .into()
}

// ─────────────────────────────────────────────────────────────────
// P3: Help tooltip
// ─────────────────────────────────────────────────────────────────

fn help_tooltip<'a>(config_path: &'a str, palette: Palette) -> iced::Element<'a, Message> {
    let help_icon = container(
        text("?")
            .size(HELP_ICON_SIZE)
            .color(to_iced(palette.help_icon)),
    )
    .width(Length::Fixed(HELP_ICON_SIZE))
    .height(Length::Fixed(HELP_ICON_SIZE))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(rounded_help_bg(palette));

    let tip_text = if config_path.is_empty() {
        "Click to change hotkey".to_string()
    } else {
        config_path.to_string()
    };

    let tip = container(
        text(tip_text)
            .size(11.0)
            .color(to_iced(palette.help_tip_text)),
    )
    .padding(Padding {
        top: 4.0,
        right: 8.0,
        bottom: 4.0,
        left: 8.0,
    })
    .style(help_tip_bg_style(palette));

    tooltip(help_icon, tip, tooltip::Position::Bottom).into()
}

fn rounded_help_bg(
    palette: Palette,
) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_: &Theme| iced::widget::container::Style {
        text_color: Some(to_iced(palette.help_icon)),
        background: Some(Background::Color(to_iced(palette.icon_bg))),
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Default::default(),
        snap: true,
    }
}

fn help_tip_bg_style(
    palette: Palette,
) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_: &Theme| iced::widget::container::Style {
        text_color: Some(to_iced(palette.help_tip_text)),
        background: Some(Background::Color(to_iced(palette.help_tip_bg))),
        border: Border {
            color: to_iced(palette.panel_border),
            width: 1.0,
            radius: 6.0.into(),
        },
        shadow: Default::default(),
        snap: true,
    }
}

// ─────────────────────────────────────────────────────────────────
// P2: Styled keycap footer
// ─────────────────────────────────────────────────────────────────

fn keycap_footer<'a>(palette: Palette) -> iced::Element<'a, Message> {
    let group1 = keycap_group(FOOTER_KEY_ENTER, FOOTER_LABEL_OPEN, palette);
    let group2 = keycap_group_pair(
        FOOTER_KEY_UP,
        FOOTER_KEY_DOWN,
        FOOTER_LABEL_MOVE,
        palette,
    );
    let group3 = keycap_group(FOOTER_KEY_ESC, FOOTER_LABEL_CLOSE, palette);

    let row = Row::new()
        .push(group1)
        .push(footer_separator(palette))
        .push(group2)
        .push(footer_separator(palette))
        .push(group3)
        .align_y(Alignment::Center)
        .spacing(0.0);

    container(row)
        .padding(Padding {
            top: FOOTER_CONTENT_PAD_Y,
            right: FOOTER_CONTENT_PAD_X,
            bottom: FOOTER_CONTENT_PAD_Y,
            left: FOOTER_CONTENT_PAD_X,
        })
        .width(Length::Fixed(FOOTER_HINT_FILL_WIDTH))
        .into()
}

fn keycap_group<'a>(
    key_text: &'a str,
    label: &'a str,
    palette: Palette,
) -> iced::Element<'a, Message> {
    let key = keycap(key_text, palette);
    let lbl = text(label)
        .size(FONT_FOOTER_PT)
        .color(to_iced(palette.text_hint_footer));

    Row::new()
        .push(key)
        .push(lbl)
        .spacing(FOOTER_KEY_LABEL_GAP)
        .align_y(Alignment::Center)
        .into()
}

fn keycap_group_pair<'a>(
    key1: &'a str,
    key2: &'a str,
    label: &'a str,
    palette: Palette,
) -> iced::Element<'a, Message> {
    let k1 = keycap(key1, palette);
    let k2 = keycap(key2, palette);
    let lbl = text(label)
        .size(FONT_FOOTER_PT)
        .color(to_iced(palette.text_hint_footer));

    Row::new()
        .push(k1)
        .push(k2)
        .push(lbl)
        .spacing(FOOTER_KEYCAP_GAP)
        .align_y(Alignment::Center)
        .into()
}

fn keycap<'a>(key_text: &'a str, palette: Palette) -> iced::Element<'a, Message> {
    let fg = to_iced(palette.text_primary);
    // Blend keycap bg closer to panel for a subtle keycap look.
    let bg = to_iced(palette.selection);

    container(
        text(key_text)
            .size(FONT_FOOTER_PT - 1.0)
            .color(fg),
    )
    .padding(Padding {
        top: 1.0,
        right: 4.0,
        bottom: 1.0,
        left: 4.0,
    })
    .style(move |_: &Theme| iced::widget::container::Style {
        text_color: Some(fg),
        background: Some(Background::Color(bg)),
        border: Border {
            color: to_iced(palette.selection_border),
            width: 1.0,
            radius: 3.0.into(),
        },
        shadow: Default::default(),
        snap: true,
    })
    .into()
}

fn footer_separator<'a>(palette: Palette) -> iced::Element<'a, Message> {
    container(
        text(FOOTER_SEPARATOR)
            .size(FONT_FOOTER_PT)
            .color(to_iced(palette.text_hint_footer)),
    )
    .padding(Padding {
        top: 0.0,
        right: 4.0,
        bottom: 0.0,
        left: 4.0,
    })
    .into()
}

// ─────────────────────────────────────────────────────────────────
// Result rows, headers, status
// ─────────────────────────────────────────────────────────────────

fn row_view<'a>(
    row: &'a OverlayRow,
    visible_index: usize,
    palette: Palette,
    icon_cache: &'a IconCache,
) -> iced::Element<'a, Message> {
    match row.role {
        OverlayRowRole::Header => header_label(&row.title, palette),
        OverlayRowRole::Status => status_label(&row.title, palette),
        OverlayRowRole::Calculator => result_row_widget(row, visible_index, palette, icon_cache, true),
        OverlayRowRole::TopHit | OverlayRowRole::Item => {
            result_row_widget(row, visible_index, palette, icon_cache, false)
        }
    }
}

/// P5: Section header with separator line underneath.
fn header_label<'a>(label: &'a str, palette: Palette) -> iced::Element<'a, Message> {
    let header_text = text(label.to_uppercase())
        .size(FONT_SECTION_PT)
        .color(to_iced(palette.text_section));

    let separator = rule::horizontal(1).style(divider_style(palette));

    column![
        container(header_text).padding(Padding {
            top: 8.0,
            right: ROW_INSET_X,
            bottom: 4.0,
            left: ROW_INSET_X,
        }),
        separator,
    ]
    .spacing(0.0)
    .into()
}

fn status_label<'a>(label: &'a str, palette: Palette) -> iced::Element<'a, Message> {
    container(
        text(label)
            .size(FONT_META_PT)
            .color(to_iced(palette.text_hint)),
    )
    .padding(Padding {
        top: 6.0,
        right: ROW_INSET_X,
        bottom: 6.0,
        left: ROW_INSET_X,
    })
    .into()
}

fn result_row_widget<'a>(
    row: &'a OverlayRow,
    visible_index: usize,
    palette: Palette,
    icon_cache: &'a IconCache,
    is_calculator: bool,
) -> iced::Element<'a, Message> {
    let is_selected = visible_index == 0;
    let title_color = if is_selected {
        palette.text_highlight
    } else {
        palette.text_primary
    };
    let title = text(&row.title)
        .size(FONT_TITLE_PT)
        .color(to_iced(title_color));
    let meta: iced::Element<'a, Message> = if row.path.is_empty() {
        text("").into()
    } else {
        text(&row.path)
            .size(FONT_META_PT)
            .color(to_iced(palette.text_secondary))
            .into()
    };

    let text_col: iced::Element<'a, Message> = column![title, meta]
        .spacing(ROW_TEXT_LINE_GAP as f32)
        .into();

    let body: iced::Element<'a, Message> = if is_calculator {
        Row::with_children([text_col]).into()
    } else {
        Row::with_children([result_icon(&row.icon_path, icon_cache, palette), text_col])
            .spacing(ROW_ICON_GAP)
            .into()
    };

    let bg = if is_selected {
        palette.selection
    } else {
        palette.results_bg
    };
    let bg_color = to_iced(bg);
    container(body)
        .padding(Padding {
            top: ROW_VERTICAL_INSET,
            right: ROW_INSET_X,
            bottom: ROW_VERTICAL_INSET,
            left: ROW_INSET_X,
        })
        .height(Length::Fixed(ROW_HEIGHT))
        .width(Length::Fill)
        .style(solid_bg(bg_color))
        .into()
}

fn result_icon<'a>(
    icon_path: &str,
    icon_cache: &IconCache,
    palette: Palette,
) -> iced::Element<'a, Message> {
    if icon_path.is_empty() {
        return icon_placeholder(palette);
    }
    match icon_cache.get_image(icon_path) {
        Some(image) => {
            container(image)
                .width(Length::Fixed(ROW_ICON_DRAW_SIZE))
                .height(Length::Fixed(ROW_ICON_DRAW_SIZE))
                .align_x(iced::Alignment::Center)
                .align_y(iced::Alignment::Center)
                .into()
        }
        None => icon_placeholder(palette),
    }
}

fn icon_placeholder<'a>(palette: Palette) -> iced::Element<'a, Message> {
    let bg = to_iced(palette.icon_bg);
    container(text(""))
        .width(Length::Fixed(ROW_ICON_DRAW_SIZE))
        .height(Length::Fixed(ROW_ICON_DRAW_SIZE))
        .style(solid_bg(bg))
        .into()
}

// ─────────────────────────────────────────────────────────────────
// Styling helpers
// ─────────────────────────────────────────────────────────────────

fn panel_style(
    palette: Palette,
) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_: &Theme| iced::widget::container::Style {
        text_color: Some(to_iced(palette.text_primary)),
        background: Some(Background::Color(to_iced(palette.panel_bg))),
        border: Border {
            color: to_iced(palette.panel_border),
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: Default::default(),
        snap: true,
    }
}

fn solid_bg(
    bg: Color,
) -> impl Fn(&Theme) -> iced::widget::container::Style {
    move |_: &Theme| iced::widget::container::Style {
        text_color: None,
        background: Some(Background::Color(bg)),
        border: Border::default(),
        shadow: Default::default(),
        snap: true,
    }
}

fn input_style(
    palette: Palette,
) -> impl Fn(&Theme, iced::widget::text_input::Status) -> iced::widget::text_input::Style
{
    move |_: &Theme, _status: iced::widget::text_input::Status| {
        iced::widget::text_input::Style {
            background: Background::Color(to_iced(palette.input_bg)),
            border: Border {
                color: to_iced(palette.input_bg),
                width: 0.0,
                radius: 0.0.into(),
            },
            icon: to_iced(palette.text_secondary),
            placeholder: to_iced(palette.text_hint),
            value: to_iced(palette.text_primary),
            selection: to_iced(palette.selection_accent),
        }
    }
}

fn divider_style(
    palette: Palette,
) -> impl Fn(&Theme) -> iced::widget::rule::Style {
    move |_: &Theme| iced::widget::rule::Style {
        color: to_iced(palette.row_separator),
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
        snap: true,
    }
}
