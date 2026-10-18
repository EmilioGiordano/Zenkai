use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::session::ChoiceKind;
use zenkai_agent::chat::thread::ToolCard;
use zenkai_agent::chat::tool_rows;
use zenkai_agent::tools::escaped_text;
use zenkai_i18n::t;

use super::tools::ToolViews;
use super::{ChatPanel, MONO, View};
use crate::actions::{
    AllowAgentChange, AllowChatPermission, DenyAgentChange, DenyChatPermission, ShowAgentChange,
};
use crate::keymap;

pub(crate) struct AgentChange {
    pub caption: Option<String>,
    pub summary: String,
    pub consequence: String,
    pub sample: Option<String>,
    pub offer_show: bool,
    pub focus: FocusHandle,
}

struct Disclosure {
    button: Button,
    open: bool,
    body: Option<String>,
}

struct Card {
    caption: Option<String>,
    summary: String,
    provenance: Option<&'static str>,
    consequence: Option<String>,
    disclosure: Option<Disclosure>,
    answers: Vec<Button>,
    standing: Vec<Button>,
}

fn render_card(card: Card, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    v_flex()
        .gap_2()
        .p_3()
        .rounded_lg()
        .bg(theme.background)
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    Icon::new(IconName::Shield)
                        .size_4()
                        .flex_shrink_0()
                        .mt_0p5()
                        .text_color(theme.success),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .when_some(card.caption, |column, caption| {
                            column.child(div().text_xs().text_color(muted).child(caption))
                        })
                        .child(div().font_weight(FontWeight::MEDIUM).child(card.summary))
                        .when_some(card.provenance, |column, note| {
                            column.child(div().text_xs().text_color(muted).child(note))
                        }),
                ),
        )
        .when_some(card.consequence, |column, consequence| {
            column.child(
                div()
                    .text_color(muted)
                    .line_height(relative(1.45))
                    .child(consequence),
            )
        })
        .when_some(card.disclosure, |column, disclosure| {
            column
                .child(h_flex().child(disclosure.button))
                .when(disclosure.open, |column| {
                    column.when_some(disclosure.body, |column, body| {
                        column.child(
                            div()
                                .font_family(MONO)
                                .text_xs()
                                .line_height(relative(1.5))
                                .p_2()
                                .rounded_md()
                                .bg(theme.muted)
                                .child(body),
                        )
                    })
                })
        })
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .flex_wrap()
                .children(card.answers)
                .child(div().flex_1())
                .children(card.standing),
        )
}

impl ChatPanel {
    pub(crate) fn set_agent_change(&mut self, change: Option<AgentChange>, cx: &mut Context<Self>) {
        if change.is_none() {
            self.change_cells_open = false;
        }
        let arrived = change.is_some() && self.agent_change.is_none();
        self.agent_change = change;
        if arrived {
            self.scroll_to_end();
        }
        cx.notify();
    }

    pub(crate) fn shows_agent_change(&self) -> bool {
        self.view == View::Chat && self.agent_change.is_some()
    }

    pub(super) fn render_agent_change(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let change = self
            .agent_change
            .as_ref()
            .filter(|_| self.view == View::Chat)?;
        let open = self.change_cells_open;
        let disclosure = change.sample.as_ref().map(|sample| Disclosure {
            button: Button::new("agent-change-cells")
                .ghost()
                .small()
                .label(if open {
                    t!("chat.approval.hide_sample")
                } else {
                    t!("chat.approval.show_sample")
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.change_cells_open = !this.change_cells_open;
                    cx.notify();
                })),
            open,
            body: Some(sample.clone()),
        });
        let mut answers = vec![
            Button::new("agent-change-allow")
                .primary()
                .small()
                .label(t!("approval.allow"))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(AllowAgentChange), cx)),
            Button::new("agent-change-deny")
                .secondary()
                .small()
                .label(t!("approval.deny"))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(DenyAgentChange), cx)),
        ];
        if change.offer_show {
            answers.push(
                Button::new("agent-change-show")
                    .ghost()
                    .small()
                    .label(t!("approval.show"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ShowAgentChange), cx)
                    }),
            );
        }
        let card = Card {
            caption: change.caption.clone(),
            summary: change.summary.clone(),
            provenance: None,
            consequence: Some(change.consequence.clone()),
            disclosure,
            answers,
            standing: Vec::new(),
        };
        Some(
            render_card(card, cx)
                .id("agent-change")
                .key_context("AgentApproval")
                .track_focus(&change.focus),
        )
    }

    pub(super) fn render_ask(
        &self,
        views: &ToolViews,
        card: &ToolCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = views.input_open(&card.id);
        let disclosure = (!card.input.is_empty()).then(|| {
            let toggled = card.id.clone();
            let input = tool_rows::body_preview(&tool_rows::pretty_input(&card.input));
            Disclosure {
                button: Button::new(SharedString::from(format!(
                    "ask-input-{}",
                    card.id.as_str()
                )))
                .ghost()
                .small()
                .label(if open {
                    t!("chat.approval.hide_input")
                } else {
                    t!("chat.approval.show_input")
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tools.toggle_input(&toggled);
                    cx.notify();
                })),
                open,
                body: Some(if input.more_chars > 0 {
                    format!(
                        "{}
{}",
                        input.shown,
                        t!("chat.tool.more_chars", count = input.more_chars)
                    )
                } else {
                    input.shown
                }),
            }
        });
        let mut answers = Vec::new();
        let mut standing = Vec::new();
        if let Some(ask) = self.permissions.iter().find(|ask| ask.tool == card.id) {
            for (index, choice) in ask.choices.iter().enumerate() {
                let tool = card.id.clone();
                let button = Button::new(SharedString::from(format!(
                    "permission-{}-{index}",
                    card.id.as_str()
                )))
                .small()
                .on_click(cx.listener(move |this, _, _, cx| this.answer(&tool, index, cx)));
                match choice.kind {
                    ChoiceKind::AllowOnce => answers.push(button.primary().label(keymap::labeled(
                        cx,
                        t!("chat.approval.allow"),
                        &AllowChatPermission,
                    ))),
                    ChoiceKind::RejectOnce => answers.push(button.secondary().label(
                        keymap::labeled(cx, t!("chat.approval.deny"), &DenyChatPermission),
                    )),
                    ChoiceKind::AllowAlways | ChoiceKind::RejectAlways => {
                        standing.push(button.ghost().label(escaped_text(&choice.label)))
                    }
                }
            }
        }
        render_card(
            Card {
                caption: None,
                summary: tool_rows::approval_summary(card),
                provenance: Some(t!("chat.approval.from_agent")),
                consequence: None,
                disclosure,
                answers,
                standing,
            },
            cx,
        )
        .into_any_element()
    }
}
