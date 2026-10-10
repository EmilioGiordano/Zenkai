use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::state::AgentState;
use zenkai_agent::presets::PRESETS;
use zenkai_agent::settings::{AgentId, AgentServer};
use zenkai_i18n::t;

use super::extras::now_seconds;
use super::header::{agent_mark, session_mark};
use super::menus::Group;
use super::session_rows::{self, Row};
use super::{ChatPanel, View, closed, launch};
use crate::actions::*;
use crate::agent_settings::AgentConfig;
use crate::keymap;

fn agent_name(servers: &BTreeMap<AgentId, AgentServer>, id: &AgentId) -> String {
    if let Some(server) = servers.get(id) {
        return server.name.clone();
    }
    if let Some(preset) = PRESETS.iter().find(|preset| preset.id == id.as_str()) {
        return preset.name.to_string();
    }
    id.as_str().to_string()
}

impl ChatPanel {
    fn all_rows(&self, cx: &App) -> Vec<Row> {
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let now = now_seconds();
        let current = launch::chosen_agent(&settings).map(|(id, _)| id);
        let mut ids: BTreeSet<AgentId> = self
            .history
            .records()
            .iter()
            .map(|record| record.agent.clone())
            .collect();
        if let Some(id) = current.clone() {
            ids.insert(id);
        }
        let mut all = Vec::new();
        for id in ids {
            let state = if current.as_ref() == Some(&id) {
                &self.state
            } else {
                &AgentState::default()
            };
            all.extend(session_rows::rows(
                &id,
                &agent_name(&settings.agents.servers, &id),
                state,
                &self.history,
                &self.session_sizes,
                now,
            ));
        }
        all
    }

    fn filtered_rows(&self, cx: &App) -> Vec<Row> {
        let query = self.sessions_query.read(cx).value().to_string();
        session_rows::filter(self.all_rows(cx), &query, &self.hidden_agents)
    }

    fn toggle_agent(&mut self, id: AgentId, cx: &mut Context<Self>) {
        if self.hidden_agents.contains(&id) {
            self.hidden_agents.remove(&id);
        } else {
            self.hidden_agents.insert(id);
        }
        cx.notify();
    }

    fn show_all_agents(&mut self, cx: &mut Context<Self>) {
        self.hidden_agents.clear();
        cx.notify();
    }

    // Recomputes every record's size off the UI thread; render only reads the map.
    pub(super) fn refresh_session_sizes(&self, cx: &mut Context<Self>) {
        let history = self.history.clone();
        cx.spawn(async move |this, cx| {
            let sizes = cx
                .background_executor()
                .spawn(async move { session_rows::sizes_for(history.records()) })
                .await;
            closed(this.update(cx, |this, cx| {
                this.session_sizes = sizes;
                cx.notify();
            }));
        })
        .detach();
    }

