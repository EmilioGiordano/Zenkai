use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::detect::Detection;
use zenkai_agent::presets::{PRESETS, Preset};
use zenkai_agent::secrets::SecretStatus;
use zenkai_agent::settings::{AgentId, AgentServer, SecretName, Settings};
use zenkai_i18n::t;

use super::{SettingsWindow, agents, brand, info, update};
use crate::actions::DetectAgents;
use crate::agent_settings::{self, AgentConfig, SecretState};

enum Status {
    Ready,
    NeedsAttention(String),
    Checking,
    NotInstalled,
}

struct AgentRow {
    id: AgentId,
    name: String,
    mark: brand::Mark,
    detail: String,
    status: Status,
    configured: bool,
    is_default: bool,
    preset: Option<Preset>,
    server: Option<AgentServer>,
}

fn account_of(preset: &Preset) -> &'static str {
    match preset.id {
        "claude" => t!("settings.agent.account_claude"),
        "gemini" => t!("settings.agent.account_google"),
        _ => t!("settings.agent.account_openai"),
    }
}

fn rows(settings: &Settings, detection: Option<&Detection>) -> Vec<AgentRow> {
    let agents = &settings.agents;
    let presets = PRESETS.iter().map(|preset| {
        let id = preset.agent_id();
        let server = agents.servers.get(&id).cloned();
        let configured = server.is_some();
        let problem = detection
            .and_then(|detection| detection.node_problem(preset.min_node_major))
            .map(|problem| problem.to_string());
        let status = match (configured, detection, problem) {
            (false, _, _) => Status::NotInstalled,
            (true, None, _) => Status::Checking,
            (true, Some(_), Some(problem)) => Status::NeedsAttention(problem),
            (true, Some(_), None) => Status::Ready,
        };
        AgentRow {
            name: preset.name.to_string(),
            mark: brand::Mark::of_preset(preset.id, preset.name),
            detail: account_of(preset).to_string(),
            is_default: agents.default.as_ref() == Some(&id),
            id,
            status,
            configured,
            preset: Some(*preset),
            server,
        }
    });
    let custom = agents
        .servers
        .iter()
        .filter(|(id, _)| !PRESETS.iter().any(|preset| preset.agent_id() == **id))
        .map(|(id, server)| AgentRow {
            id: id.clone(),
            name: server.name.clone(),
            mark: brand::Mark::of_preset(id.as_str(), &server.name),
            detail: std::iter::once(server.command.as_str())
                .chain(server.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            status: Status::Ready,
            configured: true,
            is_default: agents.default.as_ref() == Some(id),
            preset: None,
            server: Some(server.clone()),
        });
    presets.chain(custom).collect()
}

impl SettingsWindow {
    pub(super) fn add_custom_agent_button(&self, cx: &App) -> AnyElement {
        let file = info::settings_file(cx);
        Button::new("add-custom-agent")
            .label(t!("settings.agent.add_custom"))
            .accessibility_label(t!("settings.agent.add_custom.accessible"))
            .tooltip(t!("settings.agent.add_custom.tooltip"))
            .disabled(file.is_none())
            .on_click(move |_, _, cx| {
                if let Some(file) = &file {
                    cx.open_with_system(file);
                }
            })
            .into_any_element()
    }

    pub(super) fn agents_list(
        &mut self,
        settings: &Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detection = cx.global::<AgentConfig>().detection.clone();
        let list = rows(settings, detection.as_ref());
        let entries: Vec<AnyElement> = list
            .into_iter()
            .enumerate()
            .map(|(index, agent)| self.agent_row(index, agent, window, cx))
            .collect();
        v_flex()
            .mx_neg_4()
            .mb_neg_3p5()
            .children(entries)
            .child(
                h_flex()
                    .px_4()
                    .py_2p5()
                    .border_t_1()
                    .border_color(cx.theme().background)
                    .child(
                        Button::new("detect-agents")
                            .ghost()
                            .compact()
                            .label(t!("settings.agent.detect_again"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(DetectAgents), cx)
                            }),
                    ),
            )
            .into_any_element()
    }

    fn agent_row(
        &mut self,
        index: usize,
        agent: AgentRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (dot, status_text) = match &agent.status {
            Status::Ready => (theme.success, t!("settings.agent.ready")),
            Status::NeedsAttention(_) => (theme.warning, t!("settings.agent.needs_attention")),
            Status::Checking => (theme.muted_foreground, t!("settings.agent.checking")),
            Status::NotInstalled => (theme.muted_foreground, t!("settings.agent.not_installed")),
        };
        let detail = match &agent.status {
            Status::NeedsAttention(problem) => problem.clone(),
            _ => agent.detail.clone(),
        };
        let weak = cx.entity().downgrade();
        let id = agent.id.clone();
        let configuring = self.configuring.as_ref() == Some(&agent.id);
        let action = if agent.configured {
            let toggled = id.clone();
            Button::new(("configure-agent", index))
                .label(t!("settings.agent.configure"))
                .selected(configuring)
                .accessibility_label(t!("settings.agent.configure_named", name = agent.name))
                .on_click(move |_, _, cx| {
                    let toggled = toggled.clone();
                    update(&weak, cx, move |this, cx| {
                        this.configuring = if this.configuring.as_ref() == Some(&toggled) {
                            None
                        } else {
                            Some(toggled)
                        };
                        cx.notify();
                    })
                })
        } else if let Some(preset) = agent.preset {
            Button::new(("install-agent", index))
                .primary()
                .label(t!("settings.agent.install"))
                .accessibility_label(t!("settings.agent.install_named", name = agent.name))
                .on_click(move |_, _, cx| agents::add_preset(preset, cx))
        } else {
            Button::new(("install-agent", index)).label(t!("settings.agent.install"))
        };
        let menu_id = id.clone();
        let (configured, is_default) = (agent.configured, agent.is_default);
        let more = Button::new(("more-agent", index))
            .ghost()
            .compact()
            .icon(IconName::Ellipsis)
            .accessibility_label(t!("settings.agent.more_named", name = agent.name))
            .dropdown_menu(move |menu, _, _| {
                let make_default = menu_id.clone();
                let remove = menu_id.clone();
                menu.item(
                    PopupMenuItem::new(if is_default {
                        t!("settings.agent.default_agent")
                    } else {
                        t!("settings.agent.make_default")
                    })
                    .disabled(!configured || is_default)
                    .on_click(move |_, _, cx| agents::make_default(make_default.clone(), cx)),
                )
                .item(
                    PopupMenuItem::new(t!("settings.agent.remove"))
                        .disabled(!configured)
                        .on_click(move |_, _, cx| agents::remove_agent(remove.clone(), cx)),
                )
            });
        let panel = configuring
            .then(|| self.configure_panel(&agent, window, cx))
            .flatten();
        let theme = cx.theme();
        v_flex()
            .child(
                h_flex()
                    .gap_3p5()
                    .px_4()
                    .py_3()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.background)
                    .child(brand::mark(&agent.mark, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(agent.name.clone()),
                                    )
                                    .when(agent.is_default, |this| {
                                        this.child(
                                            div()
                                                .px_1p5()
                                                .rounded_md()
                                                .text_xs()
                                                .bg(theme.secondary)
                                                .text_color(theme.secondary_foreground)
                                                .child(t!("settings.agent.default_badge")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(detail),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_xs()
                            .flex_shrink_0()
                            .child(div().size(px(8.0)).rounded_full().bg(dot))
                            .child(status_text),
                    )
                    .child(action)
                    .child(more),
            )
            .children(panel)
            .into_any_element()
    }

    fn configure_panel(
        &mut self,
        agent: &AgentRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let server = agent.server.as_ref()?;
        let command = std::iter::once(server.command.as_str())
            .chain(server.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let names: Vec<SecretName> = server
            .env
            .values()
            .filter_map(|value| match value {
                zenkai_agent::settings::EnvValue::Secret { secret } => Some(secret.clone()),
                zenkai_agent::settings::EnvValue::Text(_) => None,
            })
            .collect();
        let secrets = cx.global::<AgentConfig>().secrets.clone();
        let secret_rows: Vec<AnyElement> = names
            .into_iter()
            .map(|name| {
                let status = match secrets.get(&name) {
                    Some(SecretState::Known(SecretStatus::Stored)) => {
                        t!("settings.secret.stored").to_string()
                    }
                    Some(SecretState::Known(SecretStatus::Missing)) => {
                        t!("settings.secret.missing").to_string()
                    }
                    Some(SecretState::Unavailable(why)) => {
                        t!("settings.secret.unavailable", why = why)
                    }
                    None => t!("settings.agent.checking").to_string(),
                };
                let input = self.secret_input(&name, window, cx);
                let target = name.clone();
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().w(px(170.0)).child(name.to_string()))
                    .child(
                        div()
                            .w(px(90.0))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    )
                    .child(div().flex_1().child(Input::new(&input)))
                    .child(
                        Button::new(SharedString::from(format!("secret-{name}")))
                            .label(t!("settings.secret.save"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.save_secret(target.clone(), window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_2p5()
                .px_4()
                .pb_3p5()
                .child(
                    div()
                        .font_family("monospace")
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(command),
                )
                .when(secret_rows.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("settings.agent.no_key")),
                    )
                })
                .children(secret_rows)
                .into_any_element(),
        )
    }

    fn secret_input(
        &mut self,
        name: &SecretName,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        self.secret_inputs
            .entry(name.clone())
            .or_insert_with(|| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder(t!("settings.secret.placeholder"))
                })
            })
            .clone()
    }

    fn save_secret(&mut self, name: SecretName, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.secret_inputs.get(&name) else {
            return;
        };
        let value = input.read(cx).value().to_string();
        if value.is_empty() {
            return;
        }
        input.update(cx, |state, cx| state.set_value("", window, cx));
        agent_settings::store_secret(cx, name, value);
    }

    pub(super) fn save_typed_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<SecretName> = self.secret_inputs.keys().cloned().collect();
        for name in names {
            self.save_secret(name, window, cx);
        }
    }
}
