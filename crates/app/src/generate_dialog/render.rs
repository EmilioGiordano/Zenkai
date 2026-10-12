use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_datagen::{ColumnKind, Locale};
use zenkai_i18n::t;
use zenkai_types::ColIdx;

use super::draft::{Counted, KindSource, NameRole, Placement, Source, count_label};
use super::options::{self, KindChoice};
use super::{GenerateDialog, Panel};
use crate::actions::{
    AddGenerateColumn, CancelGenerate, ConfirmGenerate, FocusNextControl, FocusPreviousControl,
    SaveGenerateHeaders,
};

mod column;

const PANEL_WIDTH: f32 = 980.0;
const OPTIONS_WIDTH: f32 = 380.0;
const SUMMARY_CHARS: usize = 40;

fn choice_button(id: impl Into<ElementId>, label: &str, selected: bool) -> Button {
    let text = if selected {
        format!("✓ {label}")
    } else {
        label.to_string()
    };
    Button::new(id)
        .ghost()
        .compact()
        .label(text)
        .selected(selected)
}

fn ellipsize(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn labelled(label: &'static str, field: impl IntoElement, cx: &App) -> Div {
    v_flex().gap_1().child(caption(label, cx)).child(field)
}

fn menu_surface(width: f32, cx: &Context<GenerateDialog>) -> Div {
    let theme = cx.theme();
    v_flex()
        .occlude()
        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_panel(cx)))
        .w(px(width))
        .p_2()
        .gap_2()
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .shadow_lg()
}

// Drawn in the window's overlay layer, so the scrolling body cannot clip it.
fn below_trigger(menu: impl IntoElement) -> impl IntoElement {
    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .offset(point(px(0.0), px(34.0)))
            .snap_to_window()
            .child(menu),
    )
}

impl Render for GenerateDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = window.viewport_size().height - px(48.0);
        let theme = cx.theme();
        let panel = v_flex()
            .key_context("GenerateDialog")
            .track_focus(&self.focus)
            .occlude()
            .on_action(cx.listener(|this, _: &ConfirmGenerate, _, cx| this.generate(cx)))
            .on_action(cx.listener(|this, _: &CancelGenerate, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &SaveGenerateHeaders, _, cx| this.save_headers(cx)))
            .on_action(
                cx.listener(|this, _: &AddGenerateColumn, window, cx| this.add_column(window, cx)),
            )
            .on_action(|_: &FocusNextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPreviousControl, window, cx| window.focus_prev(cx))
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .max_h(height)
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .child(
                v_flex()
                    .id("generate-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .p_4()
                    .child(self.render_title(cx))
                    .child(self.render_globals(cx))
                    .child(self.render_columns(cx))
                    .child(self.render_preview(cx)),
            )
            .child(self.render_footer(cx));
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(24.0))
            .bg(cx.theme().overlay)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(super::DialogEvent::Close)),
            )
            .child(panel)
    }
}

impl GenerateDialog {
    fn detected_note(&self) -> String {
        let columns = self.draft.columns();
        let detected = columns
            .iter()
            .filter(|column| column.kind_source == KindSource::Detected)
            .count();
        let total = count_label(columns.len() as u32, Counted::Column);
        if detected == columns.len() {
            t!("gen.note_all", total = total)
        } else if detected == 0 {
            t!("gen.note_none", total = total)
        } else {
            t!("gen.note_some", total = total, detected = detected)
        }
    }

