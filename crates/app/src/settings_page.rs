use std::collections::BTreeMap;

use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::detect::{Detection, NodeStatus, Program};
use zenkai_agent::presets::{PRESETS, Preset};
use zenkai_agent::secrets::SecretStatus;
use zenkai_agent::settings::{ExternalAgents, PermissionMode, SecretName};

use crate::actions::*;
use crate::agent_settings::{self, AgentConfig, BridgeStatus, SecretState};

const RELAY_EXE: &str = if cfg!(windows) {
    "zenkai-mcp.exe"
} else {
    "zenkai-mcp"
};

pub struct SettingsPage {
    focus: FocusHandle,
    // The command that registers Zenkai in Claude Code, pointing at the relay shipped
    // next to this executable.
    claude_command: Option<String>,
    secret_inputs: BTreeMap<SecretName, Entity<InputState>>,
    _config: Subscription,
}

impl SettingsPage {
    pub fn new(cx: &mut Context<Self>) -> SettingsPage {
        agent_settings::detect_agents(cx);
        let relay = std::env::current_exe()
            .ok()
            .map(|exe| exe.with_file_name(RELAY_EXE));
        SettingsPage {
            focus: cx.focus_handle(),
            claude_command: relay
                .map(|relay| format!("claude mcp add zenkai -- \"{}\"", relay.display())),
            secret_inputs: BTreeMap::new(),
            _config: cx.observe_global::<AgentConfig>(|_, cx| cx.notify()),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
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
                        .placeholder("Paste the value")
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

    fn save_typed_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<SecretName> = self.secret_inputs.keys().cloned().collect();
        for name in names {
            self.save_secret(name, window, cx);
        }
    }
}

fn section(title: &'static str) -> Div {
    v_flex()
        .gap_2()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
}

fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn detection_rows(detection: Option<&Detection>, cx: &App) -> Vec<Div> {
    let Some(detection) = detection else {
        return vec![muted("Looking for installed agents…", cx)];
    };
    Program::ALL
        .into_iter()
        .map(|program| {
            let status = match (program, detection.found.get(&program)) {
                (_, None) => "Not found".to_string(),
                (Program::Node, Some(path)) => match &detection.node {
                    NodeStatus::Installed(version) => format!("{version}  {}", path.display()),
                    NodeStatus::Unreadable(why) => format!("version unknown ({why})"),
                    NodeStatus::Missing => "Not found".to_string(),
                },
                (_, Some(path)) => path.display().to_string(),
            };
            h_flex()
                .gap_3()
                .child(div().w(px(120.0)).child(program.label()))
                .child(muted(status, cx))
        })
        .collect()
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = cx.global::<AgentConfig>();
        let settings = config.state.current.clone();
        let problem = config.state.problem.clone();
        let failure = config.failure.clone();
        let detection = config.detection.clone();
        let detected = detection_rows(detection.as_ref(), cx);
        let secrets = config.secrets.clone();
        let bridge = match &config.bridge {
            BridgeStatus::Off => "Off".to_string(),
            BridgeStatus::Listening => {
                "On: MCP clients with the command below can connect".to_string()
            }
            BridgeStatus::Failed(why) => format!("Could not start: {why}"),
        };
        let claude_command = self.claude_command.clone();
        let file = config
            .paths
            .as_ref()
            .map(|paths| paths.settings().display().to_string());
        let agents = &settings.agents;

        let mut notes: Vec<Div> = Vec::new();
        if let Some(problem) = problem {
            notes.push(div().text_color(cx.theme().danger).child(format!(
                "⚠ {problem}. Zenkai keeps using the last valid settings."
            )));
        }
        if let Some(failure) = failure {
            notes.push(
                div()
                    .text_color(cx.theme().danger)
                    .child(format!("⚠ {failure}")),
            );
        }
        for warning in settings.plain_secret_warnings() {
            notes.push(
                div()
                    .text_color(cx.theme().warning)
                    .child(format!("⚠ {warning}")),
            );
        }

        let configured: Vec<Div> = agents
            .servers
            .iter()
            .map(|(id, server)| {
                let is_default = agents.default.as_ref() == Some(id);
                let command = std::iter::once(server.command.as_str())
                    .chain(server.args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                let target = id.clone();
                h_flex()
                    .gap_3()
                    .child(div().w(px(120.0)).child(server.name.clone()))
                    .child(muted(command, cx).flex_1())
                    .child(
                        Button::new(SharedString::from(format!("default-{id}")))
                            .label(if is_default {
                                "✓ Default"
                            } else {
                                "Make default (Alt+D)"
                            })
                            .selected(is_default)
                            .on_click(move |_, _, cx| {
                                let target = target.clone();
                                agent_settings::change(cx, move |settings| {
                                    settings.agents.default = Some(target.clone());
                                });
                            }),
                    )
            })
            .collect();

        let presets: Vec<Div> = PRESETS
            .iter()
            .map(|preset| {
                let added = agents.servers.contains_key(&preset.agent_id());
                let node_note = detection
                    .as_ref()
                    .and_then(|d| d.node_problem(preset.min_node_major))
                    .map(|problem| problem.to_string());
                let preset = *preset;
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new(SharedString::from(format!("preset-{}", preset.id)))
                                    .label(if added {
                                        format!("✓ {} added", preset.name)
                                    } else {
                                        format!("Add {}", preset.name)
                                    })
                                    .disabled(added)
                                    .on_click(move |_, _, cx| add_preset(preset, cx)),
                            )
                            .child(muted(
                                format!("npx {} (Node {}+)", preset.package, preset.min_node_major),
                                cx,
                            )),
                    )
                    .children(node_note.map(|note| muted(format!("⚠ {note}"), cx)))
            })
            .collect();

