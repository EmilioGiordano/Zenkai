use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
            agents: AgentSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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

// The settings in force: a file that stops parsing keeps the last good settings and
// reports why, so a half-written edit never switches the agents off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsState {
    pub current: Settings,
    pub problem: Option<SettingsError>,
}

impl SettingsState {
    pub fn apply(&mut self, loaded: Result<Settings, SettingsError>) {
        match loaded {
            Ok(settings) => {
                self.current = settings;
                self.problem = None;
            }
            Err(problem) => self.problem = Some(problem),
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
        state.apply(Settings::parse(FULL));
        let good = state.current.clone();
        state.apply(Settings::parse("{ \"agents\": "));
        assert_eq!(state.current, good);
        assert!(matches!(state.problem, Some(SettingsError::Parse { .. })));
        state.apply(Settings::parse("{}"));
        assert_eq!(state.current, Settings::default());
        assert_eq!(state.problem, None);
    }
}
