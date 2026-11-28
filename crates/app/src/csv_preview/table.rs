use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Theme, h_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::columns::{Format, general_text, is_number};
use super::fit::ROW_HEIGHT;
use super::words::{format_label, header_label};
use super::{CsvPreview, columns_height};
use crate::interface_scale::rem_px;

pub const HEADER_HEIGHT: f32 = 48.0;
const SCROLLBAR_ROW: f32 = 10.0;
const TABLE_BORDER: f32 = 2.0;

pub fn table_height(rows: usize) -> f32 {
    HEADER_HEIGHT + rows as f32 * ROW_HEIGHT + SCROLLBAR_ROW + TABLE_BORDER
}

// Focus inside the header, 3px in: a 2px ring, then a 1px line of the foreground, which leaves
// 2px clear before the text. The outline of the selection is its outer neighbour.
fn strip_ring(theme: &Theme) -> Div {
    div()
        .absolute()
        .top(px(3.0))
        .left(px(3.0))
        .right(px(3.0))
        .bottom(px(3.0))
        .rounded(px(3.0))
        .border_2()
        .border_color(theme.ring)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .rounded(px(1.0))
                .border_1()
                .border_color(theme.foreground),
        )
}

fn selection_outline(theme: &Theme) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .border_2()
        .border_color(theme.foreground)
}

impl CsvPreview {
    pub(super) fn render_table(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let frame = v_flex()
            .h(rem_px(table_height(self.rows_shown)))
            .w_full()
            .flex_none()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .overflow_hidden();
        if !self.has_columns() {
            return frame.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(columns_height(self.rows_shown))
                    .text_color(theme.muted_foreground)
                    .child(t!("csv.nothing")),
            );
        }
        let sizes = self.column_sizes(window.rem_size());
        let list = h_virtual_list(
            cx.entity(),
            "csv-columns",
            sizes,
            |this, range, window, cx| {
                range
                    .map(|index| this.column_item(index, window, cx))
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.columns_scroll);
        let theme = cx.theme();
        frame
            .child(
                div()
                    .id("csv-strip")
                    .track_focus(&self.strip)
                    .h(columns_height(self.rows_shown))
                    .flex_none()
                    .child(list),
            )
            .child(
                div()
                    .relative()
                    .h(rem_px(SCROLLBAR_ROW))
                    .flex_none()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(
                                Scrollbar::horizontal(&self.columns_scroll).viewport_from_layout(),
                            ),
                    ),
            )
    }

    fn column_item(&self, index: usize, window: &Window, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let column = &self.columns[index];
        let format = self.format_of(index);
        let selected = index == self.selected;
        let focused = selected && self.strip.is_focused(window);
        let holds_zeros = column
            .risk
            .as_ref()
            .is_some_and(|risk| risk.leading_zero.is_some());
        let auto = !self.keep_all
            && column.risk.is_some()
            && self.formats.get(index) == Some(&Format::Text);
        let warns = format == Format::General && column.risk.is_some();
        let label_color = if selected {
            theme.foreground
        } else {
            theme.table_head_foreground
        };
        let header = v_flex()
            .id(("csv-column", index))
            .role(Role::ColumnHeader)
            .aria_label(header_label(column, format, self.keep_all, auto))
            .relative()
            .h(rem_px(HEADER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .py(px(8.0))
            .bg(if selected {
                theme.list_active
            } else {
                theme.table_head
            })
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(rem_px(16.0))
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(theme.foreground)
                            .font_weight(if selected {
                                FontWeight::BOLD
                            } else {
                                FontWeight::SEMIBOLD
                            })
                            .child(column.letter.clone()),
                    )
                    .when(auto, |line| {
                        line.child(
                            div()
                                .px(px(5.0))
                                .rounded_full()
                                .border_1()
                                .border_color(label_color)
                                .text_color(label_color)
                                .text_size(rem_px(12.0))
                                .child(t!("csv.auto")),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h(rem_px(16.0))
                    .items_center()
                    .gap_1()
                    .text_size(rem_px(12.0))
                    .text_color(label_color)
                    .when(warns, |line| {
                        line.child(
                            Icon::new(IconName::TriangleAlert)
                                .size_3()
                                .flex_shrink_0()
                                .text_color(theme.foreground),
                        )
                    })
                    .child(format_label(format)),
            )
            .when(focused, |header| header.child(strip_ring(theme)));
        let cells = (0..self.rows_shown).map(|row| {
            let raw = column.sample.get(row).map_or("", String::as_str);
            let shown = match format {
                Format::Text => raw.to_string(),
                Format::General => general_text(
                    raw,
                    self.guess.decimal_mark,
                    self.guess.date_order,
                    holds_zeros,
                ),
            };
            let right = format == Format::General && row > 0 && is_number(&shown);
            div()
                .h(rem_px(ROW_HEIGHT))
                .flex_none()
                .px(px(8.0))
                .truncate()
                .line_height(rem_px(ROW_HEIGHT))
                .when(row % 2 == 1, |cell| cell.bg(theme.table_even))
                .when(right, |cell| cell.text_right())
                .child(shown)
        });
        v_flex()
            .id(("csv-cells", index))
            .relative()
            .w(rem_px(column.width))
            .h(columns_height(self.rows_shown))
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.select(index, cx);
                    this.focus_strip(window, cx);
                }),
            )
            .child(header)
            .children(cells)
            .when(selected, |column| column.child(selection_outline(theme)))
            .into_any_element()
    }
}
