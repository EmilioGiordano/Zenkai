use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use zenkai_agent::chat::launch::{LaunchPlan, LaunchSpec};

use super::{ChatPanel, Link, MONO};
use crate::actions::*;

impl ChatPanel {
    pub(super) fn render_gate(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let gate = self.gate.as_ref()?;
        let theme = cx.theme();
        let spec = LaunchSpec::of(&gate.prepared.server);
        Some(
            v_flex()
                .id("agent-launch-gate")
                .key_context("AgentLaunchGate")
                .track_focus(&gate.focus)
                .gap_2p5()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(theme.ring)
                .bg(theme.background)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Start this agent?"),
                )
                .child(div().text_sm().child(
                    "This command is not one of Zenkai's presets, or settings.json changed it. \
                     Check it before Zenkai runs it.",
                ))
                .child(
                    div()
                        .font_family(MONO)
                        .text_xs()
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .child(spec.to_string()),
                )
                .when(
                    matches!(gate.prepared.plan, LaunchPlan::Package { .. }),
                    |card| {
                        card.child(div().text_xs().text_color(theme.muted_foreground).child(
                            format!("Zenkai will run: {}", gate.prepared.plan.describe()),
                        ))
                    },
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("decline-launch")
                                .ghost()
                                .small()
                                .label("Don't start (Esc)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(DeclineAgentLaunch), cx)
                                }),
                        )
                        .child(
                            Button::new("confirm-launch")
                                .primary()
                                .small()
                                .label("Start agent (Alt+L)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ConfirmAgentLaunch), cx)
                                }),
                        ),
                ),
        )
    }

    pub(super) fn render_problem(&self, cx: &App) -> Option<impl IntoElement> {
        let problem = self.problem.as_ref()?;
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(
                            gpui_kit::component::Icon::new(IconName::TriangleAlert)
                                .size_4()
                                .flex_shrink_0()
                                .text_color(theme.danger),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .child(problem.text.clone()),
                        ),
                )
                .when_some(problem.login, |card, command| {
                    card.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .font_family(MONO)
                                    .text_xs()
                                    .p_2()
                                    .rounded_md()
                                    .bg(theme.muted)
                                    .child(command),
                            )
                            .child(
                                Button::new("copy-login")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label("Copy (Alt+C)")
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(CopyLoginCommand), cx)
                                    }),
                            ),
                    )
                }),
        )
    }

    pub(super) fn render_status(&self, cx: &App) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let text = match &self.link {
            Link::Preparing => "Preparing the agent…",
            Link::Installing => "Installing the agent (first run only, this can take a minute)…",
            Link::Starting => "Starting the agent…",
            Link::Ready(_) if self.busy() => "Working…",
            Link::Idle | Link::Confirming | Link::Ready(_) => return None,
        };
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(Spinner::new().color(theme.muted_foreground))
                .child(div().flex_1().min_w_0().child(text)),
        )
    }

    pub(super) fn render_empty(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_2()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Ask about the open workbook or ask for a change.")
            .child(
                "Changes appear in the grid for your approval first, and Ctrl+Z undoes them. \
                 Nothing is saved for you.",
            )
    }
}
