use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::{ChatPanel, View};
use crate::actions::*;
use crate::agent_mark::{HEADER_MARK_SIZE, agent_mark};
use crate::chat_layout::ChatLayout;
use crate::interface_scale::rem_px;
use crate::keymap;

fn token_count(tokens: u64) -> String {
    if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

impl ChatPanel {
    pub(crate) fn show_workspace_view(
        &mut self,
        layout: ChatLayout,
        selected_range: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.layout != layout || self.selected_range != selected_range {
            self.layout = layout;
            self.selected_range = selected_range;
            cx.notify();
        }
    }

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
        let layout = self.layout;
        let range = self
            .selected_range
            .clone()
            .filter(|_| layout == ChatLayout::FullWindow);
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
                        row.child(agent_mark(id, &server.name, HEADER_MARK_SIZE, cx))
                    })
                    .child(
                        div()
                            .max_w(rem_px(170.0))
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
            .h(rem_px(48.0))
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
            .when_some(range, |row, range| {
                row.child(
                    div()
                        .id("chat-selected-range")
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_family(super::MONO)
                        .child(t!("chat.full_window.selection", range = range)),
                )
            })
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
                            div().size(rem_px(18.0)).child(
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
            .child(
                Button::new("chat-layout")
                    .ghost()
                    .compact()
                    .icon(IconName::LayoutPanelLeft)
                    .tooltip(keymap::labeled(
                        cx,
                        t!("chat.layout.tooltip"),
                        &CycleChatLayout,
                    ))
                    .dropdown_menu(move |menu, _, _| {
                        menu.menu_with_check(
                            t!("chat.layout.docked"),
                            layout == ChatLayout::Docked,
                            Box::new(DockChat),
                        )
                        .menu_with_check(
                            t!("chat.layout.side_by_side"),
                            layout == ChatLayout::SideBySide,
                            Box::new(SplitChatBesideSheet),
                        )
                        .menu_with_check(
                            t!("chat.layout.full_window"),
                            layout == ChatLayout::FullWindow,
                            Box::new(FillWindowWithChat),
                        )
                        .separator()
                        .menu(t!("chat.layout.cycle"), Box::new(CycleChatLayout))
                    }),
            )
            .child(
                Button::new("chat-full-window")
                    .ghost()
                    .compact()
                    .icon(if layout == ChatLayout::FullWindow {
                        IconName::Minimize
                    } else {
                        IconName::Maximize
                    })
                    .selected(layout == ChatLayout::FullWindow)
                    .tooltip(keymap::labeled(
                        cx,
                        if layout == ChatLayout::FullWindow {
                            t!("chat.full_window.leave")
                        } else {
                            t!("chat.full_window.enter")
                        },
                        &ToggleChatFullWindow,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ToggleChatFullWindow), cx)
                    }),
            )
    }
}
