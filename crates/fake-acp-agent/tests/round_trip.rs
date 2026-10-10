#![allow(
    clippy::unwrap_used,
    reason = "test helpers fail the test by panicking; clippy only exempts #[test] functions"
)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use zenkai_agent::chat::launch::LaunchPlan;
use zenkai_agent::chat::session::{
    ChoiceKind, Connection, McpRelay, PermissionAsk, SessionConfig, SessionError, SessionEvent,
    SessionHandle, start,
};
use zenkai_agent::chat::state::{AgentState, ConfigId, ConfigKind, ConfigSource};
use zenkai_agent::chat::thread::{AgentUpdate, ToolStatus, TurnEnd};

const WAIT: Duration = Duration::from_secs(30);

struct Running {
    handle: SessionHandle,
    events: Receiver<SessionEvent>,
}

fn config(mode: &str, extra_env: &[(&str, String)]) -> SessionConfig {
    config_in(mode, extra_env, std::env::temp_dir())
}

fn config_in(mode: &str, extra_env: &[(&str, String)], folder: PathBuf) -> SessionConfig {
    let mut env = vec![("FAKE_ACP_MODE".to_string(), mode.to_string())];
    env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.clone())));
    SessionConfig {
        plan: LaunchPlan::Direct {
            program: PathBuf::from(env!("CARGO_BIN_EXE_zenkai-fake-acp-agent")),
            args: Vec::new(),
        },
        folder,
        env,
        relay: Some(McpRelay {
            program: PathBuf::from("zenkai-mcp.exe"),
            pipe: "pipe-name".to_string(),
            token: "token".to_string(),
        }),
    }
}

fn launch(config: SessionConfig) -> Running {
    let (handle, events, run) = start(config);
    std::thread::spawn(move || futures::executor::block_on(run));
    Running { handle, events }
}

impl Running {
    fn next(&self) -> SessionEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(event) = self.events.try_recv() {
                return event;
            }
            assert!(Instant::now() < deadline, "no event within {WAIT:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn until<T>(&self, mut pick: impl FnMut(SessionEvent) -> Option<T>) -> T {
        loop {
            if let Some(found) = pick(self.next()) {
                return found;
            }
        }
    }

    fn ready(&self) -> String {
        self.until(|event| match event {
            SessionEvent::Connection(Connection::Ready { agent }) => Some(agent),
            SessionEvent::Failed(error) => panic!("session failed: {error}"),
            _ => None,
        })
    }

    fn permission(&self) -> PermissionAsk {
        self.until(|event| match event {
            SessionEvent::Permission(ask) => Some(ask),
            _ => None,
        })
    }

    fn turn_end(&self) -> TurnEnd {
        self.until(|event| match event {
            SessionEvent::TurnEnded(end) => Some(end),
            _ => None,
        })
    }

    fn text_until_turn_end(&self) -> (String, TurnEnd) {
        let mut text = String::new();
        let end = self.until(|event| match event {
            SessionEvent::Update(AgentUpdate::Message(chunk)) => {
                text.push_str(&chunk);
                None
            }
            SessionEvent::TurnEnded(end) => Some(end),
            _ => None,
        });
        (text, end)
    }

    fn close(&self) {
        self.handle.process_tree().close();
        self.until(|event| match event {
            SessionEvent::Closed | SessionEvent::Failed(_) => Some(()),
            _ => None,
        });
    }
}

#[test]
fn a_prompt_streams_text_runs_a_tool_and_waits_for_the_users_permission() {
    let session = launch(config("normal", &[]));
    assert_eq!(session.ready(), "Fake agent");
    session.handle.prompt("please read".to_string(), None);

    let ask = session.permission();
    assert_eq!(ask.title, "read_range on Sheet1");
    let kinds: Vec<ChoiceKind> = ask.choices.iter().map(|choice| choice.kind).collect();
    assert_eq!(kinds, [ChoiceKind::AllowOnce, ChoiceKind::RejectOnce]);
    let allow = ask.choices[0].clone();
    ask.choose(&allow);

    let (text, end) = session.text_until_turn_end();
    assert!(text.contains("Done: the range holds 3 values."), "{text}");
    assert_eq!(end, TurnEnd::Finished);
    session.close();
}