        let permission_buttons = PermissionMode::ALL.map(|mode| {
            let selected = agents.permission == mode;
            let action: Box<dyn Action> = match mode {
                PermissionMode::ReadOnly => Box::new(PermissionReadOnly),
                PermissionMode::AskBeforeWrite => Box::new(PermissionAskBeforeWrite),
                PermissionMode::Automatic => Box::new(PermissionAutomatic),
            };
            Button::new(SharedString::from(format!("permission-{mode:?}")))
                .label(if selected {
                    format!("✓ {}", mode.label())
                } else {
                    mode.label().to_string()
                })
                .selected(selected)
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        });

        let secret_rows: Vec<Div> = settings
            .secret_names()
            .into_iter()
            .map(|name| {
                let status = match secrets.get(&name) {
                    Some(SecretState::Known(SecretStatus::Stored)) => "Stored".to_string(),
                    Some(SecretState::Known(SecretStatus::Missing)) => "Not set".to_string(),
                    Some(SecretState::Unavailable(why)) => format!("Unavailable: {why}"),
                    None => "Checking…".to_string(),
                };
                let input = self.secret_input(&name, window, cx);
                let target = name.clone();
                h_flex()
                    .gap_3()
                    .child(div().w(px(160.0)).child(name.to_string()))
                    .child(muted(status, cx).w(px(90.0)))
                    .child(div().flex_1().child(Input::new(&input)))
                    .child(
                        Button::new(SharedString::from(format!("secret-{name}")))
                            .label("Save (Alt+S)")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.save_secret(target.clone(), window, cx)
                            })),
                    )
            })
            .collect();

        let theme = cx.theme();
        v_flex()
            .id("settings-page")
            .key_context("SettingsPage")
            .track_focus(&self.focus)
            .occlude()
            .w(px(760.0))
            .max_h(px(620.0))
            .overflow_y_scroll()
            .p_4()
            .gap_4()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .on_action(|_: &FocusNextControl, window, cx| window.focus_next(cx))
            .on_action(
                cx.listener(|this, _: &SaveSecrets, window, cx| {
                    this.save_typed_secrets(window, cx)
                }),
            )
            .on_action(|_: &FocusPreviousControl, window, cx| window.focus_prev(cx))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Settings"),
                    )
                    .child(Button::new("settings-close").label("Close").on_click(
                        |_, window, cx| window.dispatch_action(Box::new(CloseSettings), cx),
                    )),
            )
            .children(file.map(|file| muted(format!("Saved in {file}"), cx)))
            .children(notes)
            .child(
                section("Installed agents").children(detected).child(
                    Button::new("detect-agents")
                        .label("Detect again (F5)")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(DetectAgents), cx)
                        }),
                ),
            )
            .child(section("Add an agent").children(presets))
            .child(
                section("Configured agents")
                    .children(configured)
                    .when(agents.servers.is_empty(), |this| {
                        this.child(muted("None yet. Add one above.", cx))
                    }),
            )
            .child(
                section("What agents may do to the open workbook")
                    .child(h_flex().gap_2().children(permission_buttons))
                    .child(muted(
                        "Ask before writing shows every change for approval first. Agents never \
                         save the file, and every change can be undone.",
                        cx,
                    )),
            )
            .child(
                section("External agents")
                    .child(
                        Switch::new("external-agents")
                            .checked(agents.external_agents == ExternalAgents::Allowed)
                            .label("Allow MCP clients outside Zenkai, such as Claude Code (Alt+E)")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleExternalAgents), cx)
                            }),
                    )
                    .child(muted(format!("Status: {bridge}"), cx))
                    .children(claude_command.map(|command| {
                        let copied = command.clone();
                        v_flex()
                            .gap_1()
                            .child(muted("Add Zenkai to Claude Code with:", cx))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .px_2()
                                            .py_1()
                                            .border_1()
                                            .border_color(cx.theme().border)
                                            .rounded_md()
                                            .font_family("monospace")
                                            .text_sm()
                                            .child(command),
                                    )
                                    .child(
                                        Button::new("copy-claude-command").label("Copy").on_click(
                                            move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copied.clone(),
                                                ))
                                            },
                                        ),
                                    ),
                            )
                            .child(muted(
                                "Agents only see the open workbook: no files, no terminal, no \
                                 saving. They cannot turn off their own tools, such as Claude \
                                 Code's terminal, so review what they ask to run.",
                                cx,
                            ))
                    })),
            )
            .when(!secret_rows.is_empty(), |this| {
                this.child(
                    section("Secrets (Windows Credential Manager)")
                        .children(secret_rows)
                        .child(muted(
                            "Values are never written to settings.json or to the log.",
                            cx,
                        )),
                )
            })
    }
}

