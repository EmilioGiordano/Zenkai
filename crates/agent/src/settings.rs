use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zenkai_types::Language;

use crate::preferences::{AppearanceSettings, GeneralSettings};

pub const SCHEMA_FILE: &str = "settings.schema.json";
const SCHEMA_REFERENCE: &str = "./settings.schema.json";
const MAX_SECRET_NAME: usize = 128;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Zenkai settings")]
pub struct Settings {
    #[serde(rename = "$schema", default = "schema_reference")]
    #[schemars(description = "Path of the JSON Schema that Zenkai writes next to this file.")]
    schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Language of the interface, \"en\" or \"es\". Without it Zenkai follows the Windows display language. Takes effect on the next start."
    )]
    pub language: Option<Language>,
    #[serde(default)]
    pub general: GeneralSettings,
    #[serde(default)]
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub agents: AgentSettings,
}

fn schema_reference() -> String {
    SCHEMA_REFERENCE.to_string()
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            schema: schema_reference(),
            language: None,
            general: GeneralSettings::default(),
            appearance: AppearanceSettings::default(),
            agents: AgentSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Key in `servers` of the agent the chat starts with.")]
    pub default: Option<AgentId>,
    #[serde(default)]
    #[schemars(
        description = "Agents Zenkai can start, by key. Each one is launched as an ACP agent over stdio."
    )]
    pub servers: BTreeMap<AgentId, AgentServer>,
    #[serde(default)]
    #[schemars(description = "What an agent may do to the open workbook.")]
    pub permission: PermissionMode,
    #[serde(default)]
    #[schemars(
        description = "Whether MCP clients outside Zenkai (such as Claude Code) may connect to the open workbook."
    )]
    pub external_agents: ExternalAgents,
    #[serde(default = "confirm_by_default")]
    #[schemars(
        description = "Ask again at every start before agents may write without asking or outside MCP clients may connect. Turning it off from this file waits for your confirmation in Zenkai."
    )]
    pub confirm_elevated_at_start: bool,
}

fn confirm_by_default() -> bool {
    true
}

