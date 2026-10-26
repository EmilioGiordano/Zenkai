use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, Theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::notices::{Expiry, Notice, Severity, WARNING_LIFETIME};
use crate::actions::{CopyNotice, DismissNotice, ShowMessages};
use crate::interface_scale::rem_px;
use crate::keymap;

const CARD_WIDTH: f32 = 420.0;
pub(super) const STATUS_BAR_HEIGHT: f32 = 26.0;
pub(super) const EDGE_GAP: f32 = 12.0;
const LONG_TEXT_CHARS: usize = 200;
const LONG_TEXT_LINES: usize = 4;

struct CardText {
    shown: String,
    clamped: bool,
}

// The card never scrolls or cuts a line at its edge: a long message is clamped at a word
// boundary with an ellipsis, and "Show all" opens the full text.
fn clamp_text(text: &str) -> CardText {
    let mut shown = String::new();
    let mut chars = 0;
    let mut clamped = false;
    for (index, line) in text.lines().enumerate() {
        if index == LONG_TEXT_LINES {
            clamped = true;
            break;
        }
        if index > 0 {
            shown.push('\n');
            chars += 1;
        }
        let room = LONG_TEXT_CHARS - chars;
        if line.chars().count() <= room {
            chars += line.chars().count();
            shown.push_str(line);
            continue;
        }
        let end = line
            .char_indices()
            .nth(room)
            .map_or(line.len(), |(end, _)| end);
        let cut = &line[..end];
        let kept = if line[end..].starts_with(char::is_whitespace) {
            cut
        } else {
            cut.rfind(char::is_whitespace)
                .map_or(cut, |space| &cut[..space])
        };
        shown.push_str(kept.trim_end());
        clamped = true;
        break;
    }
    if clamped {
        shown.push('\u{2026}');
    }
    CardText { shown, clamped }
}

fn needs_show_all(notice: &Notice) -> bool {
    notice.detail().is_some() || clamp_text(&notice.text).clamped
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
    pub(super) fn expire_warning_later(
        &mut self,
        severity: Severity,
        id: u64,
        cx: &mut Context<Self>,
    ) {
        if severity != Severity::Warning {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WARNING_LIFETIME).await;
                let expiry = this.update(cx, |this, cx| {
                    let held = this.notice_hovered || this.notice_focused.get();
                    let expiry = this.notices.expire_warning(id, held);
                    if expiry == Expiry::Closed {
                        this.notice_hovered = false;
                        cx.notify();
                    }
                    expiry
                });
                if !matches!(expiry, Ok(Expiry::Held)) {
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn dismiss_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.card().is_none() || self.messages.is_some() {
            cx.propagate();
            return;
        }
        let focused = self.notice_focus.contains_focused(window, cx);
        self.notices.dismiss_card();
        self.notice_hovered = false;
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
            cx.write_to_clipboard(ClipboardItem::new_string(card.copy_text()));
        }
    }

    pub(super) fn focus_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.card().is_some() {
            window.focus(&self.notice_focus, cx);
        }
    }

    pub(super) fn render_notice_card(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let notice = self.notices.card()?;
        if self.messages.is_some() {
            return None;
        }
        self.notice_focused
            .set(self.notice_focus.contains_focused(window, cx));
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
                        .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                            this.notice_hovered = *hovered;
                        }))
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
                                .text_sm()
                                .child(clamp_text(&notice.text).shown),
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
    use super::{clamp_text, needs_show_all};

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

    #[test]
    fn a_short_text_is_shown_whole() {
        let text = clamp_text("Could not open a.xlsx: bad.\nSecond line.");
        assert_eq!(text.shown, "Could not open a.xlsx: bad.\nSecond line.");
        assert!(!text.clamped);
    }

    #[test]
    fn a_long_text_ends_at_a_word_with_an_ellipsis() {
        let text = clamp_text(&"word ".repeat(60));
        assert!(text.clamped);
        assert!(text.shown.ends_with("word\u{2026}"));
        assert!(text.shown.chars().count() <= 201);
    }

    #[test]
    fn a_word_longer_than_the_limit_is_cut_hard() {
        let text = clamp_text(&"x".repeat(300));
        assert_eq!(text.shown, format!("{}\u{2026}", "x".repeat(200)));
    }

    #[test]
    fn extra_lines_are_dropped_with_an_ellipsis() {
        let text = clamp_text("a\nb\nc\nd\ne");
        assert_eq!(text.shown, "a\nb\nc\nd\u{2026}");
        assert!(text.clamped);
    }

    #[test]
    fn a_text_of_exactly_the_limits_is_not_clamped() {
        assert!(!clamp_text(&"x".repeat(200)).clamped);
        assert!(!clamp_text("a\nb\nc\nd").clamped);
    }
}
