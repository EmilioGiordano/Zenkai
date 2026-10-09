// What the connected agent says it can do, as typed state the chat shows: models, modes and
// reasoning effort, slash commands, context usage and past sessions. Everything here is
// optional because each agent advertises a different subset.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConfigId(String);

impl ConfigId {
    pub fn new(id: impl Into<String>) -> ConfigId {
        ConfigId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigKind {
    Model,
    Mode,
    Effort,
    Other,
}

// Older agents expose modes through their own request; newer ones list them as a config option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    ConfigOption,
    LegacyMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Select {
    pub id: ConfigId,
    pub label: String,
    pub kind: ConfigKind,
    pub source: ConfigSource,
    pub current: String,
    pub choices: Vec<Choice>,
}

impl Select {
    pub fn current_label(&self) -> &str {
        self.choices
            .iter()
            .find(|choice| choice.value == self.current)
            .map_or(self.current.as_str(), |choice| choice.label.as_str())
    }

    pub fn offers(&self, value: &str) -> bool {
        self.choices.iter().any(|choice| choice.value == value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextUsage {
    pub used: u64,
    pub size: u64,
}

impl ContextUsage {
    // 0.0 to 1.0, for the progress ring; an agent that reports no size has no ring.
    pub fn fraction(self) -> Option<f32> {
        (self.size > 0).then(|| (self.used as f32 / self.size as f32).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastSession {
    pub id: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Abilities {
    pub list_sessions: bool,
    pub load_session: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateChange {
    Abilities(Abilities),
    Selects(Vec<Select>),
    Commands(Vec<SlashCommand>),
    Usage(ContextUsage),
    PastSessions(Vec<PastSession>),
    CurrentMode(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentState {
    pub abilities: Abilities,
    pub selects: Vec<Select>,
    pub commands: Vec<SlashCommand>,
    pub usage: Option<ContextUsage>,
    pub past_sessions: Vec<PastSession>,
}

impl AgentState {
    pub fn apply(&mut self, change: StateChange) {
        match change {
            StateChange::Abilities(abilities) => self.abilities = abilities,
            StateChange::Selects(mut selects) => {
                // A config-option reply does not carry the older mode request's select.
                for old in &self.selects {
                    if old.source == ConfigSource::LegacyMode
                        && !selects.iter().any(|select| select.kind == old.kind)
                    {
                        selects.push(old.clone());
                    }
                }
                self.selects = selects;
            }
            StateChange::Commands(commands) => self.commands = commands,
            StateChange::Usage(usage) => self.usage = Some(usage),
            StateChange::PastSessions(sessions) => self.past_sessions = sessions,
            StateChange::CurrentMode(mode) => {
                for select in &mut self.selects {
                    if select.source == ConfigSource::LegacyMode && select.offers(&mode) {
                        select.current = mode.clone();
                    }
                }
            }
        }
    }

    pub fn select(&self, kind: ConfigKind) -> Option<&Select> {
        self.selects.iter().find(|select| select.kind == kind)
    }

    // Commands whose name starts with what was typed after the slash, shortest names first.
    pub fn matching_commands(&self, typed: &str) -> Vec<&SlashCommand> {
        let typed = typed.to_lowercase();
        let mut found: Vec<&SlashCommand> = self
            .commands
            .iter()
            .filter(|command| command.name.to_lowercase().starts_with(&typed))
            .collect();
        found.sort_by_key(|command| command.name.len());
        found
    }

    pub fn can_resume(&self) -> bool {
        self.abilities.load_session
    }
}

// The text after a leading "/" when the composer holds a slash command being typed.
pub fn slash_query(composer: &str) -> Option<&str> {
    let rest = composer.strip_prefix('/')?;
    (!rest.contains(char::is_whitespace)).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(kind: ConfigKind, current: &str, values: &[&str]) -> Select {
        Select {
            id: ConfigId::new("id"),
            label: "Label".to_string(),
            kind,
            source: ConfigSource::ConfigOption,
            current: current.to_string(),
            choices: values
                .iter()
                .map(|value| Choice {
                    value: value.to_string(),
                    label: value.to_uppercase(),
                    description: None,
                })
                .collect(),
        }
    }

    fn command(name: &str) -> SlashCommand {
        SlashCommand {
            name: name.to_string(),
            description: String::new(),
            input_hint: None,
        }
    }

    #[test]
    fn a_new_state_offers_nothing_until_the_agent_says_so() {
        let state = AgentState::default();
        assert!(state.selects.is_empty() && state.commands.is_empty());
        assert!(state.usage.is_none() && !state.can_resume());
    }

    #[test]
    fn changes_replace_the_matching_part_of_the_state() {
        let mut state = AgentState::default();
        state.apply(StateChange::Selects(vec![
            select(ConfigKind::Model, "a", &["a", "b"]),
            select(ConfigKind::Effort, "low", &["low", "high"]),
        ]));
        state.apply(StateChange::Abilities(Abilities {
            list_sessions: true,
            load_session: true,
        }));
        assert_eq!(
            state.select(ConfigKind::Model).unwrap().current_label(),
            "A"
        );
        assert!(state.select(ConfigKind::Mode).is_none());
        assert!(state.can_resume());
        state.apply(StateChange::Selects(vec![select(
            ConfigKind::Model,
            "b",
            &["a", "b"],
        )]));
        assert_eq!(state.selects.len(), 1);
        assert!(state.select(ConfigKind::Effort).is_none());
    }

    #[test]
    fn a_config_reply_keeps_the_legacy_mode_select_it_does_not_carry() {
        let mut state = AgentState::default();
        let mut mode = select(ConfigKind::Mode, "ask", &["ask", "plan"]);
        mode.source = ConfigSource::LegacyMode;
        state.apply(StateChange::Selects(vec![mode]));
        state.apply(StateChange::Selects(vec![select(
            ConfigKind::Model,
            "a",
            &["a"],
        )]));
        assert!(state.select(ConfigKind::Mode).is_some());
        state.apply(StateChange::CurrentMode("plan".to_string()));
        assert_eq!(state.select(ConfigKind::Mode).unwrap().current, "plan");
    }

    #[test]
    fn a_select_only_offers_what_the_agent_listed() {
        let model = select(ConfigKind::Model, "a", &["a", "b"]);
        assert!(model.offers("b") && !model.offers("c"));
        assert_eq!(
            select(ConfigKind::Model, "zz", &["a"]).current_label(),
            "zz"
        );
    }

    #[test]
    fn usage_is_a_fraction_of_the_window_and_never_past_one() {
        assert_eq!(
            ContextUsage {
                used: 50,
                size: 200
            }
            .fraction(),
            Some(0.25)
        );
        assert_eq!(
            ContextUsage {
                used: 500,
                size: 200
            }
            .fraction(),
            Some(1.0)
        );
        assert_eq!(ContextUsage { used: 5, size: 0 }.fraction(), None);
    }

    #[test]
    fn slash_commands_match_by_prefix_shortest_first() {
        let mut state = AgentState::default();
        state.apply(StateChange::Commands(vec![
            command("compact"),
            command("context"),
            command("co"),
            command("review"),
        ]));
        let names: Vec<&str> = state
            .matching_commands("CO")
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert_eq!(names, ["co", "compact", "context"]);
        assert_eq!(state.matching_commands("").len(), 4);
        assert!(state.matching_commands("zzz").is_empty());
    }

    #[test]
    fn the_slash_popover_opens_only_while_the_first_word_is_being_typed() {
        assert_eq!(slash_query("/com"), Some("com"));
        assert_eq!(slash_query("/"), Some(""));
        assert_eq!(slash_query("/compact now"), None);
        assert_eq!(slash_query("hello /x"), None);
        assert_eq!(slash_query(""), None);
    }
}
