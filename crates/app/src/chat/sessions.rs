use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::extras::now_seconds;
use super::header::agent_mark;
use super::session_rows::{self, Group, Row, Source};
use super::{ChatPanel, launch};
use crate::agent_settings::AgentConfig;

impl ChatPanel {
    fn session_row(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let picked = row.clone();
        let content = h_flex()
            .gap_2p5()
            .items_center()
            .w_full()
            .child(agent_mark(&row.agent, &row.agent.to_string(), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().truncate().child(row.title.clone()))
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(row.meta.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(row.age.clone()),
            );
        Button::new(SharedString::from(format!("session-{index}")))
            .ghost()
            .w_full()
            .h_auto()
            .child(content)
            .on_click(cx.listener(move |this, _, window, cx| this.resume(&picked, window, cx)))
            .into_any_element()
    }

    pub(super) fn render_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let chosen = launch::chosen_agent(&settings);
        let query = self.sessions_query.read(cx).value().to_string();
        let rows = chosen.map_or_else(Vec::new, |(id, server)| {
            session_rows::rows(
                &id,
                &server.name,
                &self.state,
                &self.history,
                now_seconds(),
                &query,
            )
        });
        let items: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.session_row(index, row, cx))
            .collect();
        let theme = cx.theme();
        let local_only = !self.state.abilities.list_sessions;
        let mut list = v_flex().gap_0p5();
        let mut group: Option<Group> = None;
        for (row, item) in rows.iter().zip(items) {
            if group != Some(row.group) {
                group = Some(row.group);
                list = list.child(
                    div()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(row.group.title()),
                );
            }
            list = list.child(item);
        }
        let resumes_fresh = rows.iter().any(|row| row.source == Source::Local);
        v_flex()
            .id("chat-sessions-view")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap_1()
            .px_3()
            .pt_1p5()
            .pb_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_2p5()
                    .mb_2()
                    .child(
                        Icon::new(IconName::Search)
                            .size_3p5()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(&self.sessions_query)
                                .appearance(false)
                                .bordered(false),
                        ),
                    ),
            )
            .child(list)
            .when(rows.is_empty(), |view| {
                view.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No conversations yet."),
                )
            })
            .when(local_only && resumes_fresh, |view| {
                view.child(
                    div()
                        .p_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            "This agent cannot reopen earlier conversations. Picking one starts \
                             a new conversation with the same title.",
                        ),
                )
            })
            .into_any_element()
    }
}
