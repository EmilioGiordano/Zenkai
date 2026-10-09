use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use zenkai_agent::chat::session::{Connection, SessionError, SessionEvent};
use zenkai_agent::chat::thread::{Effect, MessageId, NoticeKind, TurnEnd};
use zenkai_agent::presets::Preset;
use zenkai_i18n::t;

use super::launch;
use super::{ChatEvent, ChatPanel, Link, MAX_PENDING_ASKS, Problem, View};
use crate::agent_settings::AgentConfig;

impl ChatPanel {
    pub(super) fn elapsed_seconds(&mut self) -> u64 {
        self.turn_started
            .take()
            .map_or(0, |started| started.elapsed().as_secs())
    }

    pub(super) fn finish_turn(&mut self, end: TurnEnd, cx: &mut Context<Self>) {
        for ask in self.permissions.drain(..) {
            ask.cancel();
        }
        let seconds = self.elapsed_seconds();
        if self.busy() {
            self.thread.end_turn(end, seconds);
        } else if let TurnEnd::Failed(text) = end {
            self.thread.notice(NoticeKind::Error, &text);
        }
        self.scroll_to_end();
        cx.notify();
    }

    pub(super) fn login_command(&self, cx: &App) -> Option<&'static str> {
        let agent = match &self.live {
            Some(live) => live.agent.clone(),
            None => launch::chosen_agent(&cx.global::<AgentConfig>().state.current)?.0,
        };
        Preset::for_agent(&agent).map(|preset| preset.login_command)
    }

    pub(super) fn on_event(&mut self, epoch: u64, event: SessionEvent, cx: &mut Context<Self>) {
        if epoch != self.epoch {
            return;
        }
        match event {
            SessionEvent::Connection(Connection::Installing) => self.link = Link::Installing,
            SessionEvent::Connection(Connection::Starting) => self.link = Link::Starting,
            SessionEvent::Connection(Connection::Ready { agent }) => {
                self.link = Link::Ready(agent.into());
                if let Some(live) = &mut self.live {
                    live.ready = true;
                    if self.state.abilities.list_sessions {
                        live.handle.list_sessions(self.history.started_sessions());
                    }
                    if let Some((text, context)) = self.queued.take() {
                        live.handle.prompt(text, context);
                    }
                }
            }
            SessionEvent::Update(update) => {
                let follow = self.at_bottom();
                if let Effect::Appended { message, added } = self.thread.apply(update) {
                    self.grow_message(message, added, cx);
                }
                if follow {
                    self.scroll_to_end();
                }
            }
            SessionEvent::Permission(ask) if self.permissions.len() >= MAX_PENDING_ASKS => {
                ask.cancel();
                self.thread
                    .notice(NoticeKind::Error, t!("chat.too_many_questions"));
            }
            SessionEvent::Permission(ask) => {
                self.thread.waiting_for_permission(&ask.tool, &ask.title);
                self.permissions.push(ask);
                self.view = View::Chat;
                cx.emit(ChatEvent::NeedsAnswer);
                self.scroll_to_end();
            }
            SessionEvent::TurnEnded(end) => {
                if let Some(since) = self.turn_began.take() {
                    self.list_files_written(since, cx);
                }
                self.finish_turn(end, cx);
            }
            SessionEvent::AuthRequired => self.require_sign_in(cx),
            SessionEvent::Failed(error) => {
                if error == SessionError::AuthRequired {
                    self.require_sign_in(cx);
                }
                self.end_session(cx);
                self.finish_turn(TurnEnd::Failed(error.to_string()), cx);
            }
            SessionEvent::Closed => self.end_session(cx),
            SessionEvent::State(change) => self.state.apply(change),
            SessionEvent::Problem(text) => {
                if let Some(backup) = self.resume_backup.take() {
                    self.thread = backup.thread;
                    self.texts = backup.texts;
                    self.title = backup.title;
                }
                self.thread.notice(NoticeKind::Error, &text);
            }
            SessionEvent::Started(id) => {
                self.history.remember_session(&id);
                if self.history_saves.try_send(self.history.clone()).is_err() {
                    tracing::debug!("conversation history writer is not running");
                }
            }
            SessionEvent::Resumed => {
                self.resume_backup = None;
                self.scroll_to_end();
            }
        }
        cx.notify();
    }

    pub(super) fn require_sign_in(&mut self, cx: &App) {
        let login = self.login_command(cx);
        self.problem = Some(Problem {
            text: t!("chat.sign_in").into(),
            login,
        });
    }

    pub(super) fn grow_message(
        &mut self,
        message: MessageId,
        added: String,
        cx: &mut Context<Self>,
    ) {
        match self.texts.get(&message) {
            Some(state) => state.update(cx, |state, cx| state.push_str(&added, cx)),
            None => {
                let state = cx.new(|cx| TextViewState::markdown(&added, cx));
                self.texts.insert(message, state);
                // Messages the transcript trimmed no longer need their view state.
                self.texts
                    .retain(|id, _| self.thread.message_text(*id).is_some());
            }
        }
    }

    // The tree is ended off the UI thread; the bridge closes when `live` is dropped.
    pub(super) fn end_session(&mut self, cx: &mut Context<Self>) {
        self.link = Link::Idle;
        if let Some(live) = self.live.take() {
            let tree = live.handle.process_tree();
            cx.background_executor()
                .spawn(async move { tree.close() })
                .detach();
        }
    }

    // The caller ends the tree off the UI thread.
    pub fn shutdown(&mut self) -> Option<std::sync::Arc<zenkai_agent::chat::process::ProcessTree>> {
        self.live.take().map(|live| live.handle.process_tree())
    }
}
