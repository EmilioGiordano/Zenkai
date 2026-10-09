use std::path::PathBuf;

use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_agent::settings::HeldChange;
use zenkai_agent::settings_file::SettingsPaths;

use crate::actions::{ApplyHeldSettings, KeepCurrentSettings};
use crate::agent_settings::{self, AgentConfig, BridgeStatus};

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
        .child(div().flex_1().min_w_0().child(format!(
            "{} Zenkai confirms this at every start and whenever the file changes outside this              window. Apply it only if you set it yourself.",
            agent_settings::held_summary(held)
        )))
        .child(
            Button::new("held-apply")
                .label("Apply (Alt+A)")
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ApplyHeldSettings), cx)),
        )
        .child(
            Button::new("held-keep")
                .primary()
                .label("Keep current (Alt+K)")
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
            "⚠ {problem}. Zenkai keeps using the last valid settings."
        )));
    }
    if let Some(failure) = &config.failure {
        notices.push(div().text_color(theme.danger).child(format!("⚠ {failure}")));
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
        BridgeStatus::Off => "Off".to_string(),
        BridgeStatus::Listening => "On: MCP clients with the command below can connect".to_string(),
        BridgeStatus::Failed(why) => format!("Could not start: {why}"),
    };
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(format!("Status: {status}"))
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