impl Default for AgentSettings {
    fn default() -> AgentSettings {
        AgentSettings {
            default: None,
            servers: BTreeMap::new(),
            permission: PermissionMode::default(),
            external_agents: ExternalAgents::default(),
            confirm_elevated_at_start: confirm_by_default(),
        }
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: &str) -> AgentId {
        AgentId(id.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentServer {
    #[schemars(description = "Name shown in Zenkai.")]
    pub name: String,
    #[schemars(description = "Program to run, found on PATH or as a full path.")]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    #[schemars(
        description = "Environment variables for the agent. Use {\"secret\": \"name\"} for keys and tokens; the value lives in Windows Credential Manager."
    )]
    pub env: BTreeMap<String, EnvValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnvValue {
    Text(String),
    Secret { secret: SecretName },
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(
    description = "Name of a secret stored in Windows Credential Manager under the service \"zenkai\"."
)]
pub struct SecretName(#[schemars(regex(pattern = r"^[A-Za-z0-9._/-]{1,128}$"))] String);

impl SecretName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SecretName {
    type Error = String;

    fn try_from(name: String) -> Result<SecretName, String> {
        let valid = !name.is_empty()
            && name.len() <= MAX_SECRET_NAME
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
        if valid {
            Ok(SecretName(name))
        } else {
            Err(format!(
                "secret name \"{name}\" must be 1 to {MAX_SECRET_NAME} letters, digits or . _ / -"
            ))
        }
    }
}

impl From<SecretName> for String {
    fn from(name: SecretName) -> String {
        name.0
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    AskBeforeWrite,
    Automatic,
}

impl PermissionMode {
    pub const ALL: [PermissionMode; 3] = [
        PermissionMode::ReadOnly,
        PermissionMode::AskBeforeWrite,
        PermissionMode::Automatic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "Read only",
            PermissionMode::AskBeforeWrite => "Ask before writing",
            PermissionMode::Automatic => "Automatic",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAgents {
    #[default]
    Blocked,
    Allowed,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum SettingsError {
    #[error("settings.json, line {line}, column {column}: {message}")]
    Parse {
        line: usize,
        column: usize,
        message: String,
    },
    #[error("settings.json: the default agent \"{0}\" is not one of agents.servers")]
    UnknownDefault(AgentId),
    #[error("settings.json is larger than 1 MB")]
    TooLarge,
    #[error("could not read settings.json: {0}")]
    Read(String),
    #[error("could not encode the settings: {0}")]
    Encode(String),
}

// A raw value under a name like these is most likely a key that belongs in Credential
// Manager, not in a file other programs can read.
const SECRET_LIKE_SUFFIXES: [&str; 5] = ["_KEY", "_TOKEN", "_SECRET", "PASSWORD", "_PAT"];

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let settings: Settings =
            serde_json::from_str(text).map_err(|error| SettingsError::Parse {
                line: error.line(),
                column: error.column(),
                message: error.to_string(),
            })?;
        if let Some(default) = &settings.agents.default
            && !settings.agents.servers.contains_key(default)
        {
            return Err(SettingsError::UnknownDefault(default.clone()));
        }
        Ok(settings)
    }

    pub fn to_json(&self) -> Result<String, SettingsError> {
        serde_json::to_string_pretty(self)
            .map(|json| json + "\n")
            .map_err(|error| SettingsError::Encode(error.to_string()))
    }

    pub fn schema_json() -> Result<String, SettingsError> {
        let schema = schemars::schema_for!(Settings);
        serde_json::to_string_pretty(&schema)
            .map(|json| json + "\n")
            .map_err(|error| SettingsError::Encode(error.to_string()))
    }

    pub fn plain_secret_warnings(&self) -> Vec<String> {
        self.agents
            .servers
            .iter()
            .flat_map(|(id, server)| {
                server
                    .env
                    .iter()
                    .filter(|(key, value)| {
                        matches!(value, EnvValue::Text(_))
                            && SECRET_LIKE_SUFFIXES
                                .iter()
                                .any(|suffix| key.to_ascii_uppercase().ends_with(suffix))
                    })
                    .map(move |(key, _)| {
                        format!(
                            "agents.servers.{id}.env.{key} looks like a secret written in plain \
                             text; store it in Credential Manager and use {{\"secret\": \"name\"}}"
                        )
                    })
            })
            .collect()
    }

    pub fn secret_names(&self) -> Vec<SecretName> {
        let mut names: Vec<SecretName> = self
            .agents
            .servers
            .values()
            .flat_map(|server| server.env.values())
            .filter_map(|value| match value {
                EnvValue::Secret { secret } => Some(secret.clone()),
                EnvValue::Text(_) => None,
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Escalation {
    WriteWithoutAsking,
    ExternalAgents,
    StopConfirmingAtStart,
}

impl Escalation {
    pub fn label(self) -> &'static str {
        match self {
            Escalation::WriteWithoutAsking => "let agents change the workbook without asking",
            Escalation::ExternalAgents => "let MCP clients outside Zenkai connect",
            Escalation::StopConfirmingAtStart => {
                "stop asking you to confirm elevated permissions at every start"
            }
        }
    }
}

// What `to` allows agents to do that `from` did not.
pub fn escalations(from: &Settings, to: &Settings) -> Vec<Escalation> {
    let mut raised = Vec::new();
    if to.agents.permission == PermissionMode::Automatic
        && from.agents.permission != PermissionMode::Automatic
    {
        raised.push(Escalation::WriteWithoutAsking);
    }
    if to.agents.external_agents == ExternalAgents::Allowed
        && from.agents.external_agents != ExternalAgents::Allowed
    {
        raised.push(Escalation::ExternalAgents);
    }
    if !to.agents.confirm_elevated_at_start && from.agents.confirm_elevated_at_start {
        raised.push(Escalation::StopConfirmingAtStart);
    }
    raised
}

// What the user has confirmed and the app may take for granted at the next start: nothing
// while the confirmation is on, and exactly the elevated values in force while it is off.
pub fn remembered_confirmations(settings: &Settings) -> Vec<Escalation> {
    if settings.agents.confirm_elevated_at_start {
        return Vec::new();
    }
    escalations(&Settings::default(), settings)
}

// A change to settings.json that gives agents more power, waiting for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldChange {
    pub settings: Settings,
    pub escalations: Vec<Escalation>,
}

// The settings in force. A file that stops parsing keeps the last good settings and
// reports why, so a half-written edit never switches the agents off. Any program running
// as the user can write settings.json, so a file change that gives agents more power is
// held until the user confirms it in Zenkai; everything else in it applies at once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsState {
    pub current: Settings,
    pub problem: Option<SettingsError>,
    pub held: Option<HeldChange>,
    // A held change the user turned down; the same file content does not ask again.
    declined: Option<Settings>,
}

impl SettingsState {
    pub fn apply_file(&mut self, loaded: Result<Settings, SettingsError>) {
        match loaded {
            Ok(settings) => {
                self.problem = None;
                self.apply_with_approval(settings, &[]);
            }
            Err(problem) => self.problem = Some(problem),
        }
    }

    // The first load of a run. Elevated values are held again at every start unless the user
    // turned the confirmation off and confirmed exactly these values before.
    pub fn apply_startup(
        &mut self,
        loaded: Result<Settings, SettingsError>,
        remembered: &[Escalation],
    ) {
        match loaded {
            Ok(settings) => {
                self.problem = None;
                let approved = if settings.agents.confirm_elevated_at_start {
                    &[]
                } else {
                    remembered
                };
                self.apply_with_approval(settings, approved);
            }
            Err(problem) => self.problem = Some(problem),
        }
    }

    // Settings written by Zenkai's own page: what the user clicked there is approved.
    pub fn apply_from_page(&mut self, settings: Settings, approved: &[Escalation]) {
        self.problem = None;
        self.apply_with_approval(settings, approved);
    }

    fn apply_with_approval(&mut self, settings: Settings, approved: &[Escalation]) {
        let raised: Vec<Escalation> = escalations(&self.current, &settings)
            .into_iter()
            .filter(|escalation| !approved.contains(escalation))
            .collect();
        if raised.is_empty() {
            self.current = settings;
            self.held = None;
            self.declined = None;
            return;
        }
        let mut safe = settings.clone();
        if raised.contains(&Escalation::WriteWithoutAsking) {
            safe.agents.permission = self.current.agents.permission;
        }
        if raised.contains(&Escalation::ExternalAgents) {
            safe.agents.external_agents = self.current.agents.external_agents;
        }
        if raised.contains(&Escalation::StopConfirmingAtStart) {
            safe.agents.confirm_elevated_at_start = self.current.agents.confirm_elevated_at_start;
        }
        self.current = safe;
        self.held = (self.declined.as_ref() != Some(&settings)).then_some(HeldChange {
            settings,
            escalations: raised,
        });
    }

    pub fn accept_held(&mut self) {
        if let Some(held) = self.held.take() {
            self.current = held.settings;
            self.declined = None;
        }
    }

    pub fn decline_held(&mut self) {
        if let Some(held) = self.held.take() {
            self.declined = Some(held.settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{
        "$schema": "./settings.schema.json",
        "agents": {
            "default": "claude",
            "servers": {
                "claude": {
                    "name": "Claude",
                    "command": "npx",
                    "args": ["-y", "@agentclientprotocol/claude-agent-acp@0.88.0"],
                    "env": { "ANTHROPIC_API_KEY": { "secret": "zenkai/anthropic" } }
                },
                "local": { "name": "Mine", "command": "C:\\tools\\agent.exe", "env": { "MODE": "fast" } }
            },
            "permission": "automatic",
            "external_agents": "allowed"
        }
    }"#;

    #[test]
    fn a_full_file_parses_into_typed_settings() {
        let settings = Settings::parse(FULL).unwrap();
        let agents = &settings.agents;
        assert_eq!(agents.default, Some(AgentId::new("claude")));
        assert_eq!(agents.permission, PermissionMode::Automatic);
        assert_eq!(agents.external_agents, ExternalAgents::Allowed);
        let claude = &agents.servers[&AgentId::new("claude")];
        assert_eq!(claude.args.len(), 2);
        assert_eq!(
            claude.env["ANTHROPIC_API_KEY"],
            EnvValue::Secret {
                secret: SecretName::try_from("zenkai/anthropic".to_string()).unwrap()
            }
        );
        assert_eq!(
            agents.servers[&AgentId::new("local")].env["MODE"],
            EnvValue::Text("fast".to_string())
        );
    }

    #[test]
    fn an_empty_object_means_safe_defaults() {
        let settings = Settings::parse("{}").unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.agents.permission, PermissionMode::AskBeforeWrite);
        assert_eq!(settings.agents.external_agents, ExternalAgents::Blocked);
    }

    #[test]
    fn errors_carry_line_and_column() {
        let error =
            Settings::parse("{\n  \"agents\": { \"permission\": \"sometimes\" }\n}").unwrap_err();
        match error {
            SettingsError::Parse { line, column, .. } => {
                assert_eq!(line, 2);
                assert!(column > 0);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn typos_and_bad_secret_names_are_rejected() {
        assert!(Settings::parse(r#"{ "agent": {} }"#).is_err());
        let bad_secret = r#"{ "agents": { "servers": { "a": { "name": "A", "command": "a",
            "env": { "K": { "secret": "has space" } } } } } }"#;
        assert!(Settings::parse(bad_secret).is_err());
    }

    #[test]
    fn a_default_agent_must_exist() {
        let error = Settings::parse(r#"{ "agents": { "default": "nobody" } }"#).unwrap_err();
        assert_eq!(error, SettingsError::UnknownDefault(AgentId::new("nobody")));
    }

    #[test]
    fn writing_and_reading_back_gives_the_same_settings() {
        let settings = Settings::parse(FULL).unwrap();
        assert_eq!(
            Settings::parse(&settings.to_json().unwrap()).unwrap(),
            settings
        );
        assert!(
            settings
                .to_json()
                .unwrap()
                .contains("\"$schema\": \"./settings.schema.json\"")
        );
    }

    #[test]
    fn the_schema_describes_the_same_types() {
        let schema: serde_json::Value =
            serde_json::from_str(&Settings::schema_json().unwrap()).unwrap();
        let agents = &schema["$defs"]["AgentSettings"]["properties"];
        assert!(agents["permission"].is_object());
        assert!(agents["external_agents"].is_object());
        let modes = &schema["$defs"]["PermissionMode"];
        let text = modes.to_string();
        for mode in ["read_only", "ask_before_write", "automatic"] {
            assert!(text.contains(mode), "{mode} missing from {text}");
        }
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        assert!(defaults.get("$schema").is_some());
        assert!(schema["properties"].get("$schema").is_some());
    }

    #[test]
    fn plain_text_keys_are_flagged_and_secret_references_are_listed() {
        let text = r#"{ "agents": { "servers": { "a": { "name": "A", "command": "a",
            "env": { "OPENAI_API_KEY": "sk-123", "B": { "secret": "b" }, "C": { "secret": "b" } } } } } }"#;
        let settings = Settings::parse(text).unwrap();
        let warnings = settings.plain_secret_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("OPENAI_API_KEY"));
        assert_eq!(settings.secret_names().len(), 1);
    }

    #[test]
    fn a_broken_reload_keeps_the_last_good_settings() {
        let mut state = SettingsState::default();
        let good = Settings::parse(r#"{ "agents": { "permission": "read_only" } }"#);
        state.apply_file(good.clone());
        state.apply_file(Settings::parse("{ \"agents\": "));
        assert_eq!(Ok(state.current.clone()), good);
        assert!(matches!(state.problem, Some(SettingsError::Parse { .. })));
        state.apply_file(Settings::parse("{}"));
        assert_eq!(state.current, Settings::default());
        assert_eq!(state.problem, None);
    }

    #[test]
    fn the_language_is_optional_and_round_trips() {
        assert_eq!(Settings::parse("{}").unwrap().language, None);
        let spanish = Settings::parse(r#"{ "language": "es" }"#).unwrap();
        assert_eq!(spanish.language, Some(Language::Spanish));
        assert!(spanish.to_json().unwrap().contains("\"language\": \"es\""));
        assert!(!Settings::default().to_json().unwrap().contains("language"));
        assert!(Settings::parse(r#"{ "language": "fr" }"#).is_err());
    }

    fn granting(text: &str) -> Settings {
        Settings::parse(text).unwrap()
    }

    #[test]
    fn a_file_that_grants_more_power_waits_for_the_user() {
        let mut state = SettingsState::default();
        let file = granting(FULL);
        state.apply_file(Ok(file.clone()));
        assert_eq!(
            state.current.agents.permission,
            PermissionMode::AskBeforeWrite
        );
        assert_eq!(
            state.current.agents.external_agents,
            ExternalAgents::Blocked
        );
        assert_eq!(state.current.agents.servers, file.agents.servers);
        let held = state.held.clone().unwrap();
        assert_eq!(
            held.escalations,
            [Escalation::WriteWithoutAsking, Escalation::ExternalAgents]
        );
        state.accept_held();
        assert_eq!(state.current, file);
        assert_eq!(state.held, None);
    }

    #[test]
    fn a_declined_change_does_not_ask_again_until_the_file_changes() {
        let mut state = SettingsState::default();
        let automatic = granting(r#"{ "agents": { "permission": "automatic" } }"#);
        state.apply_file(Ok(automatic.clone()));
        state.decline_held();
        assert_eq!(
            state.current.agents.permission,
            PermissionMode::AskBeforeWrite
        );
        state.apply_file(Ok(automatic));
        assert_eq!(state.held, None);
        let both = granting(
            r#"{ "agents": { "permission": "automatic", "external_agents": "allowed" } }"#,
        );
        state.apply_file(Ok(both));
        assert!(state.held.is_some());
    }

    #[test]
    fn taking_power_away_applies_at_once() {
        let mut state = SettingsState::default();
        state.apply_from_page(
            granting(
                r#"{ "agents": { "permission": "automatic", "external_agents": "allowed" } }"#,
            ),
            &[Escalation::WriteWithoutAsking, Escalation::ExternalAgents],
        );
        assert_eq!(state.held, None);
        state.apply_file(Ok(granting(
            r#"{ "agents": { "permission": "read_only" } }"#,
        )));
        assert_eq!(state.held, None);
        assert_eq!(state.current.agents.permission, PermissionMode::ReadOnly);
        assert_eq!(
            state.current.agents.external_agents,
            ExternalAgents::Blocked
        );
    }

    #[test]
    fn the_settings_page_approves_only_what_the_user_clicked() {
        let mut state = SettingsState::default();
        let saved = granting(
            r#"{ "agents": { "permission": "automatic", "external_agents": "allowed" } }"#,
        );
        state.apply_from_page(saved, &[Escalation::ExternalAgents]);
        assert_eq!(
            state.current.agents.external_agents,
            ExternalAgents::Allowed
        );
        assert_eq!(
            state.current.agents.permission,
            PermissionMode::AskBeforeWrite
        );
        assert_eq!(
            state.held.unwrap().escalations,
            [Escalation::WriteWithoutAsking]
        );
    }

    const CONFIRMATION_OFF: &str = r#"{ "agents": { "permission": "automatic",
        "confirm_elevated_at_start": false } }"#;

    #[test]
    fn turning_the_start_confirmation_off_from_the_file_waits_for_the_user() {
        let mut state = SettingsState::default();
        state.apply_file(Ok(granting(CONFIRMATION_OFF)));
        assert!(state.current.agents.confirm_elevated_at_start);
        assert_eq!(
            state.current.agents.permission,
            PermissionMode::AskBeforeWrite
        );
        assert_eq!(
            state.held.unwrap().escalations,
            [
                Escalation::WriteWithoutAsking,
                Escalation::StopConfirmingAtStart
            ]
        );
    }

    #[test]
    fn a_start_takes_for_granted_only_what_was_confirmed_with_the_confirmation_off() {
        let off = granting(CONFIRMATION_OFF);
        let remembered = remembered_confirmations(&off);
        assert_eq!(
            remembered,
            [
                Escalation::WriteWithoutAsking,
                Escalation::StopConfirmingAtStart
            ]
        );
        let mut state = SettingsState::default();
        state.apply_startup(Ok(off.clone()), &remembered);
        assert_eq!(state.current, off);
        assert_eq!(state.held, None);

        let mut forgotten = SettingsState::default();
        forgotten.apply_startup(Ok(off), &[]);
        assert!(forgotten.held.is_some());
        assert!(forgotten.current.agents.confirm_elevated_at_start);
        assert_eq!(
            forgotten.current.agents.permission,
            PermissionMode::AskBeforeWrite
        );
    }

    #[test]
    fn a_value_added_after_the_confirmation_is_held_even_with_the_confirmation_off() {
        let remembered = remembered_confirmations(&granting(CONFIRMATION_OFF));
        let more = granting(
            r#"{ "agents": { "permission": "automatic", "external_agents": "allowed",
                "confirm_elevated_at_start": false } }"#,
        );
        let mut state = SettingsState::default();
        state.apply_startup(Ok(more), &remembered);
        assert_eq!(state.current.agents.permission, PermissionMode::Automatic);
        assert_eq!(
            state.current.agents.external_agents,
            ExternalAgents::Blocked
        );
        assert_eq!(
            state.held.unwrap().escalations,
            [Escalation::ExternalAgents]
        );
    }

    #[test]
    fn with_the_confirmation_on_nothing_is_remembered_and_every_start_asks() {
        let on = granting(r#"{ "agents": { "permission": "automatic" } }"#);
        assert_eq!(remembered_confirmations(&on), []);
        let mut state = SettingsState::default();
        state.apply_startup(
            Ok(on),
            &[
                Escalation::WriteWithoutAsking,
                Escalation::StopConfirmingAtStart,
            ],
        );
        assert!(state.held.is_some());
    }

    #[test]
    fn a_change_made_while_the_confirmation_is_off_is_still_held() {
        let off = granting(CONFIRMATION_OFF);
        let mut state = SettingsState::default();
        state.apply_startup(Ok(off.clone()), &remembered_confirmations(&off));
        let more = granting(
            r#"{ "agents": { "permission": "automatic", "external_agents": "allowed",
                "confirm_elevated_at_start": false } }"#,
        );
        state.apply_file(Ok(more));
        assert_eq!(
            state.current.agents.external_agents,
            ExternalAgents::Blocked
        );
        assert!(state.held.is_some());
    }

    #[test]
    fn general_and_appearance_settings_round_trip_with_the_rest() {
        let settings = granting(
            r#"{ "general": { "restore_session": false, "autosave_seconds": 30 },
                "appearance": { "mode": "dark", "dark_theme": "high_contrast" } }"#,
        );
        assert!(!settings.general.restore_session);
        assert_eq!(settings.general.autosave_seconds, 30);
        assert_eq!(
            Settings::parse(&settings.to_json().unwrap()).unwrap(),
            settings
        );
        assert!(granting("{}").general.restore_session);
    }
}
