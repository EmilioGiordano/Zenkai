use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::Button;
use gpui_kit::component::text::{MarkdownExtensions, TextView};
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::*;
use zenkai_agent::chat::thread::{Entry, FileCard, NoticeKind, ToolCallId, ToolCard, worked_label};
use zenkai_agent::chat::tool_rows::{self, Item};
use zenkai_i18n::t;

use super::ChatPanel;
use super::badge::{self, ReferencePlugin};
use crate::agent_settings::AgentConfig;

fn user_bubble(text: &str, panel: &WeakEntity<ChatPanel>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div().flex().justify_end().child(
        div()
            .max_w(relative(0.88))
            .px_3()
            .py_2p5()
            .rounded_xl()
            .bg(theme.secondary)
            .text_color(theme.secondary_foreground)
            .line_height(relative(1.5))
            .child(match badge::user_message(text, panel, cx) {
                Some(message) => message,
                None => div().child(text.to_string()).into_any_element(),
            }),
    )
}

fn worked_divider(seconds: u64, cx: &App) -> impl IntoElement {
    let line = || div().flex_1().h(px(1.0)).bg(cx.theme().border);
    h_flex()
        .gap_2p5()
        .items_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(line())
        .child(worked_label(seconds))
        .child(line())
}

fn notice(kind: NoticeKind, text: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (icon, color) = match kind {
        NoticeKind::Info => (IconName::Info, theme.muted_foreground),
        NoticeKind::Error => (IconName::TriangleAlert, theme.danger),
    };
    h_flex()
        .gap_2()
        .items_start()
        .text_sm()
        .text_color(color)
        .child(Icon::new(icon).size_4().flex_shrink_0())
        .child(div().flex_1().min_w_0().child(text.to_string()))
}

impl ChatPanel {
    pub(super) fn render_entries(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let panel = cx.weak_entity();
        let display = cx.global::<AgentConfig>().state.current.agents.tool_calls;
        let mut views = std::mem::take(&mut self.tools);
        let entries = self.thread.entries();
        let live: HashSet<ToolCallId> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(card) => Some(card.id.clone()),
                _ => None,
            })
            .collect();
        views.keep_only(&live);
        let card_at = |index: usize| match &entries[index] {
            Entry::Tool(card) => Some(card),
            _ => None,
        };
        let rendered = tool_rows::plan(entries, display)
            .into_iter()
            .filter_map(|item| match item {
                Item::Call(index) => {
                    card_at(index).map(|card| self.render_call(&mut views, display, card, cx))
                }
                Item::Group(indices) => {
                    let cards: Vec<&ToolCard> = indices.into_iter().filter_map(card_at).collect();
                    Some(self.render_group(&mut views, &cards, cx))
                }
                Item::Used { count } => Some(self.render_used(count, cx)),
                Item::Entry(index) => match &entries[index] {
                    Entry::User(text) => Some(user_bubble(text, &panel, cx).into_any_element()),
                    Entry::Worked { seconds } => {
                        Some(worked_divider(*seconds, cx).into_any_element())
                    }
                    Entry::Notice { kind, text } => {
                        Some(notice(*kind, text, cx).into_any_element())
                    }
                    Entry::Assistant { id, text } => Some(match self.texts.get(id) {
                        Some(state) => TextView::new(state)
                            .markdown_extensions(
                                MarkdownExtensions::default()
                                    .parser_revision(1)
                                    .plugin(ReferencePlugin::new(panel.clone())),
                            )
                            .selectable(true)
                            .stream_fade(true)
                            .line_height(relative(1.6))
                            .into_any_element(),
                        None => div().child(text.clone()).into_any_element(),
                    }),
                    Entry::File(card) => {
                        Some(self.render_file_card(index, card, cx).into_any_element())
                    }
                    Entry::Tool(_) => None,
                },
            })
            .collect();
        self.tools = views;
        rendered
    }

    fn render_file_card(
        &self,
        index: usize,
        card: &FileCard,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let path = card.path.clone();
        h_flex()
            .gap_2p5()
            .items_center()
            .px_3()
            .py_2p5()
            .rounded_lg()
            .bg(theme.secondary)
            .child(
                Icon::new(IconName::Grid2x2)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(theme.success),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .truncate()
                            .child(t!("chat.file.created", name = card.name.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(card.place.clone()),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("open-created-{index}")))
                    .small()
                    .label(t!("chat.file.open"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_created(path.clone(), window, cx)
                    })),
            )
    }
}
