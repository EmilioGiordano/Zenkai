use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::settings::AgentId;
use zenkai_i18n::t;

use super::{ChatPanel, View};
use crate::actions::*;
use crate::assets::OPENCODE_MARK;
use crate::keymap;

pub(super) fn agent_mark(agent: &AgentId, name: &str, cx: &App) -> impl IntoElement {
    mark_sized(agent, name, 20.0, 12.0, cx)
}

pub(super) fn session_mark(agent: &AgentId, name: &str, cx: &App) -> impl IntoElement {
    mark_sized(agent, name, 28.0, 16.0, cx)
}

fn mark_sized(agent: &AgentId, name: &str, tile: f32, glyph: f32, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let tile = div()
        .flex_shrink_0()
        .size(px(tile))
        .rounded_md()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.secondary);
    match agent.as_str() {
        "opencode" => tile
            .child(
                svg()
                    .path(OPENCODE_MARK)
                    .size(px(glyph))
                    .text_color(theme.muted_foreground),
            )
            .into_any_element(),
        _ => tile
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.secondary_foreground)
            .child(name.chars().next().map(String::from).unwrap_or_default())
            .into_any_element(),
    }
}

fn token_count(tokens: u64) -> String {
    if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

impl ChatPanel {
    pub(super) fn render_header(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let workbook = self.active_workbook(cx).map(|(_, name)| name);
        let agent = super::launch::chosen_agent(
            &cx.global::<crate::agent_settings::AgentConfig>()
                .state
                .current,
        );
        let title = match self.view {
            View::Sessions => t!("chat.sessions.title").to_string(),
            View::Chat => self
                .title
                .clone()
                .unwrap_or_else(|| t!("chat.new_conversation").to_string()),
        };
        let usage = self.state.usage;
        let percent = usage.and_then(|usage| usage.fraction());
        let title_button = Button::new("chat-title")
            .ghost()
            .compact()
            .tooltip(keymap::labeled(
                cx,
                t!("chat.switch_conversation"),
                &ShowChatSessions,
            ))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowChatSessions), cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when_some(agent.as_ref(), |row, (id, server)| {
                        row.child(agent_mark(id, &server.name, cx))
                    })
                    .child(
                        div()
                            .max_w(px(170.0))
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size_3()
                            .text_color(theme.muted_foreground),
                    ),
            );
        h_flex()
            .h(px(48.0))
            .flex_shrink_0()
            .pl_3p5()
            .pr_3()
            .gap_2()
            .items_center()
            .text_color(theme.muted_foreground)
            .child(title_button)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .child(workbook.unwrap_or_default()),
            )
            .child(div().flex_1())
            .when_some(usage.zip(percent), |row, (usage, fraction)| {
                let percent = (fraction * 100.0).round();
                row.child(
                    h_flex()
                        .id("chat-context")
                        .gap_1p5()
                        .items_center()
                        .px_2()
                        .tooltip(move |window, cx| {
                            Tooltip::new(t!(
                                "chat.context.tooltip",
                                used = token_count(usage.used),
                                size = token_count(usage.size)
                            ))
                            .build(window, cx)
                        })
                        .child(
                            div().size(px(18.0)).child(
                                ProgressCircle::new("chat-context-ring")
                                    .value(percent)
                                    .accessibility_label(t!(
                                        "chat.context.label",
                                        percent = percent
                                    )),
                            ),
                        )
                        .child(
                            div()
                                .font_family(super::MONO)
                                .text_xs()
                                .child(format!("{percent}%")),
                        ),
                )
            })
            .child(
                Button::new("chat-sessions")
                    .ghost()
                    .compact()
                    .icon(IconName::Inbox)
                    .selected(self.view == View::Sessions)
                    .tooltip(keymap::labeled(
                        cx,
                        t!("chat.sessions.tooltip"),
                        &ShowChatSessions,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ShowChatSessions), cx)
                    }),
            )
            .child(
                Button::new("chat-new")
                    .ghost()
                    .compact()
                    .icon(IconName::Plus)
                    .tooltip(keymap::labeled(
                        cx,
                        t!("chat.new_conversation.tooltip"),
                        &NewAgentConversation,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(NewAgentConversation), cx)
                    }),
            )
    }
}
