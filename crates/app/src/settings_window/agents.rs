use zenkai_agent::presets::{self, Preset};
use zenkai_agent::settings::{AgentId, ExternalAgents, PermissionMode};

use gpui_kit::*;

use crate::actions::*;
use crate::agent_settings;

const RELAY_EXE: &str = if cfg!(windows) {
    "zenkai-mcp.exe"
} else {
    "zenkai-mcp"
};

// The commands of the AI section work without any window, so the palette and the shortcuts
// reach them from the workbook window too.
pub fn register(cx: &mut App) {
    cx.on_action(|_: &DetectAgents, cx| agent_settings::detect_agents(cx));
    cx.on_action(|_: &AddClaudeAgent, cx| add_preset(presets::CLAUDE, cx));
    cx.on_action(|_: &AddGeminiAgent, cx| add_preset(presets::GEMINI, cx));
    cx.on_action(|_: &AddCodexAgent, cx| add_preset(presets::CODEX, cx));
    cx.on_action(|_: &PermissionReadOnly, cx| set_permission(PermissionMode::ReadOnly, cx));
    cx.on_action(|_: &PermissionAskBeforeWrite, cx| {
        set_permission(PermissionMode::AskBeforeWrite, cx)
    });
    cx.on_action(|_: &PermissionAutomatic, cx| set_permission(PermissionMode::Automatic, cx));
    cx.on_action(|_: &ToggleExternalAgents, cx| toggle_external_agents(cx));
    cx.on_action(|_: &CycleDefaultAgent, cx| cycle_default_agent(cx));
    cx.on_action(|_: &CopyClaudeCommand, cx| copy_claude_command(cx));
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

pub fn make_default(id: AgentId, cx: &mut App) {
    agent_settings::change(cx, move |settings| {
        if settings.agents.servers.contains_key(&id) {
            settings.agents.default = Some(id.clone());
        }
    });
}

pub fn remove_agent(id: AgentId, cx: &mut App) {
    agent_settings::change(cx, move |settings| {
        let agents = &mut settings.agents;
        agents.servers.remove(&id);
        if agents.default.as_ref() == Some(&id) {
            agents.default = agents.servers.keys().next().cloned();
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

// The command that registers Zenkai in Claude Code, pointing at the relay shipped next to
// this executable.
pub fn relay_program() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.with_file_name(RELAY_EXE))
}

pub fn claude_command() -> Option<String> {
    let relay = relay_program()?;
    Some(format!("claude mcp add zenkai -- \"{}\"", relay.display()))
}

pub fn copy_claude_command(cx: &mut App) {
    if let Some(command) = claude_command() {
        cx.write_to_clipboard(ClipboardItem::new_string(command));
    }
}

pub fn set_permission(mode: PermissionMode, cx: &mut App) {
    agent_settings::change(cx, move |settings| settings.agents.permission = mode);
}

pub fn set_external_agents(allowed: bool, cx: &mut App) {
    agent_settings::change(cx, move |settings| {
        settings.agents.external_agents = if allowed {
            ExternalAgents::Allowed
        } else {
            ExternalAgents::Blocked
        };
    });
}

pub fn toggle_external_agents(cx: &mut App) {
    let allowed = agent_settings::settings(cx).agents.external_agents == ExternalAgents::Allowed;
    set_external_agents(!allowed, cx);
}
