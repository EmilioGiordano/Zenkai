use std::collections::BTreeMap;

use crate::settings::{AgentId, AgentServer};

// Launch commands as published in the ACP registry
// (https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json), pinned so an
// agent update never changes what Zenkai starts without a Zenkai update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub package: &'static str,
    pub extra_args: &'static [&'static str],
    pub min_node_major: u32,
    pub login_command: &'static str,
    pub provider: &'static str,
}

pub const CLAUDE: Preset = Preset {
    id: "claude",
    name: "Claude",
    package: "@agentclientprotocol/claude-agent-acp@0.88.0",
    extra_args: &[],
    min_node_major: 22,
    login_command: "claude /login",
    provider: "Anthropic",
};

pub const GEMINI: Preset = Preset {
    id: "gemini",
    name: "Gemini CLI",
    package: "@google/gemini-cli@0.63.0",
    extra_args: &["--acp"],
    min_node_major: 20,
    login_command: "gemini",
    provider: "Google",
};

pub const CODEX: Preset = Preset {
    id: "codex",
    name: "Codex",
    package: "@agentclientprotocol/codex-acp@2.1.1",
    extra_args: &[],
    min_node_major: 20,
    login_command: "codex login",
    provider: "OpenAI",
};

pub const PRESETS: [Preset; 3] = [CLAUDE, GEMINI, CODEX];

impl Preset {
    pub fn for_agent(id: &AgentId) -> Option<Preset> {
        PRESETS.into_iter().find(|preset| preset.id == id.as_str())
    }

    pub fn agent_id(&self) -> AgentId {
        AgentId::new(self.id)
    }

    pub fn server(&self) -> AgentServer {
        let args = ["-y", self.package]
            .into_iter()
            .chain(self.extra_args.iter().copied())
            .map(str::to_string)
            .collect();
        AgentServer {
            name: self.name.to_string(),
            command: "npx".to_string(),
            args,
            env: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_run_the_pinned_registry_packages_through_npx() {
        let gemini = GEMINI.server();
        assert_eq!(gemini.command, "npx");
        assert_eq!(gemini.args, ["-y", "@google/gemini-cli@0.63.0", "--acp"]);
        for preset in PRESETS {
            assert!(
                preset
                    .package
                    .rsplit_once('@')
                    .is_some_and(|(_, v)| !v.is_empty())
            );
        }
    }
}
