use std::path::PathBuf;

use zenkai_agent::bridge::Bridge;
use zenkai_agent::chat::environment::{self, EnvironmentError};
use zenkai_agent::chat::launch::{
    self, INSTALLED_MARKER, LaunchEnvironment, LaunchError, LaunchPlan,
};
use zenkai_agent::chat::session::{McpRelay, SessionConfig, SessionHandle};
use zenkai_agent::detect;
use zenkai_agent::secrets::Secrets;
use zenkai_agent::settings::{AgentId, AgentServer, Settings};
use zenkai_agent::tools::ToolEndpoint;
use zenkai_i18n::t;

use crate::settings_window::relay_program;

#[derive(Debug, thiserror::Error)]
pub(super) enum PrepareError {
    #[error("{}", t!("chat.error.no_agent"))]
    NoAgent,
    #[error("{}", t!("chat.error.cannot_start", error = .0))]
    Launch(#[from] LaunchError),
    #[error("{}", t!("chat.error.cannot_start", error = .0))]
    Environment(#[from] EnvironmentError),
    #[error("{}", t!("chat.error.bridge", error = .0))]
    Bridge(#[from] zenkai_agent::bridge::BridgeError),
    #[error("{}", t!("chat.error.no_data_folder"))]
    NoDataFolder,
    #[error("{}", t!("chat.error.relay_missing"))]
    RelayMissing,
}

// What the user confirms before a launch that did not come from the Settings page.
#[derive(Clone)]
pub(super) struct Prepared {
    pub id: AgentId,
    pub server: AgentServer,
    pub plan: LaunchPlan,
}

pub(super) struct Live {
    pub handle: SessionHandle,
    pub agent: AgentId,
    pub ready: bool,
    pub _bridge: Bridge,
}

pub(super) fn chosen_agent(settings: &Settings) -> Option<(AgentId, AgentServer)> {
    let servers = &settings.agents.servers;
    let id = settings
        .agents
        .default
        .clone()
        .filter(|id| servers.contains_key(id))
        .or_else(|| servers.keys().next().cloned())?;
    let server = servers.get(&id)?.clone();
    Some((id, server))
}

// Looks programs up on PATH, so it runs off the UI thread.
pub(super) fn plan_launch(settings: &Settings) -> Result<Prepared, PrepareError> {
    let (id, server) = chosen_agent(settings).ok_or(PrepareError::NoAgent)?;
    let search_path = detect::search_path();
    let find = |name: &str| detect::find_in(&search_path, name);
    let agents_folder = crate::recovery::session_directory()
        .ok_or(PrepareError::NoDataFolder)?
        .join("agents");
    let plan = launch::plan(
        &server,
        &LaunchEnvironment {
            find: &find,
            agents_folder,
        },
    )?;
    Ok(Prepared { id, server, plan })
}

// A package that is not installed yet needs a download, which only a message may start.
pub(super) fn needs_install(plan: &LaunchPlan) -> bool {
    match plan {
        LaunchPlan::Direct { .. } => false,
        LaunchPlan::Package { folder, .. } => !folder.join(INSTALLED_MARKER).is_file(),
    }
}

pub(super) struct Armed {
    pub config: SessionConfig,
    pub bridge: Bridge,
}

// Reads secrets and opens a private tool channel for this session only: its own pipe name and
// token, no endpoint file, so nothing outside the agent can find it.
pub(super) fn arm(prepared: &Prepared, endpoint: ToolEndpoint) -> Result<Armed, PrepareError> {
    let env = environment::resolve(&prepared.server, Secrets::platform)?;
    let relay_program: PathBuf = relay_program()
        .filter(|path| path.is_file())
        .ok_or(PrepareError::RelayMissing)?;
    let bridge = Bridge::start(endpoint, None)?;
    let address = bridge.address().clone();
    Ok(Armed {
        config: SessionConfig {
            plan: prepared.plan.clone(),
            env,
            relay: Some(McpRelay {
                program: relay_program,
                pipe: address.pipe,
                token: address.token,
            }),
        },
        bridge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(json: &str) -> Settings {
        Settings::parse(json).unwrap()
    }

    #[test]
    fn the_default_agent_is_chosen_when_it_exists() {
        let settings = settings(
            r#"{"agents": {"default": "b", "servers": {
                "a": {"name": "A", "command": "x"}, "b": {"name": "B", "command": "y"}}}}"#,
        );
        assert_eq!(chosen_agent(&settings).unwrap().0, AgentId::new("b"));
    }

    #[test]
    fn without_a_default_the_first_server_is_chosen() {
        let settings = settings(
            r#"{"agents": {"servers": {
                "b": {"name": "B", "command": "y"}, "a": {"name": "A", "command": "x"}}}}"#,
        );
        assert_eq!(chosen_agent(&settings).unwrap().0, AgentId::new("a"));
    }

    #[test]
    fn no_servers_means_no_agent() {
        assert!(chosen_agent(&Settings::default()).is_none());
    }
}
