use std::time::Duration;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::{CloseDialog, button_row};
use crate::close_prompt::{Age, Choices, Purpose};
use crate::documents::Unsaved;
use crate::view::Workspace;

fn age_text(since_edit: Option<Duration>) -> String {
    match since_edit.map(Age::of) {
        None => t!("close.edited_earlier").to_string(),
        Some(Age::Now) => t!("close.edited_now").to_string(),
        Some(Age::Minutes(count)) => t!("close.edited_minutes", count = count),
        Some(Age::Hours(count)) => t!("close.edited_hours", count = count),
        Some(Age::Days(count)) => t!("close.edited_days", count = count),
    }
}

impl Workspace {
    pub(in crate::view) fn ask_to_close_app(
        &mut self,
        items: Vec<Unsaved>,
        reopen_offered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_close_dialog(
            Purpose::App,
            Choices::new(items, reopen_offered),
            None,
            window,
            cx,
        );
    }

    fn toggle_close_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.close_dialog.as_mut() {
            dialog.choices.toggle(index);
            cx.notify();
        }
    }

    fn toggle_close_all(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.close_dialog.as_mut() {
            dialog.choices.toggle_all();
            cx.notify();
        }
    }

    fn toggle_close_reopen(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.close_dialog.as_mut() {
            dialog.choices.reopen = !dialog.choices.reopen;
            cx.notify();
        }
    }

    pub(super) fn app_content(&self, panel: Div, dialog: &CloseDialog, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let choices = &dialog.choices;
        let total = choices.items().len();
        let rows = choices
            .items()
            .iter()
            .enumerate()
            .map(|(index, item)| self.close_row(index, item, choices.is_selected(index), cx));
        let select_all = h_flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1p5()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(
                Checkbox::new("close-all")
                    .checked(choices.all_selected())
                    .label(t!("close.select_all"))
                    .flex_1()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_close_all(cx))),
            )
            .child(div().child(t!(
                "close.selected_count",
                selected = choices.selected_count(),
                total = total
            )));
        panel
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("close.title", count = total)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(t!("close.detail")),
                    ),
            )
            .child(
                v_flex()
                    .p_1()
                    .rounded_lg()
                    .bg(theme.title_bar)
                    .child(select_all)
                    .child(div().h_px().mx_3().bg(theme.border))
                    .children(rows),
            )
            .when(choices.reopen_offered, |panel| {
                panel.child(
                    Checkbox::new("close-reopen")
                        .checked(choices.reopen)
                        .label(t!("close.reopen"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_close_reopen(cx))),
                )
            })
            .child(button_row(
                t!("close.save_selected").to_string(),
                dialog.save_enabled(),
            ))
    }

    fn close_row(
        &self,
        index: usize,
        item: &Unsaved,
        selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let place = match &item.folder {
            Some(folder) => folder.display().to_string(),
            None => t!("close.never_saved").to_string(),
        };
        Checkbox::new(("close-row", index))
            .checked(selected)
            .accessibility_label(item.name.clone())
            .w_full()
            .px_3()
            .py_2()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_close_row(index, cx)))
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().child(item.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(place),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(age_text(item.since_edit)),
                    ),
            )
    }
}
