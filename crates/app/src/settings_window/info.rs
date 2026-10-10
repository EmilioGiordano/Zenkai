use std::path::PathBuf;

use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_agent::settings::HeldChange;
use zenkai_agent::settings_file::SettingsPaths;
use zenkai_i18n::t;

use crate::actions::{ApplyHeldSettings, KeepCurrentSettings};
use crate::agent_settings::{self, AgentConfig, BridgeStatus};
use crate::keymap::KeymapState;

pub fn held_banner(held: &HeldChange, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .gap_3()
        .px_4()
        .py_3()
        .items_center()
        .rounded_xl()
        .bg(theme.secondary)
        .child(div().text_color(theme.warning).child("⚠"))
        .child(div().flex_1().min_w_0().child(t!(
            "settings.held_banner",
            summary = agent_settings::held_summary(held)
        )))
        .child(
            Button::new("held-apply")
                .label(t!("held.apply"))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ApplyHeldSettings), cx)),
        )
        .child(
            Button::new("held-keep")
                .primary()
                .label(t!("settings.held_keep"))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(KeepCurrentSettings), cx)
                }),
        )
}

pub fn notices(cx: &App) -> Vec<Div> {
    let config = cx.global::<AgentConfig>();
    let theme = cx.theme();
    let mut notices = Vec::new();
    if let Some(problem) = &config.state.problem {
        notices.push(div().text_color(theme.danger).child(format!(
            "⚠ {}",
            t!("notice.settings_problem", problem = problem)
        )));
    }
    if let Some(failure) = &config.failure {
        notices.push(div().text_color(theme.danger).child(format!("⚠ {failure}")));
    }
    for problem in &cx.global::<KeymapState>().problems {
        notices.push(div().text_color(theme.warning).child(format!(
            "⚠ {}",
            t!("notice.keymap_problem", problem = problem.text())
        )));
    }
    for warning in config.state.current.plain_secret_warnings() {
        notices.push(
            div()
                .text_color(theme.warning)
                .child(format!("⚠ {warning}")),
        );
    }
    notices
}

pub fn bridge_status(cx: &App) -> AnyElement {
    let status = match &cx.global::<AgentConfig>().bridge {
        BridgeStatus::Off => t!("settings.bridge.off").to_string(),
        BridgeStatus::Listening => t!("settings.bridge.listening").to_string(),
        BridgeStatus::Failed(why) => t!("settings.bridge.failed", why = why),
    };
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(t!("settings.bridge.status", status = status))
        .into_any_element()
}

pub fn button(
    id: impl Into<ElementId>,
    label: &'static str,
    accessibility: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id)
        .label(label)
        .accessibility_label(accessibility)
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

pub fn value(text: &'static str, cx: &App) -> AnyElement {
    div()
        .font_family("monospace")
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

pub fn settings_file(cx: &App) -> Option<PathBuf> {
    cx.global::<AgentConfig>()
        .paths
        .as_ref()
        .map(SettingsPaths::settings)
}
