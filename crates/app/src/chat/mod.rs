mod access;
mod badge;
mod cards;
mod composer;
mod events;
mod extras;
mod files;
mod header;
mod launch;
mod menus;
mod permissions;
pub(crate) mod reference;
mod reference_scan;
mod render;
mod session_rows;
mod sessions;
mod slash;
mod startup;
mod transcript;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Instant, SystemTime};

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use zenkai_agent::chat::history::History;
use zenkai_agent::chat::session::{CONTEXT_MARKER, PermissionAsk};
use zenkai_agent::chat::state::AgentState;
use zenkai_agent::chat::thread::{MessageId, Thread, TurnEnd, TurnState};
use zenkai_agent::settings::AgentId;
use zenkai_agent::tools::ToolEndpoint;
use zenkai_types::WorkbookId;

use crate::agent_settings::{self, AgentConfig};
use crate::view::Workspace;
use access::Access;
use launch::{Live, Prepared};
use reference::Reference;
use zenkai_i18n::t;

const SCROLL_FOLLOW_SLACK: f32 = 48.0;
const MAX_NAME_CHARS: usize = 80;
const MAX_PENDING_ASKS: usize = 8;
const MONO: &str = "IBM Plex Mono";

pub enum ChatEvent {
    Leave,
    NeedsAnswer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Chat,
    Sessions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Start {
    Message,
    WarmUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Model,
    Access,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Link {
    Idle,
    Preparing,
    Confirming,
    Installing,
    Starting,
    Ready(SharedString),
}

struct Problem {
    text: SharedString,
    login: Option<&'static str>,
}

struct Backup {
    thread: Thread,
    texts: BTreeMap<MessageId, Entity<TextViewState>>,
    title: Option<String>,
}

struct Gate {
    prepared: Prepared,
    focus: FocusHandle,
}

pub struct ChatPanel {
    workspace: WeakEntity<Workspace>,
    endpoint: ToolEndpoint,
    composer: Entity<TextareaState>,
    scroll: ScrollHandle,
    thread: Thread,
    texts: BTreeMap<MessageId, Entity<TextViewState>>,
    linked_messages: BTreeSet<MessageId>,
    references: Vec<Reference>,
    link: Link,
    live: Option<Live>,
    queued: Option<(String, Option<String>)>,
    permissions: Vec<PermissionAsk>,
    gate: Option<Gate>,
    problem: Option<Problem>,
    turn_started: Option<Instant>,
    // Wall-clock start of the turn, to find the files the agent wrote during it.
    turn_began: Option<SystemTime>,
    state: AgentState,
    history: History,
    history_saves: async_channel::Sender<History>,
    view: View,
    menu: Option<Menu>,
    menu_index: usize,
    access: Access,
    model_choice: Option<String>,
    effort_choice: Option<String>,
    mode_synced: bool,
    title: Option<String>,
    resume_backup: Option<Backup>,
    sessions_query: Entity<InputState>,
    slash_index: usize,
    // The composer text for which the user closed the slash list.
    slash_dismissed: Option<String>,
    // Bumped when a conversation ends, so a late answer from an older session is dropped.
    epoch: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChatEvent> for ChatPanel {}

fn closed<E: std::fmt::Display>(result: Result<(), E>) {
    if let Err(error) = result {
        tracing::debug!(%error, "the chat closed before an asynchronous step finished");
    }
}

// The name of a file is the user's, but it still ends up inside text an agent reads.
fn plain_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && *c != '"')
        .take(MAX_NAME_CHARS)
        .collect()
}

fn context_line(workbook: Option<(WorkbookId, String)>, selection: Option<&str>) -> String {
    let Some((id, name)) = workbook else {
        return format!(
            "{CONTEXT_MARKER} No workbook is open. New workbooks are made with create_workbook."
        );
    };
    let mut line = format!(
        "{CONTEXT_MARKER} The user is looking at the workbook \"{}\" (workbook id {})",
        plain_name(&name),
        id.0
    );
    if let Some(selection) = selection {
        line.push_str(&format!(", selection {}", plain_name(selection)));
    }
    line.push_str(
        ". Other open workbooks are listed by list_workbooks; address any of them by id.",
    );
    line
}

impl ChatPanel {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        endpoint: ToolEndpoint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ChatPanel {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder(t!("chat.composer.placeholder"))
        });
        let sending = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => {
                    if this.menu.is_some() {
                        this.menu_accept(cx);
                    } else if this.slash_open(cx) {
                        this.slash_accept(window, cx);
                    } else {
                        this.send(window, cx);
                    }
                }
                InputEvent::Change => {
                    this.slash_index = 0;
                    cx.notify();
                }
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
                InputEvent::PressEnter { .. } => {}
            },
        );
        let settings = cx.observe_global::<AgentConfig>(|this, cx| {
            this.follow_setting(cx);
            cx.notify();
        });
        let sessions_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("chat.sessions.search")));
        let searching = cx.subscribe(&sessions_query, |_, _, _: &InputEvent, cx| cx.notify());
        let (history_saves, history_queue) = async_channel::unbounded();
        let mut panel = ChatPanel {
            workspace,
            endpoint,
            composer,
            scroll: ScrollHandle::new(),
            thread: Thread::default(),
            texts: BTreeMap::new(),
            linked_messages: BTreeSet::new(),
            references: Vec::new(),
            link: Link::Idle,
            live: None,
            queued: None,
            permissions: Vec::new(),
            gate: None,
            problem: None,
            turn_started: None,
            turn_began: None,
            state: AgentState::default(),
            history: History::default(),
            history_saves,
            view: View::Chat,
            menu: None,
            menu_index: 0,
            access: Access::from_setting(
                cx.global::<AgentConfig>().state.current.agents.permission,
            ),
            model_choice: None,
            effort_choice: None,
            mode_synced: false,
            title: None,
            resume_backup: None,
            sessions_query,
            slash_index: 0,
            slash_dismissed: None,
            epoch: 0,
            _subscriptions: vec![sending, settings, searching],
        };
        panel.load_history(history_queue, cx);
        panel
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self.composer.clone();
        composer.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn add_reference(
        &mut self,
        reference: Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.references.contains(&reference) {
            self.references.push(reference);
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub(super) fn remove_reference(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.references.len() {
            self.references.remove(index);
            cx.notify();
        }
    }

    pub(super) fn open_reference(
        &mut self,
        reference: &Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        closed(self.workspace.update(cx, |workspace, cx| {
            workspace.reveal_reference(reference, window, cx)
        }));
    }

    // Chips go in front of the message, except before a slash command, which must stay first.
    fn message_with_references(&self, typed: &str) -> String {
        if self.references.is_empty() || typed.trim().is_empty() {
            return typed.to_string();
        }
        let references: Vec<String> = self.references.iter().map(Reference::text).collect();
        let references = references.join(" ");
        if typed.starts_with('/') {
            format!("{typed} {references}")
        } else {
            format!("{references} {typed}")
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.thread.turn() != TurnState::Idle
    }

    pub(super) fn active_workbook(&self, cx: &App) -> Option<(WorkbookId, String)> {
        self.workspace
            .read_with(cx, |workspace, _| workspace.active_workbook_for_chat())
            .ok()
            .flatten()
    }

    fn folder_hint(&self, cx: &App) -> crate::agent_folder::FolderHint {
        self.workspace
            .read_with(cx, |workspace, _| workspace.folder_hint_for_chat())
            .unwrap_or_default()
    }

    pub(super) fn prompt_context(&self, cx: &App) -> Option<String> {
        let selection = self
            .workspace
            .read_with(cx, |workspace, cx| workspace.selection_for_chat(cx))
            .ok()
            .flatten();
        Some(context_line(self.active_workbook(cx), selection.as_deref()))
    }

    pub(super) fn at_bottom(&self) -> bool {
        let remaining = self.scroll.max_offset().y + self.scroll.offset().y;
        remaining.abs() < px(SCROLL_FOLLOW_SLACK)
    }

    pub(super) fn scroll_to_end(&self) {
        self.scroll.scroll_to_bottom();
    }

    pub(crate) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.composer.read(cx).value().to_string();
        let text = self.message_with_references(&typed);
        if !self.thread.submit(text.clone()) {
            return;
        }
        self.references.clear();
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.turn_started = Some(Instant::now());
        self.turn_began = Some(SystemTime::now());
        self.problem = None;
        self.view = View::Chat;
        self.menu = None;
        self.remember_conversation(&text, cx);
        let context = self.prompt_context(cx);
        match &self.live {
            Some(live) if live.ready => live.handle.prompt(text, context),
            _ => {
                self.queued = Some((text, context));
                self.begin(Start::Message, window, cx);
            }
        }
        self.scroll_to_end();
        cx.notify();
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.busy() {
            return;
        }
        self.thread.request_cancel();
        for ask in self.permissions.drain(..) {
            ask.cancel();
        }
        match &self.live {
            Some(live) if live.ready => live.handle.cancel(),
            _ => {
                self.epoch += 1;
                self.queued = None;
                self.gate = None;
                self.end_session(cx);
                self.finish_turn(TurnEnd::Cancelled, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.epoch += 1;
        self.end_session(cx);
        for ask in self.permissions.drain(..) {
            ask.cancel();
        }
        self.thread.clear();
        self.texts.clear();
        self.linked_messages.clear();
        self.references.clear();
        self.queued = None;
        self.gate = None;
        self.resume_backup = None;
        self.problem = None;
        self.turn_started = None;
        self.state = AgentState {
            selects: std::mem::take(&mut self.state.selects),
            ..AgentState::default()
        };
        self.mode_synced = false;
        self.view = View::Chat;
        self.menu = None;
        self.title = None;
        self.focus_composer(window, cx);
        self.warm_up(window, cx);
        cx.notify();
    }

    pub(super) fn cycle_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let ids: Vec<AgentId> = settings.agents.servers.keys().cloned().collect();
        if ids.len() < 2 {
            return;
        }
        let current = launch::chosen_agent(&settings).map(|(id, _)| id);
        let next = current
            .and_then(|id| ids.iter().position(|candidate| *candidate == id))
            .map_or(0, |index| (index + 1) % ids.len());
        let next = ids[next].clone();
        agent_settings::change(cx, move |settings| {
            settings.agents.default = Some(next.clone());
        });
        self.state.selects.clear();
        self.model_choice = None;
        self.effort_choice = None;
        self.new_conversation(window, cx);
    }

    pub(super) fn copy_login_command(&self, cx: &mut Context<Self>) {
        if let Some(command) = self.problem.as_ref().and_then(|problem| problem.login) {
            cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::WorkbookId;

    use super::{context_line, plain_name};

    #[test]
    fn the_context_names_the_workbook_sheet_and_selection() {
        let line = context_line(
            Some((WorkbookId(3), "Ventas.xlsx".to_string())),
            Some("'Cash flow'!A5:B11"),
        );
        assert!(line.starts_with("[Zenkai] "));
        assert!(line.contains("\"Ventas.xlsx\" (workbook id 3)"), "{line}");
        assert!(line.contains("selection 'Cash flow'!A5:B11"), "{line}");
    }

    #[test]
    fn without_a_workbook_the_context_says_so() {
        let line = context_line(None, None);
        assert!(line.contains("No workbook is open"), "{line}");
    }

    #[test]
    fn a_file_name_loses_quotes_and_control_characters_before_an_agent_reads_it() {
        assert_eq!(plain_name("Ventas \"Q3\"\n.xlsx"), "Ventas Q3.xlsx");
    }

    #[test]
    fn a_very_long_file_name_is_cut() {
        assert_eq!(plain_name(&"a".repeat(500)).chars().count(), 80);
    }
}
