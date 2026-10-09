use zenkai_agent::chat::state::AgentState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZenkaiCommand {
    Selection,
    Generate,
    Chart,
}

impl ZenkaiCommand {
    const ALL: [ZenkaiCommand; 3] = [
        ZenkaiCommand::Selection,
        ZenkaiCommand::Generate,
        ZenkaiCommand::Chart,
    ];

    fn name(self) -> &'static str {
        match self {
            ZenkaiCommand::Selection => "selection",
            ZenkaiCommand::Generate => "generate",
            ZenkaiCommand::Chart => "chart",
        }
    }

    fn description(self) -> &'static str {
        match self {
            ZenkaiCommand::Selection => "Add the current selection as context",
            ZenkaiCommand::Generate => "Fill a range with Generate data",
            ZenkaiCommand::Chart => "Chart the selection",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Zenkai(ZenkaiCommand),
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub description: String,
    pub target: Target,
}

// Zenkai's own commands first, then whatever the agent advertises.
pub fn entries(query: &str, state: &AgentState) -> Vec<Entry> {
    let query = query.to_lowercase();
    let own = ZenkaiCommand::ALL
        .into_iter()
        .filter(|command| command.name().starts_with(&query))
        .map(|command| Entry {
            name: format!("/{}", command.name()),
            description: command.description().to_string(),
            target: Target::Zenkai(command),
        });
    let agent = state
        .matching_commands(&query)
        .into_iter()
        .map(|command| Entry {
            name: format!("/{}", command.name),
            description: command.description.clone(),
            target: Target::Agent,
        });
    own.chain(agent).collect()
}

#[cfg(test)]
mod tests {
    use zenkai_agent::chat::state::{SlashCommand, StateChange};

    use super::*;

    fn state_with(names: &[&str]) -> AgentState {
        let mut state = AgentState::default();
        state.apply(StateChange::Commands(
            names
                .iter()
                .map(|name| SlashCommand {
                    name: name.to_string(),
                    description: format!("{name} description"),
                    input_hint: None,
                })
                .collect(),
        ));
        state
    }

    #[test]
    fn zenkai_commands_come_before_the_agents() {
        let found = entries("", &state_with(&["compact", "review"]));
        let names: Vec<&str> = found.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            ["/selection", "/generate", "/chart", "/review", "/compact"]
        );
        assert_eq!(found[3].target, Target::Agent);
        assert_eq!(found[0].target, Target::Zenkai(ZenkaiCommand::Selection));
    }

    #[test]
    fn typing_narrows_both_groups_and_ignores_case() {
        let state = state_with(&["compact", "context"]);
        let names: Vec<String> = entries("C", &state).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["/chart", "/compact", "/context"]);
        assert!(entries("zz", &state).is_empty());
    }

    #[test]
    fn an_agent_with_no_commands_still_offers_zenkais() {
        assert_eq!(entries("", &AgentState::default()).len(), 3);
    }
}
