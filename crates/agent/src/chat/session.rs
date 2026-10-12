use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, EnvVariable, Implementation,
    InitializeRequest, ListSessionsRequest, LoadSessionRequest, McpServer, McpServerStdio,
    NewSessionRequest, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigValueId, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, ToolCallContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode};
use async_channel::{Receiver, Sender};
use async_process::{Command as ProcessCommand, Stdio};
use futures::channel::oneshot;
use futures::{AsyncBufReadExt, StreamExt};
use zenkai_i18n::t;

use crate::bridge::{PIPE_VARIABLE, TOKEN_VARIABLE};
use crate::chat::install::install;
use crate::chat::instructions::{self, PromptRoute, SYSTEM_PROMPT};
use crate::chat::launch::{self, INSTALLED_MARKER, LaunchError, LaunchPlan};
use crate::chat::process::ProcessTree;
use crate::chat::state::{ConfigId, ConfigSource, StateChange};
use crate::chat::thread::{AgentUpdate, ToolCallId, ToolCard, ToolKind, ToolStatus, TurnEnd};
use crate::chat::wire;

// The hidden first block of every prompt; replayed history must not show it as the user's words.
pub const CONTEXT_MARKER: &str = "[Zenkai]";
// Past this many unread events an agent that floods the chat loses the overflow instead of
// growing memory without limit.
const EVENT_BACKLOG: usize = 4096;
const MAX_ASK_TITLE_CHARS: usize = 200;
const MAX_OPTION_LABEL_CHARS: usize = 100;
const MAX_OPTIONS: usize = 6;
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
    // The agent's working folder. It belongs to the user and is never removed.
    pub folder: PathBuf,
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
    State(StateChange),
    // Something the user asked for failed, but the conversation goes on.
    Problem(String),
    Resumed,
    // The id of a conversation this chat started, so its past sessions can be told apart
    // from the user's other sessions in the same folder.
    Started(String),
}

#[derive(Debug)]
pub enum Command {
    Prompt {
        text: String,
        context: Option<String>,
    },
    Cancel,
    SetConfig {
        id: ConfigId,
        source: ConfigSource,
        value: String,
    },
    ListSessions(BTreeSet<String>),
    Resume(String),
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

    pub fn set_config(&self, id: ConfigId, source: ConfigSource, value: String) {
        self.send(Command::SetConfig { id, source, value });
    }

    // Only the sessions in `started` are listed: the agent reports every session the user
    // ever had, in any folder.
    pub fn list_sessions(&self, started: BTreeSet<String>) {
        self.send(Command::ListSessions(started));
    }

