mod cards;
mod composer;
mod events;
mod extras;
mod header;
mod launch;
mod permissions;
pub(crate) mod reference;
mod render;
mod session_rows;
mod sessions;
mod slash;
mod startup;
mod transcript;

use std::collections::BTreeMap;
use std::time::Instant;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use zenkai_agent::chat::history::History;
use zenkai_agent::chat::session::PermissionAsk;
use zenkai_agent::chat::state::AgentState;
use zenkai_agent::chat::thread::{MessageId, Thread, TurnEnd, TurnState};
use zenkai_agent::settings::{AgentId, PermissionMode};
use zenkai_agent::tools::ToolEndpoint;
use zenkai_types::WorkbookId;

use crate::agent_settings::{self, AgentConfig};
use crate::view::Workspace;
use launch::{Live, Prepared};

const SCROLL_FOLLOW_SLACK: f32 = 48.0;
const MAX_NAME_CHARS: usize = 80;
const MONO: &str = "IBM Plex Mono";

pub enum ChatEvent {
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Chat,
    Sessions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Model,
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
    link: Link,
    live: Option<Live>,
    queued: Option<(String, Option<String>)>,
    permissions: Vec<PermissionAsk>,
    gate: Option<Gate>,
    problem: Option<Problem>,
    turn_started: Option<Instant>,
    state: AgentState,
    history: History,
    view: View,
    menu: Option<Menu>,
    title: Option<String>,
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
                .placeholder("Ask a question or ask for a change")
        });
        let sending = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => {
                    if this.slash_open(cx) {
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
        let settings = cx.observe_global::<AgentConfig>(|_, cx| cx.notify());
        let sessions_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions"));
        let searching = cx.subscribe(&sessions_query, |_, _, _: &InputEvent, cx| cx.notify());
        let mut panel = ChatPanel {
            workspace,
            endpoint,
            composer,
            scroll: ScrollHandle::new(),
            thread: Thread::default(),
            texts: BTreeMap::new(),
            link: Link::Idle,
            live: None,
            queued: None,
            permissions: Vec::new(),
            gate: None,
            problem: None,
            turn_started: None,
            state: AgentState::default(),
            history: History::default(),
            view: View::Chat,
            menu: None,
            title: None,
            sessions_query,
            slash_index: 0,
            slash_dismissed: None,
            epoch: 0,
            _subscriptions: vec![sending, settings, searching],
        };
        panel.load_history(cx);
        panel
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self.composer.clone();
        composer.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.composer
            .update(cx, |state, cx| state.insert(text, window, cx));
        self.focus_composer(window, cx);
    }

    pub(super) fn busy(&self) -> bool {
        self.thread.turn() != TurnState::Idle
    }

    pub(super) fn active_workbook(&self, cx: &App) -> Option<(WorkbookId, String)> {
        self.workspace
            .read_with(cx, |workspace, _| workspace.active_workbook_for_chat())
            .ok()
    }

    pub(super) fn prompt_context(&self, cx: &App) -> Option<String> {
        let (id, name) = self.active_workbook(cx)?;
        Some(format!(
            "[Zenkai] The user is looking at the workbook \"{}\" (workbook id {}). Other open \
             workbooks are listed by list_workbooks; address any of them by id.",
            plain_name(&name),
            id.0
        ))
    }

    pub(super) fn at_bottom(&self) -> bool {
        let remaining = self.scroll.max_offset().y + self.scroll.offset().y;
        remaining.abs() < px(SCROLL_FOLLOW_SLACK)
    }

    pub(super) fn scroll_to_end(&self) {
        self.scroll.scroll_to_bottom();
    }

    pub(crate) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        if !self.thread.submit(text.clone()) {
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.turn_started = Some(Instant::now());
        self.problem = None;
        self.view = View::Chat;
        self.menu = None;
        self.remember_conversation(&text, cx);
        let context = self.prompt_context(cx);
        match &self.live {
            Some(live) if live.ready => live.handle.prompt(text, context),
            _ => {
                self.queued = Some((text, context));
                self.begin(window, cx);
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
        self.queued = None;
        self.gate = None;
        self.problem = None;
        self.turn_started = None;
        self.state = AgentState::default();
        self.view = View::Chat;
        self.menu = None;
        self.title = None;
        self.focus_composer(window, cx);
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
        self.new_conversation(window, cx);
    }

    pub(super) fn cycle_permission(&mut self, cx: &mut Context<Self>) {
        let current = cx.global::<AgentConfig>().state.current.agents.permission;
        let modes = PermissionMode::ALL;
        let index = modes.iter().position(|mode| *mode == current).unwrap_or(0);
        let next = modes[(index + 1) % modes.len()];
        crate::settings_page::set_permission(next, cx);
    }

    pub(super) fn copy_login_command(&self, cx: &mut Context<Self>) {
        if let Some(command) = self.problem.as_ref().and_then(|problem| problem.login) {
            cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plain_name;

    #[test]
    fn a_file_name_loses_quotes_and_control_characters_before_an_agent_reads_it() {
        assert_eq!(plain_name("Ventas \"Q3\"\n.xlsx"), "Ventas Q3.xlsx");
    }

    #[test]
    fn a_very_long_file_name_is_cut() {
        assert_eq!(plain_name(&"a".repeat(500)).chars().count(), 80);
    }
}
