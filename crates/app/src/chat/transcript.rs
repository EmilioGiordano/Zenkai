use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{MarkdownExtensions, TextView};
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::session::ChoiceKind;
use zenkai_agent::chat::thread::{Entry, FileCard, NoticeKind, ToolCard, ToolStatus, worked_label};
use zenkai_i18n::t;

use super::badge::{self, ReferencePlugin};
use super::{ChatPanel, MONO};

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

fn status_mark(status: ToolStatus, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match status {
        ToolStatus::Pending | ToolStatus::InProgress => Spinner::new()
            .color(theme.muted_foreground)
            .into_any_element(),
        ToolStatus::WaitingForPermission => Icon::new(IconName::Info)
            .size_4()
            .text_color(theme.foreground)
            .into_any_element(),
        ToolStatus::Completed => Icon::new(IconName::CircleCheck)
            .size_4()
            .text_color(theme.muted_foreground)
            .into_any_element(),
        ToolStatus::Failed | ToolStatus::Rejected | ToolStatus::Canceled => {
            Icon::new(IconName::CircleX)
                .size_4()
                .text_color(theme.danger)
                .into_any_element()
        }
    }
}

impl ChatPanel {
    pub(super) fn render_entries(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let panel = cx.weak_entity();
        self.thread
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| match entry {
                Entry::User(text) => user_bubble(text, &panel, cx).into_any_element(),
                Entry::Worked { seconds } => worked_divider(*seconds, cx).into_any_element(),
                Entry::Notice { kind, text } => notice(*kind, text, cx).into_any_element(),
                Entry::Assistant { id, text } => match self.texts.get(id) {
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
                },
                Entry::Tool(card) => self.render_tool_card(card, cx).into_any_element(),
                Entry::File(card) => self.render_file_card(index, card, cx).into_any_element(),
            })
            .collect()
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

    fn render_tool_card(&self, card: &ToolCard, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let ask = self.permissions.iter().find(|ask| ask.tool == card.id);
        let buttons: Vec<Button> = ask
            .map(|ask| {
                ask.choices
                    .iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        let tool = card.id.clone();
                        let button = Button::new(SharedString::from(format!(
                            "permission-{}-{index}",
                            card.id.as_str()
                        )))
                        .small()
                        .w_full()
                        .label(choice.label.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.answer(&tool, index, cx);
                        }));
                        match choice.kind {
                            ChoiceKind::AllowOnce => button.primary(),
                            ChoiceKind::AllowAlways => button.outline(),
                            ChoiceKind::RejectOnce | ChoiceKind::RejectAlways => button.ghost(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        v_flex()
            .gap_2p5()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(status_mark(card.status, cx))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_xs()
                            .truncate()
                            .child(card.title.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(card.status.label()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("chat.tool.kind", kind = card.kind)),
            )
            .when(!card.input.is_empty(), |card_view| {
                card_view.child(
                    div()
                        .font_family(MONO)
                        .text_xs()
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .child(card.input.clone()),
                )
            })
            .when(!card.detail.is_empty(), |card_view| {
                card_view.child(
                    div()
                        .font_family(MONO)
                        .text_xs()
                        .line_height(relative(1.6))
                        .text_color(theme.muted_foreground)
                        .child(card.detail.clone()),
                )
            })
            .when(!buttons.is_empty(), |card_view| {
                card_view
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("chat.permission.options")),
                    )
                    .child(v_flex().gap_1().children(buttons))
            })
    }
}
