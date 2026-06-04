//! Pure widget tree built from the [`Model`]. No side effects, no
//! state reads — every value is a property of the model.

use iced::widget::{column, container, rule, text, text_input, Column, Row};
use iced::{Background, Border, Color, Length, Padding, Theme};

use crate::overlay::geometry::*;
use crate::overlay::model::{Model, OverlayRow, OverlayRowRole, Message};
use crate::overlay::theme::{palette_for_theme, to_iced, Palette};

const FONT_INPUT_PT: f32 = 15.0;
const FONT_TITLE_PT: f32 = 13.0;
const FONT_META_PT: f32 = 11.0;
const FONT_SECTION_PT: f32 = 11.0;
const FONT_FOOTER_PT: f32 = 11.0;
const INPUT_TEXT_LEFT_INSET: f32 = 19.0;
const INPUT_TEXT_RIGHT_INSET: f32 = 10.0;
const FOOTER_HINT_FILL_WIDTH: f32 = WINDOW_WIDTH - 2.0 * FOOTER_CONTENT_PAD_X;

pub fn view<'a>(model: &'a Model) -> iced::Element<'a, Message> {
    let palette = palette_for_theme(model.theme);

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

    let divider = rule::horizontal(1).style(divider_style(palette));

    let rows: iced::Element<'a, Message> = {
        let mut col = Column::new().spacing(0.0);
        for (i, r) in model.rows.iter().enumerate() {
            col = col.push(row_view(r, i, palette));
        }
        col.into()
    };

    let footer = footer_hint(palette);

    let body = column![input, divider, rows, footer]
        .spacing(0.0)
        .width(Length::Fixed(WINDOW_WIDTH));

    container(body).style(panel_style(palette)).into()
}

fn row_view<'a>(
    row: &'a OverlayRow,
    visible_index: usize,
    palette: Palette,
) -> iced::Element<'a, Message> {
    match row.role {
        OverlayRowRole::Header => header_label(&row.title, palette),
        OverlayRowRole::Status => status_label(&row.title, palette),
        OverlayRowRole::Calculator => result_row_widget(row, visible_index, palette, true),
        OverlayRowRole::TopHit | OverlayRowRole::Item => {
            result_row_widget(row, visible_index, palette, false)
        }
    }
}

fn header_label<'a>(label: &'a str, palette: Palette) -> iced::Element<'a, Message> {
    container(
        text(label.to_uppercase())
            .size(FONT_SECTION_PT)
            .color(to_iced(palette.text_section)),
    )
    .padding(Padding {
        top: 8.0,
        right: ROW_INSET_X,
        bottom: 8.0,
        left: ROW_INSET_X,
    })
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
        Row::with_children([icon_placeholder(palette), text_col])
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

fn icon_placeholder<'a>(palette: Palette) -> iced::Element<'a, Message> {
    let bg = to_iced(palette.icon_bg);
    container(text(""))
        .width(Length::Fixed(ROW_ICON_DRAW_SIZE))
        .height(Length::Fixed(ROW_ICON_DRAW_SIZE))
        .style(solid_bg(bg))
        .into()
}

fn footer_hint<'a>(palette: Palette) -> iced::Element<'a, Message> {
    container(
        text(FOOTER_HINT_TEXT)
            .size(FONT_FOOTER_PT)
            .color(to_iced(palette.text_hint_footer)),
    )
    .padding(Padding {
        top: FOOTER_CONTENT_PAD_Y,
        right: FOOTER_CONTENT_PAD_X,
        bottom: FOOTER_CONTENT_PAD_Y,
        left: FOOTER_CONTENT_PAD_X,
    })
    .width(Length::Fixed(FOOTER_HINT_FILL_WIDTH))
    .into()
}

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