    fn render_title(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .items_start()
            .justify_between()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("gen.title")),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(caption(t!("gen.range"), cx))
                            .child(
                                div()
                                    .w(px(150.0))
                                    .child(Input::new(&self.range).aria_label(t!("gen.range"))),
                            )
                            .child(caption(t!("gen.on_sheet", sheet = self.sheet_name), cx))
                            .child(caption(self.detected_note(), cx)),
                    ),
            )
            .child(
                Button::new("generate-close")
                    .ghost()
                    .compact()
                    .label("✕")
                    .accessibility_label(t!("gen.close"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(super::DialogEvent::Close))),
            )
    }

    fn render_globals(&self, cx: &Context<Self>) -> impl IntoElement {
        let locale = self.draft.locale();
        let placement = self.draft.placement();
        let locales = [
            (Locale::SpanishArgentina, t!("gen.locale_es_ar")),
            (Locale::EnglishUnitedStates, t!("gen.locale_en_us")),
        ]
        .map(|(value, label)| {
            choice_button(("locale", value as usize), label, locale == value)
                .on_click(cx.listener(move |this, _, _, cx| this.set_locale(value, cx)))
        });
        let placements = [
            (Placement::BelowHeaders, t!("gen.placement_below")),
            (Placement::AfterData, t!("gen.placement_after")),
        ]
        .map(|(value, label)| {
            choice_button(("placement", value as usize), label, placement == value).on_click(
                cx.listener(move |this, _, window, cx| this.set_placement(value, window, cx)),
            )
        });
        h_flex()
            .flex_wrap()
            .items_start()
            .gap_4()
            .child(labelled(
                t!("gen.rows"),
                div().w(px(110.0)).child(
                    Input::new(&self.rows)
                        .aria_label(t!("gen.rows"))
                        .disabled(self.draft.rows_locked()),
                ),
                cx,
            ))
            .child(labelled(
                t!("gen.placement"),
                h_flex().gap_1().children(placements),
                cx,
            ))
            .child(labelled(
                t!("gen.language"),
                h_flex().gap_1().children(locales),
                cx,
            ))
            .child(labelled(
                t!("gen.seed_caption"),
                div()
                    .w(px(110.0))
                    .child(Input::new(&self.seed).aria_label(t!("gen.seed"))),
                cx,
            ))
    }

    fn render_columns(&self, cx: &Context<Self>) -> impl IntoElement {
        let rows = (0..self.draft.columns().len()).map(|index| self.render_column(index, cx));
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_3()
                    .px_2()
                    .child(div().w(px(176.0)).child(caption(t!("gen.column"), cx)))
                    .child(div().w(px(220.0)).child(caption(t!("gen.type"), cx)))
                    .child(div().flex_1().child(caption(t!("gen.options"), cx)))
                    .child(div().w(px(84.0)).child(caption(t!("gen.blanks"), cx)))
                    .child(div().w(px(64.0)).child(caption(t!("gen.unique"), cx))),
            )
            .children(rows)
            .child(
                h_flex().child(
                    Button::new("generate-add-column")
                        .ghost()
                        .compact()
                        .label(t!("gen.add_column"))
                        .disabled(!self.draft.can_add_column())
                        .on_click(cx.listener(|this, _, window, cx| this.add_column(window, cx))),
                ),
            )
    }

    fn render_preview(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let shown = self.draft.rows().min(super::draft::PREVIEW_ROWS as u32);
        let label = t!(
            "gen.preview",
            shown = shown,
            total = count_label(self.draft.rows(), Counted::Row)
        );
        let table = match self.current_ready().map(|ready| &ready.result) {
            Some(Ok(table)) => {
                let rows = self.draft.preview(table);
                let rows = rows.into_iter().enumerate().map(|(position, row)| {
                    h_flex()
                        .gap_3()
                        .px_3()
                        .py_1()
                        .when(position > 0, |line| {
                            line.border_t_1().border_color(theme.border)
                        })
                        .children(row.into_iter().map(|cell| {
                            let empty = cell.is_empty() && position > 0;
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .when(position == 0, |text| {
                                    text.font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.muted_foreground)
                                })
                                .when(empty, |text| {
                                    text.italic().text_color(theme.muted_foreground)
                                })
                                .child(if empty {
                                    t!("gen.empty_cell").to_string()
                                } else {
                                    cell
                                })
                        }))
                });
                v_flex().children(rows).into_any_element()
            }
            _ => {
                let message = if self.working {
                    t!("gen.updating_preview")
                } else {
                    t!("gen.no_preview")
                };
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .child(message)
                    .into_any_element()
            }
        };
        v_flex().gap_1().child(caption(label, cx)).child(
            div()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .overflow_hidden()
                .child(table),
        )
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let waiting = self.submit_when_ready;
        h_flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().child(self.draft.summary()))
                    .child(caption(t!("gen.footer_note"), cx))
                    .when_some(self.footer_error(), |column, message| {
                        column.child(
                            div()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(format!("⚠ {message}")),
                        )
                    }),
            )
            .child(
                Button::new("generate-cancel")
                    .label(t!("button.cancel"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(super::DialogEvent::Close))),
            )
            .when(self.draft.changed_headers() > 0, |row| {
                row.child(
                    Button::new("generate-headers")
                        .label(t!("gen.save_headers"))
                        .disabled(self.draft.has_local_issue())
                        .on_click(cx.listener(|this, _, _, cx| this.save_headers(cx))),
                )
            })
            .child(
                Button::new("generate-run")
                    .primary()
                    .label(if waiting {
                        t!("gen.generating").to_string()
                    } else {
                        self.draft.generate_label()
                    })
                    .disabled(!self.can_generate())
                    .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
            )
    }
}
