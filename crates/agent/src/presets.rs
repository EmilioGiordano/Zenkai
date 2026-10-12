use std::collections::BTreeMap;

use crate::settings::{AgentId, AgentServer};

// Launch commands as published in the ACP registry
// (https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json), pinned so an
// agent update never changes what Zenkai starts without a Zenkai update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeBinary {
    pub os: &'static str,
    pub arch: &'static str,
    pub path: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Node,
    Native(&'static [NativeBinary]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub package: &'static str,
    pub extra_args: &'static [&'static str],
    pub entry: Entry,
    pub min_node_major: u32,
    pub login_command: &'static str,
    pub provider: &'static str,
    // package.json and package-lock.json committed with Zenkai: `npm ci` installs exactly these
    // tarballs and checks their integrity hashes.
    pub lock: (&'static str, &'static str),
}

pub const CLAUDE: Preset = Preset {
    id: "claude",
    name: "Claude",
    package: "@agentclientprotocol/claude-agent-acp@0.88.0",
    extra_args: &[],
    entry: Entry::Node,
    min_node_major: 22,
    login_command: "claude /login",
    provider: "Anthropic",
    lock: (
        include_str!("../lockfiles/claude/package.json"),
        include_str!("../lockfiles/claude/package-lock.json"),
    ),
};

pub const GEMINI: Preset = Preset {
    id: "gemini",
    name: "Gemini CLI",
    package: "@google/gemini-cli@0.63.0",
    extra_args: &["--acp"],
    entry: Entry::Node,
    min_node_major: 20,
    login_command: "gemini",
    provider: "Google",
    lock: (
        include_str!("../lockfiles/gemini/package.json"),
        include_str!("../lockfiles/gemini/package-lock.json"),
    ),
};

pub const CODEX: Preset = Preset {
    id: "codex",
    name: "Codex",
    package: "@agentclientprotocol/codex-acp@2.1.1",
    extra_args: &[],
    entry: Entry::Node,
    min_node_major: 20,
    login_command: "codex login",
    provider: "OpenAI",
    lock: (
        include_str!("../lockfiles/codex/package.json"),
        include_str!("../lockfiles/codex/package-lock.json"),
    ),
};

pub const OPENCODE: Preset = Preset {
    id: "opencode",
    name: "opencode",
    package: "opencode-ai@1.18.32",
    extra_args: &["acp"],
    entry: Entry::Native(&[
        NativeBinary {
            os: "windows",
            arch: "x86_64",
            path: "node_modules/opencode-windows-x64/bin/opencode.exe",
        },
        NativeBinary {
            os: "windows",
            arch: "aarch64",
            path: "node_modules/opencode-windows-arm64/bin/opencode.exe",
        },
        NativeBinary {
            os: "macos",
            arch: "x86_64",
            path: "node_modules/opencode-darwin-x64/bin/opencode",
        },
        NativeBinary {
            os: "macos",
            arch: "aarch64",
            path: "node_modules/opencode-darwin-arm64/bin/opencode",
        },
        NativeBinary {
            os: "linux",
            arch: "x86_64",
            path: "node_modules/opencode-linux-x64/bin/opencode",
        },
        NativeBinary {
            os: "linux",
            arch: "aarch64",
            path: "node_modules/opencode-linux-arm64/bin/opencode",
        },
    ]),
    min_node_major: 20,
    login_command: "opencode auth login",
    provider: "opencode",
    lock: (
        include_str!("../lockfiles/opencode/package.json"),
        include_str!("../lockfiles/opencode/package-lock.json"),
    ),
};

pub const PRESETS: [Preset; 4] = [CLAUDE, GEMINI, CODEX, OPENCODE];

impl Preset {
    pub fn for_package(package: &str) -> Option<Preset> {
        PRESETS.into_iter().find(|preset| preset.package == package)
    }

    pub fn for_agent(id: &AgentId) -> Option<Preset> {
        PRESETS.into_iter().find(|preset| preset.id == id.as_str())
    }

    pub fn native_path(&self) -> Option<&'static str> {
        let Entry::Native(binaries) = self.entry else {
            return None;
        };
        binaries
            .iter()
            .find(|binary| {
                binary.os == std::env::consts::OS && binary.arch == std::env::consts::ARCH
            })
            .map(|binary| binary.path)
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
        let opencode = OPENCODE.server();
        assert_eq!(opencode.command, "npx");
        assert_eq!(opencode.args, ["-y", "opencode-ai@1.18.32", "acp"]);
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
