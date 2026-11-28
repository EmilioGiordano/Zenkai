use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, FocusableExt, Icon, IconName, Theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_engine::Engine;
use zenkai_i18n::t;
use zenkai_types::{LineAxis, SheetId};

use super::{Severity, Workspace, activates_button};
use crate::actions::{ApplyLineSize, CloseLineSizeDialog, FocusNextControl, FocusPreviousControl};
use crate::autofit;
use crate::contrast::readable;
use crate::dialog_focus;
use crate::focus_ring::focus_ring;
use crate::interface_scale::rem_px;
use crate::line_size::{Lines, Outcome, Prefill, Refusal, Separator, Slot, outcome};
use crate::line_size_apply;
use crate::line_size_text;

const DIALOG_WIDTH: f32 = 380.0;
const FIELD_HEIGHT: f32 = 32.0;
const SLOT_HEIGHT: f32 = 36.0;
const SLOT_LINE_HEIGHT: f32 = 17.0;
// The field, Cancel and OK.
const TAB_STOPS: usize = 3;
// WCAG AA for the error text, and for its icon as a graphic.
const ERROR_TEXT_CONTRAST: f32 = 4.5;
const ERROR_ICON_CONTRAST: f32 = 3.0;

pub(super) struct SizeDialog {
    lines: Lines,
    sheet: SheetId,
    generation: u64,
    input: Entity<InputState>,
    prefill: Prefill,
    prefill_text: String,
    separator: Separator,
    refused: Option<Refusal>,
    focus: FocusHandle,
    cancel_focus: FocusHandle,
    ok_focus: FocusHandle,
    _typing: Subscription,
}