#[test]
fn the_tool_call_arrives_before_its_permission_and_completes_after_the_answer() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("please read".to_string(), None);
    let started = session.until(|event| match event {
        SessionEvent::Update(AgentUpdate::ToolStarted(card)) => Some(card),
        _ => None,
    });
    assert_eq!(started.title, "read_range");
    assert_eq!(started.status, ToolStatus::InProgress);
    let ask = session.permission();
    assert_eq!(ask.tool, started.id);
    let allow = ask.choices[0].clone();
    ask.choose(&allow);
    let finished = session.until(|event| match event {
        SessionEvent::Update(AgentUpdate::ToolChanged { status, .. }) => status,
        _ => None,
    });
    assert_eq!(finished, ToolStatus::Completed);
    session.close();
}

#[test]
fn denying_a_permission_tells_the_agent_to_stand_down() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("please read".to_string(), None);
    let ask = session.permission();
    let deny = ask.choices[1].clone();
    ask.choose(&deny);
    let (text, end) = session.text_until_turn_end();
    assert!(text.contains("I did not read it"), "{text}");
    assert_eq!(end, TurnEnd::Finished);
    session.close();
}

#[test]
fn dismissing_a_permission_cancels_it() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("please read".to_string(), None);
    session.permission().cancel();
    let (text, _) = session.text_until_turn_end();
    assert!(text.contains("I did not read it"), "{text}");
    session.close();
}

#[test]
fn stop_cancels_a_turn_in_progress() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("slow please".to_string(), None);
    session.until(|event| match event {
        SessionEvent::Update(AgentUpdate::Message(_)) => Some(()),
        _ => None,
    });
    session.handle.cancel();
    assert_eq!(session.turn_end(), TurnEnd::Cancelled);
    session.close();
}

#[test]
fn the_session_runs_in_the_given_folder_keeps_it_and_passes_only_the_zenkai_tools() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("ventas.xlsx"), b"user data").unwrap();
    let session = launch(config_in("normal", &[], folder.path().to_path_buf()));
    // The fake agent refuses to initialize when file-system or terminal access is advertised,
    // so reaching Ready proves none was.
    session.ready();
    session.handle.prompt("echo".to_string(), None);
    let (text, _) = session.text_until_turn_end();
    assert!(
        text.contains(&format!("cwd {} empty false", folder.path().display())),
        "{text}"
    );
    assert!(
        text.contains("mcp zenkai env ZENKAI_MCP_PIPE+ZENKAI_MCP_TOKEN"),
        "{text}"
    );
    // The session also carries the prompt in `_meta` for agents that read it there.
    assert!(text.contains("prompt in meta true"), "{text}");
    session.close();
    assert!(folder.path().join("ventas.xlsx").is_file());
}

#[test]
fn an_agent_without_a_session_prompt_reads_it_once_in_the_first_message() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt(
        "echo".to_string(),
        Some("[Zenkai] The user is looking at Ventas".to_string()),
    );
    let (first, _) = session.text_until_turn_end();
    assert!(
        first.contains("You are an assistant inside Zenkai"),
        "{first}"
    );
    assert!(first.contains("The user is looking at Ventas"), "{first}");
    session.handle.prompt("echo".to_string(), None);
    let (second, _) = session.text_until_turn_end();
    assert!(
        !second.contains("You are an assistant inside Zenkai"),
        "{second}"
    );
    session.close();
}

#[test]
fn a_new_session_reports_its_id_so_the_chat_can_list_it_later() {
    let session = launch(config("normal", &[]));
    let id = session.until(|event| match event {
        SessionEvent::Started(id) => Some(id),
        _ => None,
    });
    assert_eq!(id, "session-1");
    session.close();
}

#[test]
fn an_agent_that_exits_mid_turn_ends_the_session_with_a_failure() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("exit now".to_string(), None);
    let failure = session.until(|event| match event {
        SessionEvent::Failed(error) => Some(error),
        _ => None,
    });
    assert!(matches!(failure, SessionError::Exited { .. }), "{failure}");
}

#[test]
fn an_agent_that_needs_sign_in_reports_it() {
    let session = launch(config("auth", &[]));
    let failure = session.until(|event| match event {
        SessionEvent::Failed(error) => Some(error),
        SessionEvent::Connection(Connection::Ready { .. }) => panic!("it should not be ready"),
        _ => None,
    });
    assert_eq!(failure, SessionError::AuthRequired);
}