    pub fn resume(&self, session: String) {
        self.send(Command::Resume(session));
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
    let (events, event_stream) = async_channel::bounded(EVENT_BACKLOG);
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

// The end of a turn and of the session must reach the chat even when the queue is full; the
// sender waits for room instead of dropping them.
async fn deliver(events: &Sender<SessionEvent>, event: SessionEvent) {
    if events.send(event).await.is_err() {
        tracing::debug!("the chat closed before an event arrived");
    }
}

fn limit(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let mut shortened: String = text.chars().take(max).collect();
    shortened.push('…');
    shortened
}

fn notify(events: &Sender<SessionEvent>, event: SessionEvent) {
    if events.try_send(event).is_err() {
        tracing::debug!("the chat closed before an event arrived");
    }
}

#[cfg(windows)]
pub(super) fn hide_window(command: &mut ProcessCommand) {
    use async_process::windows::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(super) fn hide_window(_: &mut ProcessCommand) {}

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
            if !folder.join(INSTALLED_MARKER).is_file() {
                notify(events, SessionEvent::Connection(Connection::Installing));
                install(node, package, folder, tree).await?;
            }
            // A finished install with a broken package is reported, not silently redone.
            let entry = launch::package_entry(folder, package)?;
            let mut arguments = vec![entry.to_string_lossy().into_owned()];
            arguments.extend(args.iter().cloned());
            Ok((node.clone(), arguments))
        }
        LaunchPlan::Native {
            node,
            package,
            folder,
            args,
            relative,
            ..
        } => {
            if !folder.join(INSTALLED_MARKER).is_file() {
                notify(events, SessionEvent::Connection(Connection::Installing));
                install(node, package, folder, tree).await?;
            }
            // The plan only names the binary; it must be there now that the install finished.
            let program = launch::native_program(folder, package, relative)?;
            Ok((program, args.clone()))
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
    let (program, arguments) = resolve(&config.plan, events, tree).await?;
    if tree.is_closed() {
        return Ok(());
    }
    notify(events, SessionEvent::Connection(Connection::Starting));
    let mut command = ProcessCommand::new(&program);
    command
        .args(&arguments)
        .envs(config.env.iter().map(|(name, value)| (name, value)))
        .current_dir(&config.folder)
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
            config.folder.clone(),
            ByteStreams::new(stdin, stdout),
            commands,
            events,
            &tail,
        )
        .await;
        tree.close();
        result
    };
    // Once the child is reaped its pid may be reused, so it must never be killed again.
    let reaped = async {
        if let Err(error) = child.status().await {
            tracing::debug!(%error, "could not wait for the agent to exit");
        }
        tree.forget();
    };
    let (result, (), ()) = futures::join!(talk, drain, reaped);
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
                for event in incoming(notification.update) {
                    notify(&for_updates, event);
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
                    title: limit(
                        request.tool_call.fields.title.clone().unwrap_or_default(),
                        MAX_ASK_TITLE_CHARS,
                    ),
                    choices: request
                        .options
                        .iter()
                        .take(MAX_OPTIONS)
                        .map(|option| PermissionChoice {
                            id: option.option_id.to_string(),
                            label: limit(option.name.clone(), MAX_OPTION_LABEL_CHARS),
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
    let started = std::time::Instant::now();
    let initialized = connection
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(capabilities)
                .client_info(Implementation::new("zenkai", env!("CARGO_PKG_VERSION"))),
        )
        .block_task()
        .await?;
    tracing::info!(elapsed = ?started.elapsed(), "the agent answered initialize");
    let abilities = wire::abilities(&initialized);
    let route = instructions::prompt_route(initialized.agent_info.as_ref());
    let mut introduced = route == PromptRoute::SessionMeta;
    let agent = initialized
        .agent_info
        .map(|info| info.title.unwrap_or(info.name))
        .unwrap_or_else(|| t!("chat.agent").to_string());
    let servers = mcp_servers(relay);
    let session = connection
        .send_request(
            NewSessionRequest::new(folder.clone())
                .mcp_servers(servers.clone())
                .meta(instructions::session_meta()),
        )
        .block_task()
        .await?;
    tracing::info!(elapsed = ?started.elapsed(), "the agent created the session");
    let mut session_id = session.session_id;
    notify(events, SessionEvent::Started(session_id.to_string()));
    notify(
        events,
        SessionEvent::State(StateChange::Abilities(abilities)),
    );
    let selects =
        wire::selects_from_session(session.config_options.as_deref(), session.modes.as_ref());
    let unguarded = wire::leave_unguarded_mode(&selects);
    notify(events, SessionEvent::State(StateChange::Selects(selects)));
    if let Some((id, source, value)) = unguarded {
        notify(
            events,
            change_config(&connection, &session_id, id, source, value).await,
        );
    }
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
                let introduction =
                    (!introduced).then(|| format!("{CONTEXT_MARKER} {SYSTEM_PROMPT}"));
                introduced = true;
                let task_connection = connection.clone();
                let task_events = events.clone();
                let task_session = session_id.clone();
                connection.spawn(async move {
                    let result = task_connection
                        .send_request(PromptRequest::new(
                            task_session,
                            introduction
                                .into_iter()
                                .chain(context)
                                .chain(std::iter::once(text))
                                .map(ContentBlock::from)
                                .collect(),
                        ))
                        .block_task()
                        .await;
                    let end = match result {
                        Ok(response) => turn_end(response.stop_reason),
                        Err(error) if error.code == ErrorCode::AuthRequired => {
                            deliver(&task_events, SessionEvent::AuthRequired).await;
                            TurnEnd::Failed(t!("chat.turn.sign_in").to_string())
                        }
                        Err(error) => TurnEnd::Failed(error.message),
                    };
                    deliver(&task_events, SessionEvent::TurnEnded(end)).await;
                    Ok(())
                })?;
            }
            Command::Cancel => {
                connection.send_notification(CancelNotification::new(session_id.clone()))?;
            }
            Command::SetConfig { id, source, value } => {
                notify(
                    events,
                    change_config(&connection, &session_id, id, source, value).await,
                );
            }
            Command::ListSessions(started) => {
                match connection
                    .send_request(ListSessionsRequest::new())
                    .block_task()
                    .await
                {
                    Ok(response) => notify(
                        events,
                        SessionEvent::State(StateChange::PastSessions(
                            response
                                .sessions
                                .iter()
                                .filter(|info| started.contains(&info.session_id.to_string()))
                                .map(wire::past_session)
                                .collect(),
                        )),
                    ),
                    Err(error) => notify(
                        events,
                        SessionEvent::Problem(t!(
                            "chat.problem.list_sessions",
                            error = error.message
                        )),
                    ),
                }
            }
            Command::Resume(id) => {
                let loaded = connection
                    .send_request(
                        LoadSessionRequest::new(id.clone(), folder.clone())
                            .mcp_servers(servers.clone())
                            .meta(instructions::session_meta()),
                    )
                    .block_task()
                    .await;
                match loaded {
                    Ok(response) => {
                        session_id = SessionId::new(id.clone());
                        introduced = true;
                        let selects = wire::selects_from_session(
                            response.config_options.as_deref(),
                            response.modes.as_ref(),
                        );
                        // An agent that sends none keeps the pickers it had.
                        if !selects.is_empty() {
                            notify(events, SessionEvent::State(StateChange::Selects(selects)));
                        }
                        notify(events, SessionEvent::Resumed);
                    }
                    Err(error) => notify(
                        events,
                        SessionEvent::Problem(t!(
                            "chat.problem.resume_session",
                            error = error.message
                        )),
                    ),
                }
            }
        }
    }
}

