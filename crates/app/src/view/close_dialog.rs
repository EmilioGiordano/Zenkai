use gpui_kit::base::{Disableable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use crate::actions::{
    CancelCloseDialog, ConfirmCloseDialog, DiscardCloseDialog, FocusNextControl,
    FocusPreviousControl,
};
use crate::close_prompt::{Choices, CloseGoal, ClosePlan, Departure, Purpose};
use crate::document::Document;
use crate::interface_scale::rem_px;

mod app;
mod tab;

const PANEL_WIDTH: f32 = 520.0;
const BUTTONS: usize = 3;

pub(super) struct CloseDialog {
    purpose: Purpose,
    choices: Choices,
    // The edit count of the workbook when its tab was asked about, to notice later edits.
    edits: Option<u64>,
    focus: FocusHandle,
}

impl CloseDialog {
    fn save_enabled(&self) -> bool {
        self.purpose != Purpose::App || self.choices.selected_count() > 0
    }

    fn tab_stops(&self) -> usize {
        let buttons = if self.save_enabled() {
            BUTTONS
        } else {
            BUTTONS - 1
        };
        match self.purpose {
            Purpose::Tab(_) => buttons,
            Purpose::App => {
                buttons + 1 + self.choices.items().len() + usize::from(self.choices.reopen_offered)
            }
        }
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

// Cancel and Don't Save sit in a `CloseButton` context so Enter activates them, not Save.
fn button_row(save_label: String, save_enabled: bool) -> Div {
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
            dispatch_on_click("close-save", save_label, || Box::new(ConfirmCloseDialog))
                .primary()
                .disabled(!save_enabled),
        )
}

impl Workspace {
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
        if !dialog.save_enabled() {
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
                    goal: dialog.choices.goal,
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
                    goal: dialog.choices.goal,
                },
                window,
                cx,
            ),
        }
    }

    // Quitting wins over hiding: a Quit asked while a hide prompt is open makes the prompt a quit.
    pub(super) fn retarget_close_dialog(&mut self, goal: CloseGoal) {
        if let Some(dialog) = self.close_dialog.as_mut()
            && !goal.keeps_running()
        {
            dialog.choices.goal = goal;
        }
    }

    pub(super) fn focus_close_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.close_dialog.as_ref() {
            window.focus(&dialog.focus, cx);
        }
    }

    pub(super) fn cancel_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_close_dialog(window, cx);
    }

    // Tab wraps around inside the dialog; focus never lands on the window behind it.
    fn cycle_close_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.close_dialog.as_ref() else {
            return;
        };
        let focus = dialog.focus.clone();
        let stops = dialog.tab_stops();
        if forward {
            window.focus_next(cx);
            if !focus.contains_focused(window, cx) {
                window.focus(&focus, cx);
                window.focus_next(cx);
            }
        } else {
            window.focus_prev(cx);
            if !focus.contains_focused(window, cx) {
                window.focus(&focus, cx);
                for _ in 0..stops {
                    window.focus_next(cx);
                }
            }
        }
    }

    pub(super) fn render_close_dialog(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.close_dialog.as_ref()?;
        let theme = cx.theme();
        let panel = v_flex()
            .key_context("CloseDialog")
            .track_focus(&dialog.focus)
            .occlude()
            .on_action(cx.listener(|this, _: &FocusNextControl, window, cx| {
                this.cycle_close_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPreviousControl, window, cx| {
                this.cycle_close_focus(false, window, cx)
            }))
            .w(rem_px(PANEL_WIDTH))
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
}
