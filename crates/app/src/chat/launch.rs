use std::path::PathBuf;

use zenkai_agent::bridge::Bridge;
use zenkai_agent::chat::environment::{self, EnvironmentError};
use zenkai_agent::chat::launch::{self, LaunchEnvironment, LaunchError, LaunchPlan};
use zenkai_agent::chat::session::{McpRelay, SessionConfig, SessionHandle};
use zenkai_agent::detect;
use zenkai_agent::secrets::Secrets;
use zenkai_agent::settings::{AgentId, AgentServer, Settings};
use zenkai_agent::tools::ToolEndpoint;

use crate::settings_page::relay_program;

#[derive(Debug, thiserror::Error)]
pub(super) enum PrepareError {
    #[error(
        "No agent is set up. Open Settings (Ctrl+,) and add Claude, Gemini CLI or Codex, then pick it as the default."
    )]
    NoAgent,
    #[error("The agent cannot be started: {0}")]
    Launch(#[from] LaunchError),
    #[error("The agent cannot be started: {0}")]
    Environment(#[from] EnvironmentError),
    #[error("Zenkai could not open its tool channel for the agent: {0}")]
    Bridge(#[from] zenkai_agent::bridge::BridgeError),
    #[error("Zenkai has no folder for the agent packages (LOCALAPPDATA is not set).")]
    NoDataFolder,
    #[error("zenkai-mcp was not found next to Zenkai, so the agent cannot get the workbook tools.")]
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