#[test]
#[cfg(windows)]
fn closing_the_session_ends_a_process_the_agent_started() {
    let folder = tempfile::tempdir().unwrap();
    let pid_file = folder.path().join("grandchild.pid");
    let session = launch(config(
        "grandchild",
        &[("FAKE_ACP_PIDFILE", pid_file.display().to_string())],
    ));
    session.ready();
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(is_running(pid), "the grandchild should be running");
    session.close();
    let deadline = Instant::now() + Duration::from_secs(15);
    while is_running(pid) {
        assert!(
            Instant::now() < deadline,
            "the grandchild survived the close"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn is_running(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

impl Running {
    // Folds state changes into an AgentState until `done` holds for it.
    fn state_until(&self, done: impl Fn(&AgentState) -> bool) -> AgentState {
        let mut state = AgentState::default();
        loop {
            if done(&state) {
                return state;
            }
            if let SessionEvent::State(change) = self.next() {
                state.apply(change);
            }
        }
    }
}

#[test]
fn what_the_agent_advertises_becomes_typed_state() {
    let session = launch(config("normal", &[]));
    let state = session.state_until(|state| {
        !state.selects.is_empty() && !state.commands.is_empty() && state.abilities.load_session
    });
    assert!(state.abilities.list_sessions && state.can_resume());
    let model = state.select(ConfigKind::Model).unwrap();
    assert_eq!(model.current_label(), "Fast model");
    assert!(model.offers("deep"));
    assert_eq!(model.source, ConfigSource::ConfigOption);
    assert_eq!(state.select(ConfigKind::Effort).unwrap().current, "low");
    let mode = state.select(ConfigKind::Mode).unwrap();
    assert_eq!(mode.source, ConfigSource::LegacyMode);
    assert_eq!(mode.current, "ask");
    let names: Vec<&str> = state.commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["compact", "review"]);
    session.close();
}

#[test]
fn changing_the_model_or_mode_is_confirmed_by_the_agent() {
    let session = launch(config("normal", &[]));
    let mut state = session.state_until(|state| state.selects.len() == 3);
    session.handle.set_config(
        ConfigId::new("model"),
        ConfigSource::ConfigOption,
        "deep".to_string(),
    );
    session.handle.set_config(
        ConfigId::new("mode"),
        ConfigSource::LegacyMode,
        "plan".to_string(),
    );
    while state.select(ConfigKind::Model).unwrap().current != "deep"
        || state.select(ConfigKind::Mode).unwrap().current != "plan"
    {
        if let SessionEvent::State(change) = session.next() {
            state.apply(change);
        }
    }
    session.close();
}

#[test]
fn context_usage_arrives_after_a_turn() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("please read".to_string(), None);
    let ask = session.permission();
    let allow = ask.choices[0].clone();
    ask.choose(&allow);
    let usage = session.until(|event| match event {
        SessionEvent::State(zenkai_agent::chat::state::StateChange::Usage(usage)) => Some(usage),
        _ => None,
    });
    assert_eq!(usage.fraction(), Some(0.12));
    session.close();
}

#[test]
fn past_sessions_are_listed_and_one_can_be_resumed_with_its_history() {
    let session = launch(config("normal", &[]));
    session.ready();
    let started = ["old-1", "old-2", "unknown"]
        .into_iter()
        .map(String::from)
        .collect();
    session.handle.list_sessions(started);
    let listed = session.state_until(|state| !state.past_sessions.is_empty());
    let titles: Vec<Option<&str>> = listed
        .past_sessions
        .iter()
        .map(|past| past.title.as_deref())
        .collect();
    assert_eq!(titles, [Some("Fix the dates"), Some("Chart of sales")]);
    session.handle.resume(listed.past_sessions[0].id.clone());
    let mut replayed = Vec::new();
    session.until(|event| match event {
        SessionEvent::Update(update) => {
            replayed.push(update);
            None
        }
        SessionEvent::Resumed => Some(()),
        _ => None,
    });
    assert_eq!(
        replayed,
        [
            AgentUpdate::UserText("Earlier question".to_string()),
            AgentUpdate::Message("Earlier answer".to_string()),
        ]
    );
    session.close();
}
