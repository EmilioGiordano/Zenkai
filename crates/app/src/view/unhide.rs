use gpui_kit::base::{Disableable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_engine::Engine;
use zenkai_i18n::t;

use super::{Severity, Workspace};
use crate::actions::{
    CancelUnhide, ConfirmUnhide, FocusNextControl, FocusPreviousControl, UnhideExtendDown,
    UnhideExtendUp, UnhideFirstRow, UnhideFocusDown, UnhideFocusUp, UnhideLastRow, UnhideMoveDown,
    UnhideMoveUp, UnhideSelectAll, UnhideToggleRow,
};
use crate::focus_ring::{focus_ring, inset_focus_ring};
use crate::interface_scale::rem_px;
use crate::scroll_thumb;
use crate::sheet_tabs;
use crate::unhide_dialog::{ROW_HEIGHT, Selection, UnhideDialog, rows_shown};

const DIALOG_WIDTH: f32 = 400.0;
const DIALOG_TOP: f32 = 96.0;
const DIALOG_BOTTOM_MARGIN: f32 = 16.0;
// Everything in the dialog that is not the list: title, label, footer, hint, padding, gaps.
const CHROME_HEIGHT: f32 = 230.0;
const CHECK: &str = "✓";

// A button the dialog focuses itself, so its ring is the shared one and not the widget's own.
// `focus` is `None` for a disabled button, which then takes no focus.
fn dialog_button(
    id: &'static str,
    label: &'static str,
    focus: Option<&FocusHandle>,
    primary: bool,
    action: Box<dyn Action>,
    window: &Window,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let focused = focus.is_some_and(|handle| handle.is_focused(window));
    let on_key = action.boxed_clone();
    let on_click = action;
    let mut wrapper = div()
        .id(id)
        .key_context("UnhideButton")
        .role(Role::Button)
        .aria_label(label)
        .rounded_md()
        .when(focused, |el| el.shadow(focus_ring(theme)))
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if super::activates_button(event) {
                window.dispatch_action(on_key.boxed_clone(), cx);
            }
        });
    if let Some(handle) = focus {
        wrapper = wrapper.track_focus(handle);
    }
    let mut button = Button::new((id, 1usize))
        .label(label)
        .tab_stop(false)
        .disabled(focus.is_none())
        .on_click(move |_, window, cx| window.dispatch_action(on_click.boxed_clone(), cx));
    if primary {
        button = button.primary();
    }
    wrapper.child(button)
}

