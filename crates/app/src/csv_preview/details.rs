use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme, Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::CsvPreview;
use super::columns::Format;
use super::controls::{DISABLED_OPACITY, focus_ring, labelled, radio_box};
use super::words::{self, Note};
use crate::interface_scale::rem_px;

pub(super) const NOTE_HEIGHT: f32 = 54.0;
pub(super) const PANEL_HEIGHT: f32 = 90.0;

#[derive(Clone, Copy)]
enum Jump {
    Column(usize),
    NextFlagged,
}

impl CsvPreview {
    pub(super) fn render_note(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let note = words::note(&self.columns, &self.formats, self.keep_all);
        let intro = words::note_intro(&note);
        let stops: Vec<(String, Jump)> = match note {
            Note::Sentence(_) => Vec::new(),
            Note::Flagged { entries, more } => entries
                .into_iter()
                .map(|entry| (entry.text, Jump::Column(entry.column)))
                .chain((more > 0).then(|| (t!("csv.note.more", number = more), Jump::NextFlagged)))
                .collect(),
        };
        let line = h_flex()
            .h(rem_px(18.0))
            .items_center()
            .gap(px(14.0))
            .children(stops.into_iter().enumerate().map(|(stop, (text, jump))| {
                let focused = self.note_stops[stop].is_focused(window);
                h_flex()
                    .when(stop > 0, |wrapper| {
                        wrapper.pl(px(14.0)).border_l_1().border_color(theme.border)
                    })
                    .child(
                        div()
                            .id(("csv-note-entry", stop))
                            .key_context("CsvControl")
                            .role(Role::Button)
                            .aria_label(text.clone())
                            .track_focus(&self.note_stops[stop])
                            .relative()
                            .cursor_pointer()
                            .underline()
                            .font_weight(FontWeight::MEDIUM)
                            .on_click(cx.listener(move |this, _, window, cx| match jump {
                                Jump::Column(column) => {
                                    this.select(column, cx);
                                    this.focus_strip(window, cx);
                                }
                                Jump::NextFlagged => this.select_next_flagged(cx),
                            }))
                            .child(text)
                            .when(focused, |entry| entry.child(focus_ring(theme))),
                    )
            }));
        h_flex()
            .id("csv-note")
            .role(Role::Status)
            .h(rem_px(NOTE_HEIGHT))
            .flex_none()
            .items_start()
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .bg(theme.popover)
            .overflow_hidden()
            .child(
                Icon::new(IconName::Info)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(theme.muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().h(rem_px(18.0)).truncate().child(intro))
                    .child(line),
            )
    }

    fn render_radios(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let open = self.formats_are_open();
        let focused = open && self.radios.is_focused(window);
        let current = self.format_of(self.selected);
        let row = |format: Format, label: &'static str, key: &'static str| {
            let selected = current == format;
            h_flex()
                .id(match format {
                    Format::General => "csv-format-general",
                    Format::Text => "csv-format-text",
                })
                .role(Role::RadioButton)
                .aria_label(label)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .relative()
                .h(rem_px(24.0))
                .gap_2()
                .items_center()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_format(format, cx);
                    if this.formats_are_open() {
                        window.focus(&this.radios, cx);
                    }
                }))
                .child(radio_box(selected, theme))
                .child(labelled(label, key, self.alt_held))
                .when(focused && selected, |line| line.child(focus_ring(theme)))
        };
        v_flex()
            .id("csv-radios")
            .key_context("CsvRadios")
            .role(Role::RadioGroup)
            .aria_label(t!("csv.format.group"))
            .when(open, |group| group.track_focus(&self.radios))
            .when(!open, |group| group.opacity(DISABLED_OPACITY))
            .child(row(
                Format::General,
                t!("csv.format.general"),
                t!("csv.mnemonic.general"),
            ))
            .child(row(
                Format::Text,
                t!("csv.format.text"),
                t!("csv.mnemonic.text"),
            ))
    }

    pub(super) fn render_panel(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let words::Panel {
            title,
            reason,
            warns,
        } = words::panel(
            self.columns.get(self.selected),
            self.format_of(self.selected),
            self.keep_all,
        );
        let radios = self.render_radios(window, cx);
        h_flex()
            .h(rem_px(PANEL_HEIGHT))
            .flex_none()
            .gap(rem_px(16.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                v_flex()
                    .w(rem_px(200.0))
                    .flex_none()
                    .child(
                        div()
                            .h(rem_px(18.0))
                            .text_color(theme.muted_foreground)
                            .child(t!("csv.format.group")),
                    )
                    .child(radios),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .h(rem_px(36.0))
                            .overflow_hidden()
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_clamp(2)
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .h(rem_px(36.0))
                            .overflow_hidden()
                            .gap_1p5()
                            .items_start()
                            .text_color(if warns {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            })
                            .when(warns, |line| {
                                line.child(
                                    div().flex_none().mt(px(2.0)).child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size_3p5()
                                            .text_color(theme.warning),
                                    ),
                                )
                            })
                            .child(div().flex_1().min_w_0().line_clamp(2).child(reason)),
                    ),
            )
    }
}