    pub(crate) fn sessions_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.view != View::Sessions {
            return;
        }
        let count = self.filtered_rows(cx).len();
        self.menu_index = super::model_search::step(self.menu_index, count, forward);
        cx.notify();
    }

    pub(crate) fn sessions_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Sessions {
            return;
        }
        let rows = self.filtered_rows(cx);
        if rows.is_empty() {
            return;
        }
        let picked = rows
            .get(self.menu_index.min(rows.len() - 1))
            .expect("nonempty rows")
            .clone();
        self.resume(&picked, window, cx);
    }

    pub(crate) fn sessions_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view != View::Sessions {
            return;
        }
        if self.sessions_query.read(cx).value().is_empty() {
            self.toggle_sessions(window, cx);
        } else {
            self.sessions_query.update(cx, |query, cx| {
                query.set_value("", window, cx);
            });
        }
    }

    fn session_row(&self, row: &Row, highlighted: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let picked = row.clone();
        let content = h_flex()
            .gap_2p5()
            .items_center()
            .w_full()
            .child(session_mark(&row.agent, &row.agent_name, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .w_full()
                            .child(div().flex_1().truncate().child(row.title.clone()))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(row.active.clone()),
                            ),
                    )
                    .child({
                        let mut meta = h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .text_color(muted)
                            .child(row.agent_name.clone());
                        if let Some(size) = row.size_bytes {
                            meta = meta.child(session_rows::format_size(size));
                        }
                        if let Some(usage) = row.context {
                            let fraction = usage.fraction().unwrap_or(0.0).clamp(0.0, 1.0);
                            meta = meta
                                .child(
                                    div()
                                        .w(px(46.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(theme.muted)
                                        .flex()
                                        .items_center()
                                        .child(
                                            div()
                                                .w(px(46.0 * fraction))
                                                .h_full()
                                                .rounded_full()
                                                .bg(
                                                    if session_rows::context_over_threshold(&usage)
                                                    {
                                                        theme.warning
                                                    } else {
                                                        theme.success
                                                    },
                                                ),
                                        ),
                                )
                                .child(format!(
                                    "{} / {}",
                                    session_rows::format_tokens(usage.used),
                                    session_rows::format_tokens(usage.size)
                                ));
                        }
                        div().truncate().child(meta)
                    }),
            );
        Button::new(SharedString::from(format!(
            "session-{}-{}",
            row.agent.as_str(),
            row.title
        )))
        .ghost()
        .w_full()
        .h_auto()
        .when(highlighted, |button| button.bg(theme.accent))
        .child(content)
        .on_click(cx.listener(move |this, _, window, cx| this.resume(&picked, window, cx)))
        .into_any_element()
    }

    pub(super) fn render_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let query = self.sessions_query.clone();
        let rows = self.filtered_rows(cx);
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let names: Vec<(AgentId, String)> = session_rows::agent_ids(&self.all_rows(cx))
            .into_iter()
            .map(|id| {
                let name = agent_name(&settings.agents.servers, &id);
                (id, name)
            })
            .collect();
        let total = rows.iter().map(|row| row.size_bytes).sum();
        let summary = session_rows::summary(rows.len(), total);
        let mut list = v_flex().gap_1().px_2().pb_2();
        let mut group: Option<Group> = None;
        for (index, row) in rows.iter().enumerate() {
            if group != Some(row.group) {
                group = Some(row.group);
                list = list.child(
                    div()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child(match row.group {
                            Group::Today => t!("chat.sessions.today"),
                            Group::ThisWeek => t!("chat.sessions.this_week"),
                            Group::Earlier => t!("chat.sessions.earlier"),
                        }),
                );
            }
            list = list.child(self.session_row(row, index == self.menu_index, cx));
        }
        if rows.is_empty() {
            list = list.child(div().px_2().py_4().text_sm().text_color(muted).child(
                if self.hidden_agents.is_empty() && query.read(cx).value().is_empty() {
                    t!("chat.sessions.empty")
                } else {
                    t!("chat.sessions.no_match")
                },
            ));
        }
        let mut chips = h_flex().gap_1().px_2().flex_wrap();
        for (id, name) in &names {
            let on = !self.hidden_agents.contains(id);
            let id = id.clone();
            chips = chips.child(
                Button::new(SharedString::from(format!("session-chip-{}", id.as_str())))
                    .ghost()
                    .compact()
                    .toggled(on)
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(agent_mark(&id, name, cx))
                            .child(name.clone())
                            .when(on, |row| row.child(Icon::new(IconName::Check).size_3p5())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_agent(id.clone(), cx))),
            );
        }
        chips = chips.child(
            Button::new("session-chips-all")
                .ghost()
                .compact()
                .label(t!("chat.sessions.all"))
                .on_click(cx.listener(|this, _, _, cx| this.show_all_agents(cx))),
        );
        v_flex()
            .key_context("ChatSessions")
            .gap_1()
            .pt_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .child(
                        Button::new("sessions-back")
                            .ghost()
                            .compact()
                            .icon(IconName::ChevronLeft)
                            .tooltip(keymap::labeled(
                                cx,
                                t!("chat.sessions.back"),
                                &ShowChatSessions,
                            ))
                            .on_click(cx.listener(|this, window, _, cx| {
                                this.toggle_sessions(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(muted).child(summary)),
            )
            .child(
                div().px_2().child(
                    Input::new(&query)
                        .prefix(Icon::new(IconName::Search).size_4())
                        .placeholder(t!("chat.sessions.search")),
                ),
            )
            .child(chips)
            .child(list)
            .into_any_element()
    }
}