async fn change_config(
    connection: &ConnectionTo<Agent>,
    session_id: &SessionId,
    id: ConfigId,
    source: ConfigSource,
    value: String,
) -> SessionEvent {
    let outcome = match source {
        ConfigSource::ConfigOption => connection
            .send_request(SetSessionConfigOptionRequest::new(
                session_id.clone(),
                id.as_str().to_string(),
                SessionConfigValueId::new(value.clone()),
            ))
            .block_task()
            .await
            .map(|response| {
                StateChange::Selects(wire::selects_from_options(&response.config_options))
            }),
        ConfigSource::LegacyMode => connection
            .send_request(SetSessionModeRequest::new(
                session_id.clone(),
                value.clone(),
            ))
            .block_task()
            .await
            .map(|_| StateChange::CurrentMode(value.clone())),
    };
    match outcome {
        Ok(change) => SessionEvent::State(change),
        Err(error) => {
            SessionEvent::Problem(t!("chat.problem.change_refused", error = error.message))
        }
    }
}

fn mcp_servers(relay: Option<McpRelay>) -> Vec<McpServer> {
    relay
        .map(|relay| {
            McpServer::Stdio(McpServerStdio::new("zenkai", relay.program).env(vec![
                EnvVariable::new(PIPE_VARIABLE, relay.pipe),
                EnvVariable::new(TOKEN_VARIABLE, relay.token),
            ]))
        })
        .into_iter()
        .collect()
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

fn incoming(update: SessionUpdate) -> Vec<SessionEvent> {
    let state = |change| vec![SessionEvent::State(change)];
    match update {
        SessionUpdate::UserMessageChunk(chunk)
            if block_text(&chunk.content).starts_with(CONTEXT_MARKER) =>
        {
            Vec::new()
        }
        SessionUpdate::AvailableCommandsUpdate(update) => state(StateChange::Commands(
            wire::commands(&update.available_commands),
        )),
        SessionUpdate::UsageUpdate(update) => state(StateChange::Usage(wire::usage(&update))),
        SessionUpdate::ConfigOptionUpdate(update) => state(StateChange::Selects(
            wire::selects_from_options(&update.config_options),
        )),
        SessionUpdate::CurrentModeUpdate(update) => state(StateChange::CurrentMode(
            update.current_mode_id.0.to_string(),
        )),
        other => agent_update(other)
            .map(|update| vec![SessionEvent::Update(update)])
            .unwrap_or_default(),
    }
}

fn agent_update(update: SessionUpdate) -> Option<AgentUpdate> {
    match update {
        SessionUpdate::UserMessageChunk(chunk) => {
            Some(AgentUpdate::UserText(block_text(&chunk.content)))
        }
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
            input: call
                .raw_input
                .as_ref()
                .map(|input| input.to_string())
                .unwrap_or_default(),
        })),
        SessionUpdate::ToolCallUpdate(update) => Some(AgentUpdate::ToolChanged {
            id: ToolCallId::new(update.tool_call_id.to_string()),
            title: update.fields.title,
            status: update.fields.status.map(tool_status),
            detail: update.fields.content.as_deref().map(content_detail),
            input: update
                .fields
                .raw_input
                .as_ref()
                .map(|input| input.to_string()),
        }),
        _ => None,
    }
}