impl Workspace {
    pub(super) fn open_unhide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let sheets = sheet_tabs::hidden(&document.sheets);
        if sheets.is_empty() {
            self.notify(Severity::Info, t!("notice.no_hidden_sheets"), cx);
            return;
        }
        let dialog = UnhideDialog {
            generation: document.generation(),
            sheets_version: document.sheets_version(),
            selection: Selection::new(sheets.len()),
            sheets,
            dialog_focus: cx.focus_handle(),
            list_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            ok_focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
        };
        self.tab_color = None;
        window.focus(&dialog.list_focus, cx);
        self.unhide = Some(dialog);
        cx.notify();
    }

    pub(super) fn close_unhide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.unhide.take().is_none() {
            return;
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn confirm_unhide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &self.unhide else {
            return;
        };
        let chosen = dialog.chosen();
        if chosen.is_empty() {
            return;
        }
        let unchanged = self.documents.active().is_some_and(|document| {
            document.generation() == dialog.generation
                && document.sheets_version() == dialog.sheets_version
        });
        self.close_unhide(window, cx);
        if !unchanged {
            return;
        }
        if let Some(document) = self.documents.active_mut() {
            document.note_unhide(chosen.clone());
        }
        self.edit(window, cx, move |wb| wb.unhide_sheets(&chosen));
    }

    fn change_unhide_selection(
        &mut self,
        change: impl FnOnce(&mut Selection),
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.unhide.as_mut() else {
            return;
        };
        change(&mut dialog.selection);
        dialog.scroll.scroll_to_item(dialog.selection.focus());
        cx.notify();
    }

    fn click_unhide_row(
        &mut self,
        row: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.modifiers();
        let double = event.click_count() >= 2;
        self.change_unhide_selection(
            |selection| {
                if double {
                    selection.click(row, false, false);
                } else {
                    selection.click(row, modifiers.secondary(), modifiers.shift);
                }
            },
            cx,
        );
        if let Some(dialog) = &self.unhide {
            window.focus(&dialog.list_focus, cx);
        }
        if double {
            self.confirm_unhide(window, cx);
        }
    }

    // Tab wraps around inside the dialog; focus never lands on the window behind it.
    fn cycle_unhide_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.unhide.as_ref() else {
            return;
        };
        let focus = dialog.dialog_focus.clone();
        let stops = if dialog.selection.count() > 0 { 3 } else { 2 };
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

    // The keys of the list, which the dialog's own context does not bind.
    pub(super) fn with_unhide_actions<E: InteractiveElement>(root: E, cx: &mut Context<Self>) -> E {
        root.on_action(cx.listener(|this, _: &UnhideMoveUp, _, cx| {
            this.change_unhide_selection(|selection| selection.step(-1), cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideMoveDown, _, cx| {
            this.change_unhide_selection(|selection| selection.step(1), cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideExtendUp, _, cx| {
            this.change_unhide_selection(|selection| selection.extend(-1), cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideExtendDown, _, cx| {
            this.change_unhide_selection(|selection| selection.extend(1), cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideFocusUp, _, cx| {
            this.change_unhide_selection(|selection| selection.move_focus(-1), cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideFocusDown, _, cx| {
            this.change_unhide_selection(|selection| selection.move_focus(1), cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideToggleRow, _, cx| {
            this.change_unhide_selection(Selection::toggle_focused, cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideSelectAll, _, cx| {
            this.change_unhide_selection(Selection::select_all, cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideFirstRow, _, cx| {
            this.change_unhide_selection(Selection::first, cx)
        }))
        .on_action(cx.listener(|this, _: &UnhideLastRow, _, cx| {
            this.change_unhide_selection(Selection::last, cx)
        }))
        .on_action(
            cx.listener(|this, _: &ConfirmUnhide, window, cx| this.confirm_unhide(window, cx)),
        )
        .on_action(cx.listener(|this, _: &CancelUnhide, window, cx| this.close_unhide(window, cx)))
    }

    fn render_unhide_rows(
        &self,
        dialog: &UnhideDialog,
        window: &Window,
        cx: &Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let Some(document) = self.documents.active() else {
            return Vec::new();
        };
        let theme = cx.theme();
        let list_focused = dialog.list_focus.is_focused(window);
        dialog
            .sheets
            .iter()
            .enumerate()
            .map(|(row, id)| {
                let name = document
                    .sheets
                    .iter()
                    .find(|sheet| sheet.id == *id)
                    .map(|sheet| sheet.name.clone())
                    .unwrap_or_default();
                let selected = dialog.selection.is_selected(row);
                let focused = list_focused && dialog.selection.focus() == row;
                h_flex()
                    .id(("unhide-row", row))
                    .role(Role::ListBoxOption)
                    .aria_label(name.clone())
                    .aria_selected(selected)
                    .h(rem_px(ROW_HEIGHT))
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .when(selected, |el| el.bg(theme.list_active))
                    .when(focused, |el| el.shadow(inset_focus_ring(theme)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.click_unhide_row(row, event, window, cx)
                    }))
                    .child(
                        div()
                            .w(rem_px(16.0))
                            .child(if selected { CHECK } else { "" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .child(name),
                    )
            })
            .collect()
    }

    pub(super) fn render_unhide(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let dialog = self.unhide.as_ref()?;
        let rows = self.render_unhide_rows(dialog, window, cx);
        let theme = cx.theme();
        let unit = f32::from(rem_px(1.0).to_pixels(window.rem_size()));
        let room = f32::from(window.viewport_size().height) / unit
            - DIALOG_TOP
            - DIALOG_BOTTOM_MARGIN
            - CHROME_HEIGHT;
        let shown = rows_shown(rows.len(), room);
        let chosen = dialog.selection.count();
        let total = dialog.selection.rows();
        let count = if chosen == 1 {
            t!("unhide.count_one", n = chosen, m = total)
        } else {
            t!("unhide.count_other", n = chosen, m = total)
        };
        let list = v_flex()
            .id("unhide-list")
            .key_context("UnhideList")
            .track_focus(&dialog.list_focus)
            .role(Role::ListBox)
            .aria_label(t!("unhide.label"))
            .track_scroll(&dialog.scroll)
            .overflow_y_scroll()
            .h(rem_px(ROW_HEIGHT * shown as f32 + 2.0))
            .flex_none()
            .border_1()
            .border_color(theme.muted_foreground)
            .rounded_md()
            .children(rows)
            .child(scroll_thumb::overlay(
                &dialog.scroll,
                "unhide-scrollbar",
                theme.muted_foreground,
            ));
        let footer = h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(count),
            )
            .child(dialog_button(
                "unhide-cancel",
                t!("button.cancel"),
                Some(&dialog.cancel_focus),
                false,
                Box::new(CancelUnhide),
                window,
                theme,
            ))
            .child(dialog_button(
                "unhide-ok",
                t!("button.ok"),
                (chosen > 0).then_some(&dialog.ok_focus),
                true,
                Box::new(ConfirmUnhide),
                window,
                theme,
            ));
        let panel = v_flex()
            .id("unhide-dialog")
            .key_context("UnhideDialog")
            .track_focus(&dialog.dialog_focus)
            .role(Role::Dialog)
            .aria_label(t!("unhide.title"))
            .occlude()
            .on_action(cx.listener(|this, _: &FocusNextControl, window, cx| {
                this.cycle_unhide_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPreviousControl, window, cx| {
                this.cycle_unhide_focus(false, window, cx)
            }))
            .w(rem_px(DIALOG_WIDTH))
            .p_4()
            .gap_3()
            .bg(theme.background)
            .text_color(theme.foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("unhide.title")),
            )
            .child(div().text_sm().child(t!("unhide.label")))
            .child(list)
            .child(footer)
            .child(
                div()
                    .h(rem_px(32.0))
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("unhide.hint")),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.dispatch_action(Box::new(CancelUnhide), cx)
                })
                .on_mouse_down(MouseButton::Right, |_, window, cx| {
                    window.dispatch_action(Box::new(CancelUnhide), cx)
                })
                .child(
                    div()
                        .absolute()
                        .top(rem_px(DIALOG_TOP))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(panel),
                ),
        )
    }
}
