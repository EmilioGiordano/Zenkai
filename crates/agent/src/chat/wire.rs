use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, InitializeResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions, SessionInfo,
    SessionModeState, UsageUpdate,
};

use crate::chat::state::{
    Abilities, Choice, ConfigId, ConfigKind, ConfigSource, ContextUsage, PastSession, Select,
    SlashCommand,
};

// Modes in which the agent stops asking before it writes outside its folder or runs commands
// (Claude's bypass and classifier modes, Codex's full access, Gemini's yolo). The chat never
// offers them, so every write outside the folder and every command reaches the user.
const UNGUARDED_MODES: [&str; 4] = ["bypassPermissions", "auto", "full-access", "yolo"];

fn offered(kind: ConfigKind, value: &str) -> bool {
    kind != ConfigKind::Mode || !UNGUARDED_MODES.contains(&value)
}

// A session that starts in an unguarded mode (from the user's own agent settings) is moved
// to the first mode the chat offers.
pub fn leave_unguarded_mode(selects: &[Select]) -> Option<(ConfigId, ConfigSource, String)> {
    let mode = selects
        .iter()
        .find(|select| select.kind == ConfigKind::Mode)?;
    if mode
        .choices
        .iter()
        .any(|choice| choice.value == mode.current)
    {
        return None;
    }
    let safe = mode.choices.first()?;
    Some((mode.id.clone(), mode.source, safe.value.clone()))
}

fn kind_of(option: &SessionConfigOption) -> ConfigKind {
    match &option.category {
        Some(SessionConfigOptionCategory::Mode) => ConfigKind::Mode,
        Some(SessionConfigOptionCategory::Model) => ConfigKind::Model,
        Some(SessionConfigOptionCategory::ThoughtLevel) => ConfigKind::Effort,
        _ => {
            let name = format!("{} {}", option.id.0, option.name).to_lowercase();
            if name.contains("effort") || name.contains("reasoning") {
                ConfigKind::Effort
            } else {
                ConfigKind::Other
            }
        }
    }
}

// Yes/no options have no picker in the chat, so only selects are kept.
pub fn selects_from_options(options: &[SessionConfigOption]) -> Vec<Select> {
    options
        .iter()
        .filter_map(|option| {
            let SessionConfigKind::Select(select) = &option.kind else {
                return None;
            };
            let flat = match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|group| group.options.iter())
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            let kind = kind_of(option);
            Some(Select {
                id: ConfigId::new(&*option.id.0),
                label: option.name.clone(),
                kind,
                source: ConfigSource::ConfigOption,
                current: select.current_value.0.to_string(),
                choices: flat
                    .into_iter()
                    .filter(|choice| offered(kind, &choice.value.0))
                    .map(|choice| Choice {
                        value: choice.value.0.to_string(),
                        label: choice.name.clone(),
                        description: choice.description.clone(),
                    })
                    .collect(),
            })
        })
        .collect()
}

pub fn select_from_modes(modes: &SessionModeState) -> Select {
    Select {
        id: ConfigId::new("mode"),
        label: "Mode".to_string(),
        kind: ConfigKind::Mode,
        source: ConfigSource::LegacyMode,
        current: modes.current_mode_id.0.to_string(),
        choices: modes
            .available_modes
            .iter()
            .filter(|mode| offered(ConfigKind::Mode, &mode.id.0))
            .map(|mode| Choice {
                value: mode.id.0.to_string(),
                label: mode.name.clone(),
                description: mode.description.clone(),
            })
            .collect(),
    }
}

// Config options win over the older modes request when an agent sends both.
pub fn selects_from_session(
    options: Option<&[SessionConfigOption]>,
    modes: Option<&SessionModeState>,
) -> Vec<Select> {
    let mut selects = options.map(selects_from_options).unwrap_or_default();
    if let Some(modes) = modes
        && !selects.iter().any(|select| select.kind == ConfigKind::Mode)
    {
        selects.push(select_from_modes(modes));
    }
    selects
}

pub fn abilities(response: &InitializeResponse) -> Abilities {
    let capabilities = &response.agent_capabilities;
    Abilities {
        list_sessions: capabilities.session_capabilities.list.is_some(),
        load_session: capabilities.load_session,
    }
}

pub fn commands(commands: &[AvailableCommand]) -> Vec<SlashCommand> {
    commands
        .iter()
        .map(|command| SlashCommand {
            name: command.name.clone(),
            description: command.description.clone(),
            input_hint: match &command.input {
                Some(AvailableCommandInput::Unstructured(input)) => Some(input.hint.clone()),
                _ => None,
            },
        })
        .collect()
}

pub fn usage(update: &UsageUpdate) -> ContextUsage {
    ContextUsage {
        used: update.used,
        size: update.size,
    }
}

pub fn past_session(info: &SessionInfo) -> PastSession {
    PastSession {
        id: info.session_id.0.to_string(),
        title: info.title.clone(),
        updated_at: info.updated_at.clone(),
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{NewSessionResponse, SessionMode};

    use super::*;

    const CLAUDE_NEW_SESSION: &str = include_str!("claude_new_session.json");

    fn claude_modes(current: &str) -> SessionModeState {
        SessionModeState::new(
            current.to_string(),
            [
                "default",
                "acceptEdits",
                "plan",
                "auto",
                "bypassPermissions",
            ]
            .into_iter()
            .map(|id| SessionMode::new(id, id))
            .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn modes_that_never_ask_are_not_offered() {
        let select = select_from_modes(&claude_modes("default"));
        let offered: Vec<&str> = select.choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(offered, ["default", "acceptEdits", "plan"]);
        assert_eq!(leave_unguarded_mode(&[select]), None);
    }

    #[test]
    fn a_session_that_starts_unguarded_moves_to_the_first_offered_mode() {
        let select = select_from_modes(&claude_modes("auto"));
        assert_eq!(
            leave_unguarded_mode(&[select]),
            Some((
                ConfigId::new("mode"),
                ConfigSource::LegacyMode,
                "default".to_string()
            ))
        );
    }

    #[test]
    fn the_claude_adapter_session_lists_modes_models_and_effort() {
        let response: NewSessionResponse = serde_json::from_str(CLAUDE_NEW_SESSION).unwrap();
        let selects =
            selects_from_session(response.config_options.as_deref(), response.modes.as_ref());
        let kinds: Vec<ConfigKind> = selects.iter().map(|select| select.kind).collect();
        assert_eq!(
            kinds,
            [ConfigKind::Mode, ConfigKind::Model, ConfigKind::Effort]
        );
    }
}
