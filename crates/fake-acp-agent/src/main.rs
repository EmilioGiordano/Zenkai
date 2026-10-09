#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio as ProcessStdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AvailableCommand, AvailableCommandsUpdate, CancelNotification, ContentBlock,
    ContentChunk, Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, SessionCapabilities, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelectOption, SessionId,
    SessionInfo, SessionListCapabilities, SessionMode, SessionModeState, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, StopReason, ToolCall, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error, Responder, Stdio};

const SLEEP_MODE: &str = "sleep";
const MCP_READ_LIMIT: usize = 2_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    AuthRequired,
    Grandchild,
}

type Facts = Arc<Mutex<String>>;
type McpCommand = (PathBuf, Vec<(String, String)>);

fn main() -> Result<(), Error> {
    let mode = std::env::var("FAKE_ACP_MODE").unwrap_or_default();
    if mode == SLEEP_MODE {
        std::thread::sleep(Duration::from_secs(600));
        return Ok(());
    }
    let mode = match mode.as_str() {
        "auth" => Mode::AuthRequired,
        "grandchild" => Mode::Grandchild,
        _ => Mode::Normal,
    };
    futures::executor::block_on(serve(mode))
}

fn start_grandchild() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let child = Command::new(exe)
        .env("FAKE_ACP_MODE", SLEEP_MODE)
        .stdin(ProcessStdio::null())
        .stdout(ProcessStdio::null())
        .stderr(ProcessStdio::null())
        .spawn()?;
    if let Some(file) = std::env::var_os("FAKE_ACP_PIDFILE") {
        std::fs::write(PathBuf::from(file), child.id().to_string())?;
    }
    Ok(())
}

fn describe_session(request: &NewSessionRequest) -> String {
    let empty = std::fs::read_dir(&request.cwd).is_ok_and(|mut entries| entries.next().is_none());
    let servers: Vec<String> = request
        .mcp_servers
        .iter()
        .map(|server| match server {
            McpServer::Stdio(stdio) => {
                let names: Vec<&str> = stdio.env.iter().map(|var| var.name.as_str()).collect();
                format!("{} env {}", stdio.name, names.join("+"))
            }
            _ => "other".to_string(),
        })
        .collect();
    format!(
        "cwd {} empty {empty}; mcp {}",
        request.cwd.display(),
        servers.join(", ")
    )
}

fn stdio_server(request: &NewSessionRequest) -> Option<McpCommand> {
    request.mcp_servers.iter().find_map(|server| match server {
        McpServer::Stdio(stdio) => Some((
            stdio.command.clone(),
            stdio
                .env
                .iter()
                .map(|var| (var.name.clone(), var.value.clone()))
                .collect(),
        )),
        _ => None,
    })
}

// A minimal MCP client over the stdio server the session was given, so a run without a
// model still goes through the relay, the pipe and the tools.
fn call_over_mcp(
    program: PathBuf,
    env: Vec<(String, String)>,
    tool: &str,
    arguments: serde_json::Value,
) -> String {
    let mut child = match Command::new(program)
        .envs(env)
        .stdin(ProcessStdio::piped())
        .stdout(ProcessStdio::piped())
        .stderr(ProcessStdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return format!("could not start the MCP server: {error}"),
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return "the MCP server has no standard streams".to_string();
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut ask = |message: serde_json::Value| writeln!(stdin, "{message}");
    let init = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "fake-acp-agent", "version": "0"}}});
    let initialized = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let call = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}});
    if ask(init).is_err() || lines.next().is_none() {
        return "the MCP server did not answer initialize".to_string();
    }
    if ask(initialized).is_err() || ask(call).is_err() {
        return "the MCP server closed early".to_string();
    }
    let mut text = String::from("no answer");
    for line in lines.map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if message["id"] == 2 {
            text = message["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or("empty result")
                .chars()
                .take(MCP_READ_LIMIT)
                .collect();
            break;
        }
    }
    drop(stdin);
    text
}

fn say(connection: &ConnectionTo<Client>, session: &SessionId, text: &str) -> Result<(), Error> {
    connection.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text))),
    ))
}

