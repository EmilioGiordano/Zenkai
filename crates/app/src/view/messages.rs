use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::notice_card::{severity_icon, severity_title};
use crate::actions::CloseMessages;
use crate::interface_scale::rem_px;

const PANEL_WIDTH: f32 = 480.0;
const PANEL_MAX_HEIGHT: f32 = 420.0;
const STATUS_BAR_HEIGHT: f32 = 26.0;
const EDGE_GAP: f32 = 12.0;
const TIME_WIDTH: f32 = 60.0;

pub(super) struct MessagesPanel {
    focus: FocusHandle,
    scroll: ScrollHandle,
    selected: Option<u64>,
}

impl Workspace {
    pub(super) fn toggle_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.messages.is_some() {
            self.close_messages(window, cx);
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.messages = Some(MessagesPanel {
            focus,
            scroll: ScrollHandle::new(),
            selected: None,
        });
        cx.notify();
    }

    pub(super) fn close_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.messages.take().is_none() {
            return;
        }
        window.focus(&self.grid.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn step_message(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.messages else {
            return;
        };
        let current = self.notices.index_of(panel.selected);
        let next = if forward {
            (current + 1).min(self.notices.len().saturating_sub(1))
        } else {
            current.saturating_sub(1)
        };
        panel.selected = self.notices.id_at(next);
        panel.scroll.scroll_to_item(next);
        cx.notify();
    }

    fn select_message(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.messages {
            panel.selected = Some(id);
            cx.notify();
        }
    }

    fn copy_message(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(notice) = self.notices.newest_first().find(|notice| notice.id() == id) {
            cx.write_to_clipboard(ClipboardItem::new_string(notice.text.to_string()));
        }
    }

    pub(super) fn copy_selected_message(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &self.messages else {
            return;
        };
        if let Some(id) = self.notices.id_at(self.notices.index_of(panel.selected)) {
            self.copy_message(id, cx);
        }
    }

    pub(super) fn render_messages(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let panel = self.messages.as_ref()?;
        let theme = cx.theme();
        let chosen = self.notices.index_of(panel.selected);
        let rows: Vec<_> = self
            .notices
            .newest_first()
            .enumerate()
            .map(|(index, notice)| {
                let (icon, color) = severity_icon(notice.severity, theme);
                let selected = index == chosen;
                let id = notice.id();
                h_flex()
                    .id(("message", index))
                    .role(Role::ListBoxOption)
                    .aria_label(format!(
                        "{} {}: {}",
                        notice.time,
                        severity_title(notice.severity),
                        notice.text
                    ))
                    .aria_selected(selected)
                    .gap_2()
                    .items_start()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected {
                        theme.ring
                    } else {
                        theme.transparent
                    })
                    .when(selected, |row| row.bg(theme.accent))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_message(id, cx)))
                    .child(
                        div()
                            .w(rem_px(TIME_WIDTH))
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(notice.time.clone()),
                    )
                    .child(Icon::new(icon).text_color(color).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(notice.text.clone()),
                    )
                    .child(
                        Button::new(("message-copy", index))
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .accessibility_label(t!("notice.copy"))
                            .on_click(cx.listener(move |this, _, _, cx| this.copy_message(id, cx))),
                    )
            })
            .collect();
        let list = if rows.is_empty() {
            div()
                .p_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t!("messages.empty"))
                .into_any_element()
        } else {
            v_flex()
                .id("messages-list")
                .role(Role::ListBox)
                .track_scroll(&panel.scroll)
                .overflow_y_scroll()
                .min_h_0()
                .gap_1()
                .children(rows)
                .into_any_element()
        };
        Some(
            div()
                .key_context("MessagesPanel")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.dispatch_action(Box::new(CloseMessages), cx)
                })
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(rem_px(STATUS_BAR_HEIGHT + EDGE_GAP))
                        .px(rem_px(EDGE_GAP))
                        .flex()
                        .justify_end()
                        .child(
                            v_flex()
                                .id("messages-panel")
                                .track_focus(&panel.focus)
                                .role(Role::Dialog)
                                .aria_label(t!("messages.title"))
                                .w_full()
                                .max_w(rem_px(PANEL_WIDTH))
                                .max_h(rem_px(PANEL_MAX_HEIGHT))
                                .gap_2()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.popover)
                                .text_color(theme.popover_foreground)
                                .shadow_lg()
                                .focus_visible(|panel| panel.border_color(theme.ring))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .flex_1()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(t!("messages.title")),
                                        )
                                        .child(
                                            Button::new("messages-close")
                                                .ghost()
                                                .small()
                                                .icon(IconName::Close)
                                                .accessibility_label(t!("gen.close"))
                                                .on_click(|_, window, cx| {
                                                    window.dispatch_action(
                                                        Box::new(CloseMessages),
                                                        cx,
                                                    )
                                                }),
                                        ),
                                )
                                .child(list),
                        ),
                ),
        )
    }
}
