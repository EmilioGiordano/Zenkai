use std::time::Duration;

use gpui_kit::base::{Disableable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::Workspace;
use crate::actions::{
    CancelCloseDialog, ConfirmCloseDialog, DiscardCloseDialog, FocusNextControl,
    FocusPreviousControl,
};
use crate::close_prompt::{Age, Choices, ClosePlan, Departure, Purpose};
use crate::document::Document;
use crate::documents::Unsaved;

const PANEL_WIDTH: f32 = 520.0;

pub(super) struct CloseDialog {
    purpose: Purpose,
    choices: Choices,
    // The edit count of the workbook when its tab was asked about, to notice later edits.
    edits: Option<u64>,
    focus: FocusHandle,
}

fn age_text(since_edit: Option<Duration>) -> String {
    match since_edit.map(Age::of) {
        None => t!("close.edited_earlier").to_string(),
        Some(Age::Now) => t!("close.edited_now").to_string(),
        Some(Age::Minutes(count)) => t!("close.edited_minutes", count = count),
        Some(Age::Hours(count)) => t!("close.edited_hours", count = count),
        Some(Age::Days(count)) => t!("close.edited_days", count = count),
    }
}

fn dispatch_on_click(
    id: &'static str,
    label: String,
    action: impl Fn() -> Box<dyn Action> + 'static,
) -> Button {
    Button::new(id)
        .label(label)
        .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
}

impl Workspace {
    pub(super) fn ask_to_close_tab(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(unsaved) = self
            .documents
            .unsaved()
            .into_iter()
            .find(|unsaved| unsaved.id == id)
        else {
            self.finish_close(id, window, cx);
            return;
        };
        let edits = self.documents.get(id).map(Document::edit_count);
        self.open_close_dialog(
            Purpose::Tab(id),
            Choices::new(vec![unsaved], false),
            edits,
            window,
            cx,
        );
    }

    pub(super) fn ask_to_close_app(
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

    fn open_close_dialog(
        &mut self,
        purpose: Purpose,
        choices: Choices,
        edits: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.close_dialog = Some(CloseDialog {
            purpose,
            choices,
            edits,
            focus,
        });
        cx.notify();
    }

    fn dismiss_close_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<CloseDialog> {
        let dialog = self.close_dialog.take()?;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
        Some(dialog)
    }

    pub(super) fn confirm_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.close_dialog.as_ref() else {
            return;
        };
        if dialog.purpose == Purpose::App && dialog.choices.selected_count() == 0 {
            return;
        }
        let Some(dialog) = self.dismiss_close_dialog(window, cx) else {
            return;
        };
        let departure = match dialog.purpose {
            Purpose::Tab(id) => Departure::closing_tab(id),
            Purpose::App => Departure::closing_app(
                dialog.choices.ids_to_save(),
                ClosePlan {
                    discard: dialog.choices.ids_to_discard(),
                    start_empty: !dialog.choices.reopen,
                },
            ),
        };
        self.begin_departure(departure, window, cx);
    }

    pub(super) fn discard_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dismiss_close_dialog(window, cx) else {
            return;
        };
        match dialog.purpose {
            Purpose::Tab(id) => {
                let edits = self.documents.get(id).map(Document::edit_count);
                if edits == dialog.edits {
                    self.finish_close(id, window, cx);
                } else {
                    self.close_document(id, window, cx);
                }
            }
            Purpose::App => self.run_close(
                ClosePlan {
                    discard: dialog.choices.all_ids(),
                    start_empty: !dialog.choices.reopen,
                },
                window,
                cx,
            ),
        }
    }

    pub(super) fn cancel_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_close_dialog(window, cx);
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

    pub(super) fn render_close_dialog(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.close_dialog.as_ref()?;
        let theme = cx.theme();
        let panel = v_flex()
            .key_context("CloseDialog")
            .track_focus(&dialog.focus)
            .occlude()
            .on_action(|_: &FocusNextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPreviousControl, window, cx| window.focus_prev(cx))
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .p_5()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg();
        let panel = match dialog.purpose {
            Purpose::Tab(_) => self.tab_content(panel, dialog, cx),
            Purpose::App => self.app_content(panel, dialog, cx),
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.overlay)
                .occlude()
                .child(panel),
        )
    }

    fn tab_content(&self, panel: Div, dialog: &CloseDialog, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let name = dialog
            .choices
            .items()
            .first()
            .map(|item| item.name.clone())
            .unwrap_or_default();
        panel
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("close.tab_title", name = name)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(t!("close.tab_detail")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(div().key_context("CloseButton").child(dispatch_on_click(
                        "close-discard",
                        t!("close.dont_save").to_string(),
                        || Box::new(DiscardCloseDialog),
                    )))
                    .child(div().key_context("CloseButton").child(dispatch_on_click(
                        "close-cancel",
                        t!("button.cancel").to_string(),
                        || Box::new(CancelCloseDialog),
                    )))
                    .child(
                        dispatch_on_click("close-save", t!("close.save").to_string(), || {
                            Box::new(ConfirmCloseDialog)
                        })
                        .primary(),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("close.tab_hint")),
            )
    }

    fn app_content(&self, panel: Div, dialog: &CloseDialog, cx: &Context<Self>) -> Div {
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
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(div().key_context("CloseButton").child(dispatch_on_click(
                        "close-discard",
                        t!("close.dont_save").to_string(),
                        || Box::new(DiscardCloseDialog),
                    )))
                    .child(div().key_context("CloseButton").child(dispatch_on_click(
                        "close-cancel",
                        t!("button.cancel").to_string(),
                        || Box::new(CancelCloseDialog),
                    )))
                    .child(
                        dispatch_on_click(
                            "close-save",
                            t!("close.save_selected").to_string(),
                            || Box::new(ConfirmCloseDialog),
                        )
                        .primary()
                        .disabled(choices.selected_count() == 0),
                    ),
            )
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