fn prompt_text(request: &PromptRequest) -> String {
    request
        .prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn run_prompt(
    connection: ConnectionTo<Client>,
    responder: Responder<PromptResponse>,
    session: SessionId,
    text: String,
    facts: Facts,
    cancel: async_channel::Receiver<()>,
    mcp: Option<McpCommand>,
) -> Result<(), Error> {
    if text.contains("exit") {
        say(&connection, &session, "Leaving.")?;
        std::process::exit(3);
    }
    if text.contains("slow") {
        say(&connection, &session, "Thinking about it. ")?;
        // Stays here until the client cancels, as an agent in the middle of a long turn.
        cancel.recv().await.map_err(|_| Error::internal_error())?;
        return responder.respond(PromptResponse::new(StopReason::Cancelled));
    }
    if text.contains("echo") {
        let facts = facts.lock().map(|facts| facts.clone()).unwrap_or_default();
        say(&connection, &session, &format!("Session facts: {facts}"))?;
        return responder.respond(PromptResponse::new(StopReason::EndTurn));
    }
    if text.contains("mcp") || text.contains("write") {
        let (tool, arguments) = if text.contains("write") {
            (
                "write_cells",
                serde_json::json!({"workbook": 0, "sheet": "Sheet1", "start": "A1", "rows": [["agent wrote"]]}),
            )
        } else {
            ("list_workbooks", serde_json::json!({}))
        };
        say(&connection, &session, "Calling the workbook tools.\n\n")?;
        let answer = match mcp {
            Some((program, env)) => call_over_mcp(program, env, tool, arguments),
            None => "No MCP server was passed in the session.".to_string(),
        };
        say(&connection, &session, &answer)?;
        return responder.respond(PromptResponse::new(StopReason::EndTurn));
    }
    say(&connection, &session, "Looking at **your request**. ")?;
    connection.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::ToolCall(
            ToolCall::new("call-1", "read_range")
                .kind(ToolKind::Read)
                .status(ToolCallStatus::InProgress),
        ),
    ))?;
    let decision = connection
        .send_request(RequestPermissionRequest::new(
            session.clone(),
            ToolCallUpdate::new(
                "call-1",
                ToolCallUpdateFields::new().title("read_range on Sheet1"),
            ),
            vec![
                PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        ))
        .block_task()
        .await?;
    let allowed = matches!(
        decision.outcome,
        RequestPermissionOutcome::Selected(chosen) if &*chosen.option_id.0 == "allow"
    );
    let status = if allowed {
        ToolCallStatus::Completed
    } else {
        ToolCallStatus::Failed
    };
    connection.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new().status(status),
        )),
    ))?;
    say(
        &connection,
        &session,
        if allowed {
            "Done: the range holds 3 values."
        } else {
            "Understood, I did not read it."
        },
    )?;
    connection.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::UsageUpdate(UsageUpdate::new(1_200, 10_000)),
    ))?;
    responder.respond(PromptResponse::new(StopReason::EndTurn))
}

fn zenkai_folder(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(name)
}

type Choices = Arc<Mutex<(String, String)>>;

fn config_options(model: &str, effort: &str) -> Vec<SessionConfigOption> {
    let choices =
        |values: &'static [(&'static str, &'static str)]| -> Vec<SessionConfigSelectOption> {
            values
                .iter()
                .map(|(value, name)| SessionConfigSelectOption::new(*value, *name))
                .collect()
        };
    vec![
        SessionConfigOption::select(
            "model",
            "Model",
            model.to_string(),
            choices(&[("fast", "Fast model"), ("deep", "Deep model")]),
        )
        .category(SessionConfigOptionCategory::Model),
        SessionConfigOption::select(
            "effort",
            "Reasoning effort",
            effort.to_string(),
            choices(&[("low", "Low"), ("high", "High")]),
        )
        .category(SessionConfigOptionCategory::ThoughtLevel),
    ]
}

fn modes() -> SessionModeState {
    SessionModeState::new(
        "ask",
        vec![
            SessionMode::new("ask", "Ask"),
            SessionMode::new("plan", "Plan"),
        ],
    )
}

