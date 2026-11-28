use std::rc::Rc;

use gpui_kit::base::{Disableable, FocusableExt, Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_formats::{DateOrder, DecimalMark, Delimiter};
use zenkai_i18n::t;

use super::columns::Format;
use super::controls::{DISABLED_OPACITY, RING_REACH, checkbox_box, focus_ring, labelled};
use super::words;
use super::{
    CANCEL_STOP, CsvPreview, DATE_STOPS, DECIMAL_STOPS, OPEN_STOP, SEPARATOR_STOPS,
    delimiter_label, fit,
};
use crate::actions::*;
use crate::interface_scale::rem_px;

const DELIMITERS: [Delimiter; 4] = [
    Delimiter::Comma,
    Delimiter::Semicolon,
    Delimiter::Tab,
    Delimiter::Pipe,
];
const TITLE_HEIGHT: f32 = 28.0;
const ROW_HEIGHT: f32 = 30.0;
const FOOTER_HEIGHT: f32 = 34.0;
// Under the height of the row, so the ring of a choice has room inside the section.
const CHOICE_HEIGHT: f32 = 28.0;
// The section clips what is drawn past its edge, and a focus ring reaches 5px past a stop: the
// section is that much larger on every side, and its content stays where it was.
const RING_ROOM: f32 = 6.0;

// A button of the dialog as a stop: where it is among them, what a screen reader calls it, whether
// it is a choice that is on or off, and whether it can be used now.
struct Stop {
    index: usize,
    name: &'static str,
    chosen: Option<bool>,
    enabled: bool,
}

type Activation = Rc<dyn Fn(&mut CsvPreview, &mut Window, &mut Context<CsvPreview>)>;

fn activation(
    run: impl Fn(&mut CsvPreview, &mut Window, &mut Context<CsvPreview>) + 'static,
) -> Activation {
    Rc::new(run)
}

impl Render for CsvPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = f32::from(window.rem_size()) / 16.0;
        let fitted = fit::fit(f32::from(window.viewport_size().height), scale);
        self.rows_changed(fitted.rows);
        self.section_scrolls = fitted.scrolls;
        self.keep_focus(window, cx);
        self.follow_focus(window, cx);
        let table = self.render_table(window, cx).into_any_element();
        let settings = self.render_settings(window, cx).into_any_element();
        let keep_all = self.render_keep_all(window, cx).into_any_element();
        let note = self.render_note(window, cx).into_any_element();
        let panel = self.render_panel(window, cx).into_any_element();
        let title = self.render_title();
        let footer = self.render_footer(window, cx);
        let theme = cx.theme();
        let section = v_flex()
            .id("csv-section")
            .track_scroll(&self.section_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .gap_3()
            .mx(px(-RING_ROOM))
            .my(px(-RING_ROOM))
            .px(px(RING_ROOM))
            .py(px(RING_ROOM))
            .child(self.block(0, settings))
            .child(self.block(1, keep_all))
            .child(self.block(2, note))
            .child(self.block(3, table))
            .child(self.block(4, panel));
        let dialog = v_flex()
            .key_context("CsvPreview")
            .track_focus(&self.focus)
            .occlude()
            .on_action(cx.listener(|this, _: &CsvColumnLeft, _, cx| this.select_offset(-1, cx)))
            .on_action(cx.listener(|this, _: &CsvColumnRight, _, cx| this.select_offset(1, cx)))
            .on_action(cx.listener(|this, _: &CsvColumnFirst, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &CsvColumnLast, _, cx| {
                this.select(this.columns.len().saturating_sub(1), cx)
            }))
            .on_action(
                cx.listener(|this, _: &CsvColumnPageBack, _, cx| this.select_page(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CsvColumnPageForward, _, cx| this.select_page(true, cx)),
            )
            .on_action(cx.listener(|this, _: &SwitchColumnFormat, _, cx| this.switch_format(cx)))
            .on_action(
                cx.listener(|this, _: &SetColumnGeneral, _, cx| {
                    this.set_format(Format::General, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SetColumnText, _, cx| this.set_format(Format::Text, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleKeepAllAsText, _, cx| this.toggle_keep_all(cx)))
            .on_action(
                cx.listener(|this, _: &ScrollCsvPreviewUp, _, cx| this.scroll_section(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ScrollCsvPreviewDown, _, cx| this.scroll_section(true, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusNextControl, window, cx| {
                this.cycle_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPreviousControl, window, cx| {
                this.cycle_focus(false, window, cx)
            }))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                if this.alt_held != event.modifiers.alt {
                    this.alt_held = event.modifiers.alt;
                    cx.notify();
                }
            }))
            .w(rem_px(920.0))
            .max_w_full()
            .h(px(fitted.height))
            .p_4()
            .gap_3()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .child(title)
            .child(section)
            .child(footer);
        div()
            .absolute()
            .top(px(fitted.top))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(dialog)
    }
}

impl CsvPreview {
    // Scrolls the part between the title and the footer so the block that took focus is fully
    // in view, which only matters when the window is too short for all of it.
    fn follow_focus(&mut self, window: &Window, cx: &App) {
        let focused = self
            .blocks
            .iter()
            .position(|block| block.contains_focused(window, cx));
        if focused != self.shown_block {
            if let Some(block) = focused {
                self.reveal_block(block);
            }
            self.shown_block = focused;
        }
    }

    // Moves the section the least that shows the block with the ring of its stops, which reach
    // past it; a block taller than the section shows from its top.
    fn reveal_block(&self, index: usize) {
        let Some(block) = self.section_scroll.bounds_for_item(index) else {
            return;
        };
        let view = self.section_scroll.bounds();
        let block_top = f32::from(block.origin.y) - RING_REACH;
        let block_bottom = f32::from(block.origin.y + block.size.height) + RING_REACH;
        let view_top = f32::from(view.origin.y);
        let view_bottom = view_top + f32::from(view.size.height);
        let shift = if block_top < view_top {
            view_top - block_top
        } else if block_bottom > view_bottom {
            view_bottom - block_bottom
        } else {
            return;
        };
        let offset = self.section_scroll.offset();
        let furthest = -f32::from(self.section_scroll.max_offset().y);
        let target = (f32::from(offset.y) + shift).clamp(furthest, 0.0);
        self.section_scroll.set_offset(point(offset.x, px(target)));
    }

    fn block(&self, index: usize, content: AnyElement) -> impl IntoElement {
        div()
            .track_focus(&self.blocks[index])
            .flex_none()
            .child(content)
    }

    fn render_title(&self) -> impl IntoElement {
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        div()
            .h(rem_px(TITLE_HEIGHT))
            .flex_none()
            .truncate()
            .text_lg()
            .font_weight(FontWeight::SEMIBOLD)
            .child(t!("csv.title", name = name))
    }

    // A button of the dialog with the focus ring of the other stops, not the library's. The stop
    // is a wrapper that keeps the focus and answers Enter and Space; the button answers clicks.
    fn stop_button(
        &self,
        button: Button,
        stop: Stop,
        activate: Activation,
        window: &Window,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let Stop {
            index: stop,
            name,
            chosen,
            enabled,
        } = stop;
        let focused = enabled && self.button_stops[stop].is_focused(window);
        let by_click = activate.clone();
        div()
            .id(("csv-button", stop))
            .relative()
            .flex_none()
            .when(enabled, |wrapper| {
                wrapper
                    .role(Role::Button)
                    .aria_label(name)
                    .track_focus(&self.button_stops[stop])
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.is_keyboard() {
                            activate(this, window, cx);
                        }
                    }))
            })
            .when_some(chosen.filter(|_| enabled), |wrapper, chosen| {
                wrapper.aria_toggled(if chosen {
                    Toggled::True
                } else {
                    Toggled::False
                })
            })
            .child(
                button
                    .focus_ring(false)
                    .tab_stop(false)
                    .disabled(!enabled)
                    .on_click(cx.listener(move |this, _, window, cx| by_click(this, window, cx))),
            )
            .when(focused, |wrapper| wrapper.child(focus_ring(cx.theme())))
    }

    fn render_settings(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let reading_open = !self.keep_all;
        let choice = |stop: usize,
                      label: &'static str,
                      selected: bool,
                      enabled: bool,
                      activate: Activation| {
            // The check mark keeps the choice readable without colour.
            let text = if selected {
                format!("\u{2713} {label}")
            } else {
                label.to_string()
            };
            let button = Button::new(("csv-choice", stop))
                .ghost()
                .compact()
                .label(text)
                .h(rem_px(CHOICE_HEIGHT))
                .selected(selected);
            let stop = Stop {
                index: stop,
                name: label,
                chosen: Some(selected),
                enabled,
            };
            self.stop_button(button, stop, activate, window, cx)
        };
        let group = |name: &'static str, enabled: bool, buttons: Vec<Stateful<Div>>| {
            h_flex()
                .key_context("CsvControl")
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .when(!enabled, |dimmed| dimmed.opacity(DISABLED_OPACITY))
                        .child(name),
                )
                .children(buttons)
        };
        let separators: Vec<Stateful<Div>> = DELIMITERS
            .into_iter()
            .enumerate()
            .map(|(index, delimiter)| {
                choice(
                    SEPARATOR_STOPS + index,
                    delimiter_label(delimiter),
                    self.parsed.delimiter == delimiter,
                    true,
                    activation(move |this, _, cx| this.choose_separator(delimiter, cx)),
                )
            })
            .collect();
        let decimals: Vec<Stateful<Div>> = [(DecimalMark::Dot, "1.5"), (DecimalMark::Comma, "1,5")]
            .into_iter()
            .enumerate()
            .map(|(index, (mark, label))| {
                choice(
                    DECIMAL_STOPS + index,
                    label,
                    self.guess.decimal_mark == mark,
                    reading_open,
                    activation(move |this, _, cx| this.choose_decimal_mark(mark, cx)),
                )
            })
            .collect();
        let dates: Vec<Stateful<Div>> = [
            (DateOrder::DayFirst, t!("csv.date_day_first")),
            (DateOrder::MonthFirst, t!("csv.date_month_first")),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (order, label))| {
            choice(
                DATE_STOPS + index,
                label,
                self.guess.date_order == order,
                reading_open,
                activation(move |this, _, cx| this.choose_date_order(order, cx)),
            )
        })
        .collect();
        h_flex()
            .h(rem_px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .text_sm()
            .child(group(t!("csv.separator"), true, separators))
            .child(div().w(rem_px(8.0)))
            .child(group(t!("csv.decimal"), reading_open, decimals))
            .child(div().w(rem_px(8.0)))
            .child(group(t!("csv.dates"), reading_open, dates))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .text_color(theme.muted_foreground)
                    .child(t!(
                        "csv.summary",
                        encoding = self.parsed.encoding.label(),
                        rows = t!("csv.rows", count = self.parsed.rows.len()),
                        columns = t!("csv.columns", count = self.columns.len())
                    )),
            )
    }

    fn render_keep_all(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let enabled = self.has_columns();
        let focused = enabled && self.keep_all_stop.is_focused(window);
        let label = t!("csv.keep_all");
        let toggled = if self.keep_all {
            Toggled::True
        } else {
            Toggled::False
        };
        h_flex()
            .h(rem_px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .id("csv-keep-all")
                    .key_context("CsvControl")
                    .role(Role::CheckBox)
                    .aria_label(label)
                    .aria_toggled(toggled)
                    .relative()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .when(enabled, |stop| {
                        stop.track_focus(&self.keep_all_stop)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_keep_all(cx)))
                    })
                    .when(!enabled, |stop| stop.opacity(DISABLED_OPACITY))
                    .child(checkbox_box(self.keep_all, theme))
                    .child(labelled(label, t!("csv.mnemonic.keep_all"), self.alt_held))
                    .when(focused, |stop| stop.child(focus_ring(theme))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .line_height(rem_px(18.0))
                    .text_color(theme.muted_foreground)
                    .child(words::keep_all_hint(self.keep_all)),
            )
    }

    fn render_footer(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let cancel = Button::new("csv-cancel")
            .label(t!("button.cancel"))
            .h(rem_px(FOOTER_HEIGHT));
        let open = Button::new("csv-import")
            .primary()
            .label(t!("csv.import"))
            .h(rem_px(FOOTER_HEIGHT));
        h_flex()
            .h(rem_px(FOOTER_HEIGHT))
            .flex_none()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rem_px(12.0))
                    .line_height(rem_px(18.0))
                    .text_color(theme.muted_foreground)
                    .child(words::footer_hint(self.keep_all, self.has_columns())),
            )
            .child(div().key_context("CsvControl").child(self.stop_button(
                cancel,
                Stop {
                    index: CANCEL_STOP,
                    name: t!("button.cancel"),
                    chosen: None,
                    enabled: true,
                },
                activation(|_, window, cx| window.dispatch_action(Box::new(CancelCsvImport), cx)),
                window,
                cx,
            )))
            .child(div().key_context("CsvControl").child(self.stop_button(
                open,
                Stop {
                    index: OPEN_STOP,
                    name: t!("csv.import"),
                    chosen: None,
                    enabled: true,
                },
                activation(|_, window, cx| window.dispatch_action(Box::new(ConfirmCsvImport), cx)),
                window,
                cx,
            )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::details::{NOTE_HEIGHT, PANEL_HEIGHT};
    use super::super::table::table_height;
    use super::*;

    #[test]
    fn the_blocks_add_up_to_the_height_of_the_dialog() {
        let blocks = TITLE_HEIGHT
            + ROW_HEIGHT * 2.0
            + NOTE_HEIGHT
            + table_height(12)
            + PANEL_HEIGHT
            + FOOTER_HEIGHT;
        let gaps = 12.0 * 6.0;
        let padding_and_border = 16.0 * 2.0 + 2.0;
        assert_eq!(blocks + gaps + padding_and_border, fit::DIALOG_HEIGHT);
    }
}
