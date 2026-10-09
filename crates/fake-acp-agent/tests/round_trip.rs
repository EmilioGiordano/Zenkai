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
use zenkai_agent::chat::thread::{AgentUpdate, ToolStatus, TurnEnd};

const WAIT: Duration = Duration::from_secs(30);

struct Running {
    handle: SessionHandle,
    events: Receiver<SessionEvent>,
}

fn config(mode: &str, extra_env: &[(&str, String)]) -> SessionConfig {
    let mut env = vec![("FAKE_ACP_MODE".to_string(), mode.to_string())];
    env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.clone())));
    SessionConfig {
        plan: LaunchPlan::Direct {
            program: PathBuf::from(env!("CARGO_BIN_EXE_zenkai-fake-acp-agent")),
            args: Vec::new(),
        },
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
    session.handle.prompt("please read".to_string());

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
    session.handle.prompt("please read".to_string());
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
    session.handle.prompt("please read".to_string());
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
    session.handle.prompt("please read".to_string());
    session.permission().cancel();
    let (text, _) = session.text_until_turn_end();
    assert!(text.contains("I did not read it"), "{text}");
    session.close();
}

#[test]
fn stop_cancels_a_turn_in_progress() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("slow please".to_string());
    session.until(|event| match event {
        SessionEvent::Update(AgentUpdate::Message(_)) => Some(()),
        _ => None,
    });
    session.handle.cancel();
    assert_eq!(session.turn_end(), TurnEnd::Cancelled);
    session.close();
}

#[test]
fn the_session_runs_in_an_empty_folder_and_passes_only_the_zenkai_tools_and_no_capabilities() {
    let session = launch(config("normal", &[]));
    // The fake agent refuses to initialize when file-system or terminal access is advertised,
    // so reaching Ready proves none was.
    session.ready();
    session.handle.prompt("echo".to_string());
    let (text, _) = session.text_until_turn_end();
    assert!(text.contains("empty true"), "{text}");
    assert!(
        text.contains("mcp zenkai env ZENKAI_MCP_PIPE+ZENKAI_MCP_TOKEN"),
        "{text}"
    );
    let folder = text
        .split("cwd ")
        .nth(1)
        .and_then(|rest| rest.split(" empty").next())
        .unwrap()
        .to_string();
    session.close();
    assert!(
        !PathBuf::from(folder).exists(),
        "work folder was not removed"
    );
}

#[test]
fn an_agent_that_exits_mid_turn_ends_the_session_with_a_failure() {
    let session = launch(config("normal", &[]));
    session.ready();
    session.handle.prompt("exit now".to_string());
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