fn announce_commands(connection: &ConnectionTo<Client>, session: &SessionId) -> Result<(), Error> {
    connection.send_notification(SessionNotification::new(
        session.clone(),
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![
            AvailableCommand::new("compact", "Summarize the conversation"),
            AvailableCommand::new("review", "Review the open workbook"),
        ])),
    ))
}

async fn serve(mode: Mode) -> Result<(), Error> {
    let facts: Facts = Facts::default();
    let mcp: Arc<Mutex<Option<McpCommand>>> = Arc::default();
    let (cancel_send, cancel_receive) = async_channel::unbounded::<()>();
    let new_session_facts = facts.clone();
    let new_session_mcp = mcp.clone();
    let prompt_facts = facts.clone();
    let choices: Choices = Arc::new(Mutex::new(("fast".to_string(), "low".to_string())));
    let new_session_choices = choices.clone();
    let config_choices = choices.clone();
    Agent
        .builder()
        .name("fake-acp-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                let fs = &request.client_capabilities.fs;
                if fs.read_text_file || fs.write_text_file || request.client_capabilities.terminal {
                    return responder.respond_with_internal_error(
                        "the client advertised file-system or terminal capabilities",
                    );
                }
                if mode == Mode::Grandchild
                    && let Err(error) = start_grandchild()
                {
                    return responder.respond_with_internal_error(error);
                }
                responder.respond(
                    InitializeResponse::new(ProtocolVersion::V1)
                        .agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(true)
                                .session_capabilities(
                                    SessionCapabilities::new()
                                        .list(SessionListCapabilities::default()),
                                ),
                        )
                        .agent_info(
                            Implementation::new("fake-acp-agent", "0.1.0").title("Fake agent"),
                        ),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, connection| {
                if mode == Mode::AuthRequired {
                    return responder.respond_with_error(Error::auth_required());
                }
                if let Ok(mut facts) = new_session_facts.lock() {
                    *facts = describe_session(&request);
                }
                if let Ok(mut slot) = new_session_mcp.lock() {
                    *slot = stdio_server(&request);
                }
                let (model, effort) = new_session_choices
                    .lock()
                    .map(|c| c.clone())
                    .unwrap_or_default();
                announce_commands(&connection, &SessionId::new("session-1"))?;
                responder.respond(
                    NewSessionResponse::new("session-1")
                        .config_options(config_options(&model, &effort))
                        .modes(modes()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let text = prompt_text(&request);
                let session = request.session_id.clone();
                let facts = prompt_facts.clone();
                let cancel = cancel_receive.clone();
                let mcp = mcp.lock().ok().and_then(|slot| slot.clone());
                let task_connection = connection.clone();
                connection.spawn(run_prompt(
                    task_connection,
                    responder,
                    session,
                    text,
                    facts,
                    cancel,
                    mcp,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                let value = match &request.value {
                    SessionConfigOptionValue::ValueId { value } => value.0.to_string(),
                    _ => String::new(),
                };
                let (model, effort) = {
                    let Ok(mut current) = config_choices.lock() else {
                        return responder.respond_with_internal_error("poisoned");
                    };
                    match &*request.config_id.0 {
                        "model" => current.0 = value,
                        _ => current.1 = value,
                    }
                    current.clone()
                };
                responder.respond(SetSessionConfigOptionResponse::new(config_options(
                    &model, &effort,
                )))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: SetSessionModeRequest, responder, _connection| {
                responder.respond(SetSessionModeResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: ListSessionsRequest, responder, _connection| {
                responder.respond(ListSessionsResponse::new(vec![
                    SessionInfo::new("old-1", zenkai_folder("old-1")).title("Fix the dates"),
                    SessionInfo::new("foreign", "C:/other/project").title("Someone else"),
                    SessionInfo::new("old-2", zenkai_folder("old-2")).title("Chart of sales"),
                ]))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, connection| {
                connection.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::from(
                        "[Zenkai] The user is looking at the workbook",
                    ))),
                ))?;
                connection.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::from(
                        "Earlier question",
                    ))),
                ))?;
                say(&connection, &request.session_id, "Earlier answer")?;
                responder.respond(LoadSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |_cancel: CancelNotification, _connection| {
                cancel_send
                    .send(())
                    .await
                    .map_err(|_| Error::internal_error())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}
