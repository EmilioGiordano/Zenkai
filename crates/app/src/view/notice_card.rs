use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::notices::{Notice, Severity};
use crate::actions::{CopyNotice, DismissNotice, ShowMessages};
use crate::interface_scale::rem_px;
use crate::keymap;

const CARD_WIDTH: f32 = 420.0;
pub(super) const STATUS_BAR_HEIGHT: f32 = 26.0;
pub(super) const EDGE_GAP: f32 = 12.0;
const TEXT_MAX_HEIGHT: f32 = 168.0;
const LONG_TEXT_CHARS: usize = 200;
const LONG_TEXT_LINES: usize = 4;

// The text box scrolls, so a long message is never lost; the button makes sure nobody has
// to guess that there is more.
fn needs_show_all(notice: &Notice) -> bool {
    notice.detail().is_some()
        || notice.text.chars().count() > LONG_TEXT_CHARS
        || notice.text.lines().count() > LONG_TEXT_LINES
}

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
        if self.notices.card().is_none() || self.messages.is_some() {
            cx.propagate();
            return;
        }
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
            cx.write_to_clipboard(ClipboardItem::new_string(card.copy_text().to_string()));
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
                        )
                        .when(needs_show_all(notice), |card| {
                            card.child(
                                h_flex().justify_end().child(
                                    Button::new("notice-show-all")
                                        .ghost()
                                        .small()
                                        .label(t!("notice.show_all"))
                                        .tooltip(keymap::labeled(
                                            cx,
                                            t!("notice.show_all"),
                                            &ShowMessages,
                                        ))
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(ShowMessages), cx)
                                        }),
                                ),
                            )
                        }),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::notices::{Lifetime, Notices, Severity};
    use super::needs_show_all;

    fn needs(text: &str, detail: Option<&str>) -> bool {
        let mut notices = Notices::default();
        notices.post(
            Severity::Error,
            text.to_string().into(),
            detail.map(|detail| detail.to_string().into()),
            Lifetime::UntilReplaced,
            "12:00:00".into(),
        );
        notices.card().is_some_and(needs_show_all)
    }

    #[test]
    fn a_short_message_without_detail_needs_no_button() {
        assert!(!needs("Could not save a.xlsx.", None));
    }

    #[test]
    fn a_long_or_tall_message_or_one_with_detail_offers_show_all() {
        assert!(needs(&"x".repeat(201), None));
        assert!(needs(
            "a
b
c
d
e",
            None
        ));
        assert!(needs("Short.", Some("data and path")));
    }
}
