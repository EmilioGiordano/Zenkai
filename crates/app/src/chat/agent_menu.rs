use gpui_kit::*;
use zenkai_agent::presets::PRESETS;
use zenkai_agent::settings::{AgentId, Settings};
use zenkai_i18n::t;

use super::launch::chosen_agent;
use super::menus::{Item, item, note, popover, rule};
use super::{ChatPanel, Menu};
use crate::agent_settings::{self, AgentConfig};

const AGENT_MENU_WIDTH: f32 = 280.0;
const ALWAYS_LISTED: [&str; 2] = ["claude", "codex"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentOption {
    pub id: AgentId,
    pub name: String,
    pub installed: bool,
    pub active: bool,
}

pub(super) fn agent_options(settings: &Settings) -> Vec<AgentOption> {
    let servers = &settings.agents.servers;
    let active = chosen_agent(settings).map(|(id, _)| id);
    let presets = PRESETS
        .iter()
        .filter(|preset| {
            ALWAYS_LISTED.contains(&preset.id) || servers.contains_key(&preset.agent_id())
        })
        .map(|preset| {
            let id = preset.agent_id();
            AgentOption {
                name: servers
                    .get(&id)
                    .map_or_else(|| preset.name.to_string(), |server| server.name.clone()),
                installed: servers.contains_key(&id),
                active: active.as_ref() == Some(&id),
                id,
            }
        });
    let custom = servers
        .iter()
        .filter(|(id, _)| !PRESETS.iter().any(|preset| preset.agent_id() == **id))
        .map(|(id, server)| AgentOption {
            id: id.clone(),
            name: server.name.clone(),
            installed: true,
            active: active.as_ref() == Some(id),
        });
    presets.chain(custom).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingAgent {
    pub id: AgentId,
    pub failure_before: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum SwitchOutcome {
    Waiting,
    Reached,
    Failed,
}

// A failure already showing when the switch began belongs to something else.
fn switch_outcome(
    reached: bool,
    failure_before: &Option<String>,
    failure_now: &Option<String>,
) -> SwitchOutcome {
    if reached {
        SwitchOutcome::Reached
    } else if failure_now.is_some() && failure_now != failure_before {
        SwitchOutcome::Failed
    } else {
        SwitchOutcome::Waiting
    }
}

impl ChatPanel {
    pub(super) fn agent_rows(&self, cx: &App) -> Vec<AgentOption> {
        agent_options(&cx.global::<AgentConfig>().state.current)
    }

    pub(crate) fn toggle_agent_menu(&mut self, cx: &mut Context<Self>) {
        self.toggle_menu(Menu::Agent, cx);
    }

    // The default agent is written in the background, so the new conversation starts when
    // the settings report it; starting it now would still launch the previous agent.
    pub(super) fn pick_agent(&mut self, id: AgentId, cx: &mut Context<Self>) {
        self.menu = None;
        let rows = self.agent_rows(cx);
        let option = rows.iter().find(|o| o.id == id);
        match option {
            Some(option) if option.active => {}
            Some(option) if option.installed => {
                if let Some(previous) = rows.iter().find(|o| o.active) {
                    self.remembered_selects
                        .insert(previous.id.clone(), self.state.selects.clone());
                }
                self.pending_agent = Some(PendingAgent {
                    id: id.clone(),
                    failure_before: cx.global::<AgentConfig>().failure.clone(),
                });
                agent_settings::change(cx, move |settings| {
                    settings.agents.default = Some(id.clone());
                });
            }
            _ => crate::settings_window::open(cx),
        }
        cx.notify();
    }

    pub(super) fn open_agent_settings(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        crate::settings_window::open(cx);
        cx.notify();
    }

    pub(super) fn follow_agent_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_agent.clone() else {
            return;
        };
        let config = cx.global::<AgentConfig>();
        let reached = chosen_agent(&config.state.current).is_some_and(|(id, _)| id == pending.id);
        match switch_outcome(reached, &pending.failure_before, &config.failure) {
            SwitchOutcome::Waiting => {}
            SwitchOutcome::Failed => self.pending_agent = None,
            SwitchOutcome::Reached => {
                self.pending_agent = None;
                self.state.selects = self
                    .remembered_selects
                    .get(&pending.id)
                    .cloned()
                    .unwrap_or_default();
                self.model_choice = None;
                self.effort_choice = None;
                self.switch_conversation(window, cx);
            }
        }
    }

    pub(super) fn render_agent_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Agent) {
            return None;
        }
        let mut menu = popover(
            "chat-agent-menu",
            t!("chat.switch_agent.label"),
            AGENT_MENU_WIDTH,
            cx,
        );
        let options = self.agent_rows(cx);
        let count = options.len();
        for (index, option) in options.into_iter().enumerate() {
            let id = option.id.clone();
            menu = menu.child(
                item(
                    Item {
                        id: SharedString::from(format!("agent-{}", option.id.as_str())),
                        name: option.name.into(),
                        note: (option.installed && !option.active)
                            .then(|| SharedString::from(t!("chat.agent.installed"))),
                        checked: option.active,
                        highlighted: self.menu_index == index,
                    },
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_agent(id.clone(), cx))),
            );
        }
        menu = menu
            .child(rule(cx))
            .child(
                item(
                    Item {
                        id: "agent-manage".into(),
                        name: t!("chat.agent.manage").into(),
                        note: None,
                        checked: false,
                        highlighted: self.menu_index == count,
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_agent_settings(cx))),
            )
            .child(rule(cx))
            .child(note(t!("chat.agent.footer"), cx));
        Some(menu.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use zenkai_agent::settings::Settings;

    use super::{SwitchOutcome, agent_options, switch_outcome};

    fn settings(json: &str) -> Settings {
        Settings::parse(json).unwrap()
    }

    #[test]
    fn claude_and_codex_are_always_listed_and_only_installed_ones_are_marked() {
        let options = agent_options(&settings(
            r#"{"agents": {"default": "claude", "servers": {
                "claude": {"name": "Claude", "command": "npx"}}}}"#,
        ));
        let names: Vec<_> = options
            .iter()
            .map(|o| (o.name.as_str(), o.installed, o.active))
            .collect();
        assert_eq!(names, [("Claude", true, true), ("Codex", false, false)]);
    }

    #[test]
    fn the_chosen_default_is_the_only_active_agent() {
        let options = agent_options(&settings(
            r#"{"agents": {"default": "codex", "servers": {
                "claude": {"name": "Claude", "command": "npx"},
                "codex": {"name": "Codex", "command": "npx"}}}}"#,
        ));
        let active: Vec<_> = options
            .iter()
            .filter(|o| o.active)
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(active, ["codex"]);
    }

    #[test]
    fn custom_servers_are_listed_after_the_presets() {
        let options = agent_options(&settings(
            r#"{"agents": {"servers": {"mine": {"name": "Mine", "command": "x"}}}}"#,
        ));
        assert_eq!(options.last().map(|o| o.name.as_str()), Some("Mine"));
        assert!(options.last().is_some_and(|o| o.installed && o.active));
    }

    #[test]
    fn a_failure_from_before_the_switch_does_not_cancel_it() {
        let stale = Some("could not write".to_string());
        assert_eq!(
            switch_outcome(false, &stale, &stale),
            SwitchOutcome::Waiting
        );
    }

    #[test]
    fn a_new_failure_cancels_the_switch_and_the_new_default_completes_it() {
        let new = Some("disk full".to_string());
        assert_eq!(switch_outcome(false, &None, &new), SwitchOutcome::Failed);
        assert_eq!(switch_outcome(true, &None, &None), SwitchOutcome::Reached);
    }
}