pub fn add_preset(preset: Preset, cx: &mut App) {
    agent_settings::change(cx, move |settings| {
        let agents = &mut settings.agents;
        agents
            .servers
            .entry(preset.agent_id())
            .or_insert_with(|| preset.server());
        if agents.default.is_none() {
            agents.default = Some(preset.agent_id());
        }
    });
}

// The keyboard way to pick the default agent: each press moves to the next one.
pub fn cycle_default_agent(cx: &mut App) {
    agent_settings::change(cx, |settings| {
        let agents = &mut settings.agents;
        let ids: Vec<_> = agents.servers.keys().cloned().collect();
        let next = match &agents.default {
            Some(current) => ids
                .iter()
                .skip_while(|id| *id != current)
                .nth(1)
                .or(ids.first()),
            None => ids.first(),
        };
        agents.default = next.cloned();
    });
}

pub fn set_permission(mode: PermissionMode, cx: &mut App) {
    agent_settings::change(cx, move |settings| settings.agents.permission = mode);
}

pub fn toggle_external_agents(cx: &mut App) {
    agent_settings::change(cx, |settings| {
        settings.agents.external_agents = match settings.agents.external_agents {
            ExternalAgents::Blocked => ExternalAgents::Allowed,
            ExternalAgents::Allowed => ExternalAgents::Blocked,
        };
    });
}
