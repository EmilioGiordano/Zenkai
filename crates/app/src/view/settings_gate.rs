use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_agent::settings::HeldChange;

use super::{Severity, Workspace};
use crate::actions::{ApplyHeldSettings, KeepCurrentSettings};
use crate::agent_settings::AgentConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeldDecision {
    Apply,
    Keep,
}

impl Workspace {
    pub(super) fn on_settings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_bridge(cx);
        let state = cx.global::<AgentConfig>().state.clone();
        if state.held != self.shown_held {
            if state.held.is_some() && self.shown_held.is_none() {
                self.held_return_focus = self.take_focus_for_bar(&self.held_focus, window, cx);
            }
            self.shown_held = state.held.clone();
            cx.notify();
        }
        if state.problem == self.shown_settings_problem {
            return;
        }
        if let Some(problem) = &state.problem {
            self.notify(
                Severity::Warning,
                format!("{problem}. Zenkai keeps using the last valid settings."),
                cx,
            );
        }
        self.shown_settings_problem = state.problem;
    }

    pub(super) fn decide_held_settings(
        &mut self,
        decision: HeldDecision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.update_global::<AgentConfig, _>(|config, _| match decision {
            HeldDecision::Apply => config.state.accept_held(),
            HeldDecision::Keep => config.state.decline_held(),
        });
        let previous = self.held_return_focus.take();
        self.release_focus_from_bar(&self.held_focus.clone(), previous, window, cx);
        cx.notify();
    }

    pub(super) fn render_held_settings(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let HeldChange { escalations, .. } = self.shown_held.as_ref()?;
        let asks: Vec<&str> = escalations.iter().map(|e| e.label()).collect();
        let theme = cx.theme();
        Some(
            h_flex()
                .key_context("HeldSettings")
                .track_focus(&self.held_focus)
                .px_2()
                .py_1()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .child(div().text_color(theme.warning).child("⚠"))
                .child(div().flex_1().min_w_0().child(format!(
                    "settings.json was changed outside Zenkai's Settings page and asks to {}. \
                     Apply it only if you made this change.",
                    asks.join(" and ")
                )))
                .child(Button::new("held-apply").label("Apply (Alt+A)").on_click(
                    |_, window, cx| window.dispatch_action(Box::new(ApplyHeldSettings), cx),
                ))
                .child(
                    Button::new("held-keep")
                        .primary()
                        .label("Keep current (Enter)")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(KeepCurrentSettings), cx)
                        }),
                ),
        )
    }
}
