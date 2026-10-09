use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, InitializeResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions, SessionInfo,
    SessionModeState, UsageUpdate,
};

use crate::chat::state::{
    Abilities, Choice, ConfigId, ConfigKind, ConfigSource, ContextUsage, PastSession, Select,
    SlashCommand,
};

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
            Some(Select {
                id: ConfigId::new(&*option.id.0),
                label: option.name.clone(),
                kind: kind_of(option),
                source: ConfigSource::ConfigOption,
                current: select.current_value.0.to_string(),
                choices: flat
                    .into_iter()
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
