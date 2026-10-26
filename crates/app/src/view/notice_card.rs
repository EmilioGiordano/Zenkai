use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Theme};
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::notices::Severity;
use crate::actions::{CopyNotice, DismissNotice};
use crate::interface_scale::rem_px;
use crate::keymap;

const CARD_WIDTH: f32 = 420.0;
const STATUS_BAR_HEIGHT: f32 = 26.0;
const EDGE_GAP: f32 = 12.0;
const TEXT_MAX_HEIGHT: f32 = 168.0;

pub(super) fn severity_title(severity: Severity) -> SharedString {
    match severity {
        Severity::Info => t!("notice.title.info").into(),
        Severity::Warning => t!("notice.title.warning").into(),
        Severity::Error => t!("notice.title.error").into(),
    }
}

pub(super) fn severity_icon(severity: Severity, theme: &Theme) -> (IconName, Hsla) {
    match severity {
        Severity::Info => (IconName::Info, theme.muted_foreground),
        Severity::Warning => (IconName::TriangleAlert, theme.warning),
        Severity::Error => (IconName::CircleX, theme.danger),
    }
}

impl Workspace {
    pub(super) fn dismiss_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.notice_focus.contains_focused(window, cx);
        self.notices.dismiss_card();
        if focused {
            window.focus(&self.grid.focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(super) fn copy_notice(&mut self, cx: &mut Context<Self>) {
        if self.messages.is_some() {
            self.copy_selected_message(cx);
            return;
        }
        if let Some(card) = self.notices.card() {
            cx.write_to_clipboard(ClipboardItem::new_string(card.text.to_string()));
        }
    }

    pub(super) fn focus_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.card().is_some() {
            window.focus(&self.notice_focus, cx);
        }
    }

    pub(super) fn render_notice_card(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let notice = self.notices.card()?;
        if self.messages.is_some() {
            return None;
        }
        let theme = cx.theme();
        let (icon, color) = severity_icon(notice.severity, theme);
        let title = severity_title(notice.severity);
        Some(
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
                        .id("notice-card")
                        .key_context("NoticeCard")
                        .track_focus(&self.notice_focus)
                        .role(Role::Alert)
                        .aria_label(format!("{title}: {}", notice.text))
                        .w_full()
                        .max_w(rem_px(CARD_WIDTH))
                        .gap_2()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .text_color(theme.popover_foreground)
                        .shadow_lg()
                        .focus_visible(|card| card.border_color(theme.ring))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(icon).text_color(color))
                                .child(
                                    div()
                                        .flex_1()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(title),
                                )
                                .child(
                                    Button::new("notice-copy")
                                        .ghost()
                                        .small()
                                        .icon(IconName::Copy)
                                        .label(t!("notice.copy"))
                                        .tooltip(keymap::labeled(
                                            cx,
                                            t!("notice.copy"),
                                            &CopyNotice,
                                        ))
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(CopyNotice), cx)
                                        }),
                                )
                                .child(
                                    Button::new("notice-close")
                                        .ghost()
                                        .small()
                                        .icon(IconName::Close)
                                        .accessibility_label(t!("notice.close"))
                                        .tooltip(keymap::labeled(
                                            cx,
                                            t!("notice.close"),
                                            &DismissNotice,
                                        ))
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(DismissNotice), cx)
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .id("notice-card-text")
                                .max_h(rem_px(TEXT_MAX_HEIGHT))
                                .overflow_y_scroll()
                                .text_sm()
                                .child(notice.text.clone()),
                        ),
                ),
        )
    }
}