impl Workspace {
    // Excel's Row Height and Column Width: one size for every row or column of the selection.
    pub(super) fn open_size_dialog(
        &mut self,
        axis: LineAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self
            .size_dialog
            .as_ref()
            .map(|dialog| (dialog.lines.axis, dialog.input.clone()));
        match open {
            // The command again: back to the field, as the text selected in it.
            Some((open_axis, input)) if open_axis == axis => {
                input.update(cx, |state, cx| {
                    state.focus(window, cx);
                    state.select_all(window, cx);
                });
                return;
            }
            Some(_) => self.close_size_dialog(window, cx),
            None => {}
        }
        if self.format_dialog.is_some() || self.user_is_editing(cx) {
            return;
        }
        let Some(document) = self.documents.active() else {
            return;
        };
        let (sheet, generation) = (document.sheet, document.generation());
        let sizes = document.workbook().map(|workbook| workbook.sizes(sheet));
        let Some(sizes) = sizes else {
            self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
            return;
        };
        let lines = Lines::of(axis, self.selection(cx));
        let separator = Separator::of(zenkai_i18n::active().language());
        let prefill = Prefill::of(lines, &sizes);
        let prefill_text = prefill.text(axis, separator);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(prefill_text.clone()));
        let typing = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.apply_size_dialog(window, cx),
            InputEvent::Change => {
                if let Some(dialog) = this.size_dialog.as_mut() {
                    dialog.refused = None;
                }
                cx.notify();
            }
            _ => {}
        });
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.size_dialog = Some(SizeDialog {
            lines,
            sheet,
            generation,
            input,
            prefill,
            prefill_text,
            separator,
            refused: None,
            focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle().tab_stop(true),
            ok_focus: cx.focus_handle().tab_stop(true),
            _typing: typing,
        });
        cx.notify();
    }

    pub(super) fn apply_size_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.size_dialog.as_ref() else {
            return;
        };
        let text = dialog.input.read(cx).value().to_string();
        let (lines, sheet, generation) = (dialog.lines, dialog.sheet, dialog.generation);
        match outcome(lines, &text, dialog.separator) {
            Outcome::Refuse(refusal) => self.refuse_size(refusal, window, cx),
            Outcome::Close => self.close_size_dialog(window, cx),
            Outcome::Apply(size) => {
                // Applied where it was opened, never to another sheet or document.
                let unchanged = self.documents.active().is_some_and(|document| {
                    document.sheet == sheet && document.generation() == generation
                });
                self.close_size_dialog(window, cx);
                if !unchanged {
                    self.notify(
                        Severity::Warning,
                        line_size_text::stale_notice(lines.axis),
                        cx,
                    );
                    return;
                }
                self.edit(window, cx, move |workbook| {
                    line_size_apply::apply(workbook, sheet, lines, size)
                });
            }
        }
    }

    fn refuse_size(&mut self, refusal: Refusal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.size_dialog.as_mut() else {
            return;
        };
        dialog.refused = Some(refusal);
        dialog.input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.notify();
    }

    pub(super) fn close_size_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.size_dialog.take().is_none() {
            return;
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn cycle_size_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.size_dialog.as_ref() else {
            return;
        };
        dialog_focus::cycle(&dialog.focus, TAB_STOPS, forward, window, cx);
    }

    // Each row or column of `lines` to its own content, in one undo step.
    pub(super) fn auto_fit(&mut self, lines: Lines, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        self.edit(window, cx, move |workbook| {
            workbook.as_one_undo_step(|workbook| match lines.axis {
                LineAxis::Row => autofit::fit_rows(workbook, sheet, lines),
                LineAxis::Column => autofit::fit_columns(workbook, sheet, lines),
            })
        });
    }

    pub(super) fn auto_fit_selection(
        &mut self,
        axis: LineAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines = Lines::of(axis, self.selection(cx));
        self.auto_fit(lines, window, cx);
    }

    pub(super) fn render_size_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let dialog = self.size_dialog.as_ref()?;
        let theme = cx.theme();
        let axis = dialog.lines.axis;
        let text = dialog.input.read(cx).value().to_string();
        let slot = Slot::of(
            dialog.lines,
            dialog.prefill,
            &dialog.prefill_text,
            &text,
            dialog.refused,
        );
        let error_text = readable(
            theme.danger,
            theme.popover,
            theme.foreground,
            ERROR_TEXT_CONTRAST,
        );
        let error_icon = readable(
            theme.danger,
            theme.popover,
            theme.foreground,
            ERROR_ICON_CONTRAST,
        );
        let (icon, icon_color, slot_color) = match slot {
            Slot::ZeroNote | Slot::HiddenNote => (
                Some(IconName::EyeOff),
                theme.muted_foreground,
                theme.muted_foreground,
            ),
            Slot::Refusal(_) => (Some(IconName::CircleX), error_icon, error_text),
            Slot::Hint | Slot::EveryLineHiddenNote => {
                (None, theme.muted_foreground, theme.muted_foreground)
            }
        };
        let field_focused = dialog.input.focus_handle(cx).is_focused(window);
        let field = h_flex()
            .items_center()
            .gap_2()
            .h(rem_px(FIELD_HEIGHT))
            .px(rem_px(10.0))
            .rounded_md()
            .border_1()
            .border_color(theme.muted_foreground)
            .bg(theme.popover)
            .when(field_focused, |field| field.shadow(focus_ring(theme)))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&dialog.input)
                        .px_0()
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .focus_ring(false)
                        .aria_label(line_size_text::field_label(axis)),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(line_size_text::unit(axis)),
            );
        let slot_row = h_flex()
            .id("line-size-slot")
            .h(rem_px(SLOT_HEIGHT))
            .overflow_hidden()
            .items_start()
            .gap(rem_px(7.0))
            .text_xs()
            .line_height(rem_px(SLOT_LINE_HEIGHT))
            .text_color(slot_color)
            .when(slot.is_alert(), |row| row.role(Role::Alert))
            .children(icon.map(|name| {
                Icon::new(name)
                    .size_3p5()
                    .flex_none()
                    .mt_0p5()
                    .text_color(icon_color)
            }))
            .child(div().flex_1().min_w_0().child(line_size_text::slot(
                dialog.lines,
                slot,
                dialog.separator,
            )));
        let buttons = h_flex()
            .gap_2()
            .justify_end()
            .mt(rem_px(2.0))
            .py(rem_px(4.0))
            .pr(rem_px(5.0))
            .child(dialog_button(
                ("line-size-cancel", "line-size-cancel-button"),
                t!("button.cancel"),
                &dialog.cancel_focus,
                false,
                (window, theme),
                || Box::new(CloseLineSizeDialog),
            ))
            .child(dialog_button(
                ("line-size-ok", "line-size-ok-button"),
                t!("button.ok"),
                &dialog.ok_focus,
                true,
                (window, theme),
                || Box::new(ApplyLineSize),
            ));
        let panel = v_flex()
            .id("line-size-dialog")
            .key_context("LineSizeDialog")
            .track_focus(&dialog.focus)
            .role(Role::Dialog)
            .aria_label(line_size_text::title(axis))
            // Clicks on the dialog must not reach the grid underneath.
            .occlude()
            .on_action(cx.listener(|this, _: &FocusNextControl, window, cx| {
                this.cycle_size_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPreviousControl, window, cx| {
                this.cycle_size_focus(false, window, cx)
            }))
            .w(rem_px(DIALOG_WIDTH))
            .p_4()
            .gap_3()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(line_size_text::title(axis)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(line_size_text::scope(dialog.lines, dialog.prefill)),
                    ),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(line_size_text::label(axis)),
                    )
                    .child(field)
                    .child(slot_row),
            )
            .child(buttons);
        Some(
            div()
                .absolute()
                .top(rem_px(96.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(panel),
        )
    }
}

// The button keeps the stock look; the wrapper owns the focus so Tab, Enter and Space
// reach it and the ring can be drawn around it.
fn dialog_button(
    (id, button_id): (&'static str, &'static str),
    label: &'static str,
    focus: &FocusHandle,
    primary: bool,
    (window, theme): (&Window, &Theme),
    action: fn() -> Box<dyn Action>,
) -> Stateful<Div> {
    let button = Button::new(button_id)
        .label(label)
        .tab_stop(false)
        .focus_ring(false)
        .on_click(move |_, window, cx| window.dispatch_action(action(), cx));
    let button = if primary { button.primary() } else { button };
    div()
        .id(id)
        .track_focus(focus)
        .rounded_md()
        .when(focus.is_focused(window), |wrapper| {
            wrapper.shadow(focus_ring(theme))
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if activates_button(event) {
                window.dispatch_action(action(), cx);
                cx.stop_propagation();
            }
        })
        .child(button)
}
