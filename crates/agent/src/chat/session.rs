use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, EnvVariable, Implementation,
    InitializeRequest, McpServer, McpServerStdio, NewSessionRequest, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason, ToolCallContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode};
use async_channel::{Receiver, Sender};
use async_process::{Command as ProcessCommand, Stdio};
use futures::channel::oneshot;
use futures::{AsyncBufReadExt, StreamExt};

use crate::bridge::{PIPE_VARIABLE, TOKEN_VARIABLE};
use crate::chat::launch::{self, LaunchError, LaunchPlan, PackageSpec};
use crate::chat::process::{ProcessError, ProcessTree, WorkFolder};
use crate::chat::thread::{AgentUpdate, ToolCallId, ToolCard, ToolKind, ToolStatus, TurnEnd};

const STDERR_LINES_KEPT: usize = 20;
const MAX_CHUNK_CHARS: usize = 200_000;

#[derive(Clone, Debug)]
pub struct McpRelay {
    pub program: PathBuf,
    pub pipe: String,
    pub token: String,
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub plan: LaunchPlan,
    pub env: Vec<(String, String)>,
    pub relay: Option<McpRelay>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    Installing,
    Starting,
    Ready { agent: String },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Launch(#[from] LaunchError),
    #[error("could not prepare the agent: {0}")]
    Prepare(String),
    #[error("installing the agent package failed: {0}")]
    Install(String),
    #[error("could not start the agent: {0}")]
    Spawn(String),
    #[error("the agent stopped{}", stderr_suffix(.stderr))]
    Exited { stderr: String },
    #[error("the agent needs you to sign in")]
    AuthRequired,
    #[error("the agent answered something Zenkai does not understand: {0}")]
    Protocol(String),
}

fn stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

impl From<ProcessError> for SessionError {
    fn from(error: ProcessError) -> SessionError {
        SessionError::Prepare(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

impl ChoiceKind {
    pub fn allows(self) -> bool {
        matches!(self, ChoiceKind::AllowOnce | ChoiceKind::AllowAlways)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionChoice {
    pub id: String,
    pub label: String,
    pub kind: ChoiceKind,
}

pub struct PermissionAsk {
    pub tool: ToolCallId,
    pub title: String,
    pub choices: Vec<PermissionChoice>,
    reply: oneshot::Sender<Option<String>>,
}

impl PermissionAsk {
    pub fn choose(self, choice: &PermissionChoice) {
        if self.reply.send(Some(choice.id.clone())).is_err() {
            tracing::debug!("the agent stopped waiting for a permission answer");
        }
    }

    pub fn cancel(self) {
        if self.reply.send(None).is_err() {
            tracing::debug!("the agent stopped waiting for a permission answer");
        }
    }
}

pub enum SessionEvent {
    Connection(Connection),
    Update(AgentUpdate),
    Permission(PermissionAsk),
    TurnEnded(TurnEnd),
    AuthRequired,
    Failed(SessionError),
    Closed,
}

#[derive(Debug)]
pub enum Command {
    Prompt {
        text: String,
        context: Option<String>,
    },
    Cancel,
}

#[derive(Clone)]
pub struct SessionHandle {
    commands: Sender<Command>,
    tree: Arc<ProcessTree>,
}

impl SessionHandle {
    // `context` is read by the agent but not shown in the chat: which workbook is active.
    pub fn prompt(&self, text: String, context: Option<String>) {
        self.send(Command::Prompt { text, context });
    }

    pub fn cancel(&self) {
        self.send(Command::Cancel);
    }

    fn send(&self, command: Command) {
        if self.commands.try_send(command).is_err() {
            tracing::debug!("the chat session is already closed");
        }
    }

    pub fn process_tree(&self) -> Arc<ProcessTree> {
        self.tree.clone()
    }
}

// The caller runs the returned future on a background executor and reads the events on the
// foreground. Ending the commands channel or closing the tree ends the session.
pub fn start(
    config: SessionConfig,
) -> (
    SessionHandle,
    Receiver<SessionEvent>,
    impl Future<Output = ()> + Send + 'static,
) {
    let (commands, received) = async_channel::unbounded();
    let (events, event_stream) = async_channel::unbounded();
    let tree = Arc::new(ProcessTree::default());
    let handle = SessionHandle {
        commands,
        tree: tree.clone(),
    };
    let run = run(config, received, events, tree);
    (handle, event_stream, run)
}

async fn run(
    config: SessionConfig,
    commands: Receiver<Command>,
    events: Sender<SessionEvent>,
    tree: Arc<ProcessTree>,
) {
    let outcome = drive(&config, &commands, &events, &tree).await;
    tree.close();
    let last = match outcome {
        Ok(()) => SessionEvent::Closed,
        Err(error) => SessionEvent::Failed(error),
    };
    if events.send(last).await.is_err() {
        tracing::debug!("the chat closed before the session ended");
    }
}

fn notify(events: &Sender<SessionEvent>, event: SessionEvent) {
    if events.try_send(event).is_err() {
        tracing::debug!("the chat closed before an event arrived");
    }
}

async fn install(
    node: &std::path::Path,
    package: &PackageSpec,
    folder: &std::path::Path,
    tree: &ProcessTree,
) -> Result<(), SessionError> {
    std::fs::create_dir_all(folder)
        .map_err(|error| SessionError::Install(format!("{}: {error}", folder.display())))?;
    let mut command = ProcessCommand::new(node);
    command
        .arg(launch::npm_cli(node))
        .args(["install", "--prefix"])
        .arg(folder)
        .args([
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
        ])
        .arg(package.spec())
        .env("npm_config_update_notifier", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_window(&mut command);
    let child = command
        .spawn()
        .map_err(|error| SessionError::Install(error.to_string()))?;
    tree.register(child.id());
    tracing::info!(package = %package.spec(), "installing the agent package");
    let output = child
        .output()
        .await
        .map_err(|error| SessionError::Install(error.to_string()))?;
    tree.forget();
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
        let reason = tail.into_iter().rev().collect::<Vec<_>>().join(" ");
        Err(SessionError::Install(reason))
    }
}

#[cfg(windows)]
fn hide_window(command: &mut ProcessCommand) {
    use async_process::windows::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_: &mut ProcessCommand) {}

async fn resolve(
    plan: &LaunchPlan,
    events: &Sender<SessionEvent>,
    tree: &ProcessTree,
) -> Result<(PathBuf, Vec<String>), SessionError> {
    match plan {
        LaunchPlan::Direct { program, args } => Ok((program.clone(), args.clone())),
        LaunchPlan::Package {
            node,
            package,
            folder,
            args,
        } => {
            let entry = match launch::package_entry(folder, package) {
                Ok(entry) => entry,
                Err(_) => {
                    notify(events, SessionEvent::Connection(Connection::Installing));
                    install(node, package, folder, tree).await?;
                    launch::package_entry(folder, package)?
                }
            };
            let mut arguments = vec![entry.to_string_lossy().into_owned()];
            arguments.extend(args.iter().cloned());
            Ok((node.clone(), arguments))
        }
    }
}

type StderrTail = Arc<Mutex<VecDeque<String>>>;

fn tail_text(tail: &StderrTail) -> String {
    tail.lock()
        .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

async fn drive(
    config: &SessionConfig,
    commands: &Receiver<Command>,
    events: &Sender<SessionEvent>,
    tree: &Arc<ProcessTree>,
) -> Result<(), SessionError> {
    let work = WorkFolder::create()?;
    let (program, arguments) = resolve(&config.plan, events, tree).await?;
    if tree.is_closed() {
        return Ok(());
    }
    notify(events, SessionEvent::Connection(Connection::Starting));
    let mut command = ProcessCommand::new(&program);
    command
        .args(&arguments)
        .envs(config.env.iter().map(|(name, value)| (name, value)))
        .current_dir(work.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_window(&mut command);
    tracing::info!(program = %program.display(), ?arguments, "starting the agent");
    let mut child = command
        .spawn()
        .map_err(|error| SessionError::Spawn(format!("{}: {error}", program.display())))?;
    tree.register(child.id());
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(SessionError::Spawn(
            "the agent has no standard streams".into(),
        ));
    };
    let tail: StderrTail = Arc::default();
    let drain = {
        let tail = tail.clone();
        async move {
            let mut lines = futures::io::BufReader::new(stderr).lines();
            while let Some(Ok(line)) = lines.next().await {
                tracing::debug!(%line, "agent stderr");
                if let Ok(mut kept) = tail.lock() {
                    if kept.len() == STDERR_LINES_KEPT {
                        kept.pop_front();
                    }
                    kept.push_back(line);
                }
            }
        }
    };
    let talk = async {
        let result = converse(
            config,
            work.path().to_path_buf(),
            ByteStreams::new(stdin, stdout),
            commands,
            events,
            &tail,
        )
        .await;
        tree.close();
        result
    };
    let (result, ()) = futures::join!(talk, drain);
    drop(child);
    result
}

async fn converse(
    config: &SessionConfig,
    folder: PathBuf,
    transport: ByteStreams<async_process::ChildStdin, async_process::ChildStdout>,
    commands: &Receiver<Command>,
    events: &Sender<SessionEvent>,
    tail: &StderrTail,
) -> Result<(), SessionError> {
    let for_updates = events.clone();
    let for_permissions = events.clone();
    let builder = Client
        .builder()
        .name("zenkai")
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                if let Some(update) = agent_update(notification.update) {
                    notify(&for_updates, SessionEvent::Update(update));
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, connection| {
                let (reply, answered) = oneshot::channel();
                let ask = PermissionAsk {
                    tool: ToolCallId::new(request.tool_call.tool_call_id.to_string()),
                    title: request.tool_call.fields.title.clone().unwrap_or_default(),
                    choices: request
                        .options
                        .iter()
                        .map(|option| PermissionChoice {
                            id: option.option_id.to_string(),
                            label: option.name.clone(),
                            kind: choice_kind(option.kind),
                        })
                        .collect(),
                    reply,
                };
                notify(&for_permissions, SessionEvent::Permission(ask));
                // The dispatch loop must keep streaming while the user decides.
                connection.spawn(async move {
                    let outcome = match answered.await {
                        Ok(Some(id)) => {
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
                        }
                        Ok(None) | Err(_) => RequestPermissionOutcome::Cancelled,
                    };
                    responder.respond(RequestPermissionResponse::new(outcome))
                })
            },
            agent_client_protocol::on_receive_request!(),
        );
    let relay = config.relay.clone();
    let outcome = builder
        .connect_with(transport, async |connection: ConnectionTo<Agent>| {
            converse_on(connection, folder, relay, commands, events).await
        })
        .await;
    match outcome {
        Ok(result) => result,
        Err(error) => Err(session_error(&error, tail)),
    }
}

fn session_error(error: &agent_client_protocol::Error, tail: &StderrTail) -> SessionError {
    if error.code == ErrorCode::AuthRequired {
        SessionError::AuthRequired
    } else if agent_client_protocol::is_incoming_transport_closed(error) {
        SessionError::Exited {
            stderr: tail_text(tail),
        }
    } else {
        SessionError::Protocol(error.message.clone())
    }
}

async fn converse_on(
    connection: ConnectionTo<Agent>,
    folder: PathBuf,
    relay: Option<McpRelay>,
    commands: &Receiver<Command>,
    events: &Sender<SessionEvent>,
) -> Result<Result<(), SessionError>, agent_client_protocol::Error> {
    // No file-system or terminal capability: the agent reaches the workbook only through
    // the MCP tools Zenkai passes in the session.
    let capabilities = ClientCapabilities::new();
    let initialized = connection
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(capabilities)
                .client_info(Implementation::new("zenkai", env!("CARGO_PKG_VERSION"))),
        )
        .block_task()
        .await?;
    let agent = initialized
        .agent_info
        .map(|info| info.title.unwrap_or(info.name))
        .unwrap_or_else(|| "Agent".to_string());
    let mut request = NewSessionRequest::new(folder);
    if let Some(relay) = relay {
        let server = McpServerStdio::new("zenkai", relay.program).env(vec![
            EnvVariable::new(PIPE_VARIABLE, relay.pipe),
            EnvVariable::new(TOKEN_VARIABLE, relay.token),
        ]);
        request = request.mcp_servers(vec![McpServer::Stdio(server)]);
    }
    let session = connection.send_request(request).block_task().await?;
    let session_id = session.session_id;
    notify(
        events,
        SessionEvent::Connection(Connection::Ready { agent }),
    );
    loop {
        let next = futures::future::select(
            Box::pin(commands.recv()),
            Box::pin(connection.incoming_closed()),
        )
        .await;
        let command = match next {
            futures::future::Either::Left((Ok(command), _)) => command,
            futures::future::Either::Left((Err(_), _)) => return Ok(Ok(())),
            futures::future::Either::Right(_) => {
                return Ok(Err(SessionError::Exited {
                    stderr: String::new(),
                }));
            }
        };
        match command {
            Command::Prompt { text, context } => {
                let task_connection = connection.clone();
                let task_events = events.clone();
                let task_session = session_id.clone();
                connection.spawn(async move {
                    let result = task_connection
                        .send_request(PromptRequest::new(
                            task_session,
                            context
                                .into_iter()
                                .chain(std::iter::once(text))
                                .map(ContentBlock::from)
                                .collect(),
                        ))
                        .block_task()
                        .await;
                    let end = match result {
                        Ok(response) => turn_end(response.stop_reason),
                        Err(error) if error.code == ErrorCode::AuthRequired => {
                            notify(&task_events, SessionEvent::AuthRequired);
                            TurnEnd::Failed(
                                "The agent needs you to sign in before it can answer.".to_string(),
                            )
                        }
                        Err(error) => TurnEnd::Failed(error.message),
                    };
                    notify(&task_events, SessionEvent::TurnEnded(end));
                    Ok(())
                })?;
            }
            Command::Cancel => {
                connection.send_notification(CancelNotification::new(session_id.clone()))?;
            }
        }
    }
}

fn turn_end(reason: StopReason) -> TurnEnd {
    match reason {
        StopReason::EndTurn => TurnEnd::Finished,
        StopReason::MaxTokens | StopReason::MaxTurnRequests => TurnEnd::TokenLimit,
        StopReason::Refusal => TurnEnd::Refused,
        StopReason::Cancelled => TurnEnd::Cancelled,
        _ => TurnEnd::Finished,
    }
}

fn choice_kind(kind: PermissionOptionKind) -> ChoiceKind {
    match kind {
        PermissionOptionKind::AllowOnce => ChoiceKind::AllowOnce,
        PermissionOptionKind::AllowAlways => ChoiceKind::AllowAlways,
        PermissionOptionKind::RejectAlways => ChoiceKind::RejectAlways,
        _ => ChoiceKind::RejectOnce,
    }
}

fn block_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.chars().take(MAX_CHUNK_CHARS).collect(),
        ContentBlock::Image(_) => "[image]".to_string(),
        ContentBlock::Audio(_) => "[audio]".to_string(),
        ContentBlock::ResourceLink(link) => link.uri.clone(),
        ContentBlock::Resource(_) => "[resource]".to_string(),
        _ => String::new(),
    }
}

fn tool_kind(kind: agent_client_protocol::schema::v1::ToolKind) -> ToolKind {
    use agent_client_protocol::schema::v1::ToolKind as Wire;
    match kind {
        Wire::Read => ToolKind::Read,
        Wire::Edit => ToolKind::Edit,
        Wire::Delete => ToolKind::Delete,
        Wire::Move => ToolKind::Move,
        Wire::Search => ToolKind::Search,
        Wire::Execute => ToolKind::Execute,
        Wire::Think => ToolKind::Think,
        Wire::Fetch => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

fn tool_status(status: agent_client_protocol::schema::v1::ToolCallStatus) -> ToolStatus {
    use agent_client_protocol::schema::v1::ToolCallStatus as Wire;
    match status {
        Wire::Pending => ToolStatus::Pending,
        Wire::InProgress => ToolStatus::InProgress,
        Wire::Completed => ToolStatus::Completed,
        Wire::Failed => ToolStatus::Failed,
        _ => ToolStatus::InProgress,
    }
}

fn content_detail(content: &[ToolCallContent]) -> String {
    content
        .iter()
        .map(|item| match item {
            ToolCallContent::Content(content) => block_text(&content.content),
            ToolCallContent::Diff(diff) => format!("changes {}", diff.path.display()),
            ToolCallContent::Terminal(_) => "terminal output".to_string(),
            _ => String::new(),
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn agent_update(update: SessionUpdate) -> Option<AgentUpdate> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            Some(AgentUpdate::Message(block_text(&chunk.content)))
        }
        SessionUpdate::AgentThoughtChunk(_) => Some(AgentUpdate::Thought),
        SessionUpdate::ToolCall(call) => Some(AgentUpdate::ToolStarted(ToolCard {
            id: ToolCallId::new(call.tool_call_id.to_string()),
            title: call.title,
            kind: tool_kind(call.kind),
            status: tool_status(call.status),
            detail: content_detail(&call.content),
        })),
        SessionUpdate::ToolCallUpdate(update) => Some(AgentUpdate::ToolChanged {
            id: ToolCallId::new(update.tool_call_id.to_string()),
            title: update.fields.title,
            status: update.fields.status.map(tool_status),
            detail: update.fields.content.as_deref().map(content_detail),
        }),
        _ => None,
    }
}
