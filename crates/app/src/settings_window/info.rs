use std::path::PathBuf;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::Button;
use gpui_kit::*;
use zenkai_agent::settings_file::SettingsPaths;

use crate::agent_settings::{AgentConfig, BridgeStatus};

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
