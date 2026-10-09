use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use zenkai_agent::chat::session::{Connection, SessionError, SessionEvent};
use zenkai_agent::chat::thread::{Effect, MessageId, NoticeKind, TurnEnd};
use zenkai_agent::presets::Preset;

use super::launch;
use super::{ChatPanel, Link, Problem};
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
                        live.handle.list_sessions();
                    }
                    if let Some((text, context)) = self.queued.take() {
                        live.handle.prompt(text, context);
                    }
                }
            }
            SessionEvent::Update(update) => {
                let follow = self.at_bottom();
                if let Effect::Appended { message, from } = self.thread.apply(update) {
                    self.grow_message(message, from, cx);
                }
                if follow {
                    self.scroll_to_end();
                }
            }
            SessionEvent::Permission(ask) => {
                self.thread.waiting_for_permission(&ask.tool, &ask.title);
                self.permissions.push(ask);
                self.scroll_to_end();
            }
            SessionEvent::TurnEnded(end) => self.finish_turn(end, cx),
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
            SessionEvent::Problem(text) => self.thread.notice(NoticeKind::Error, &text),
            SessionEvent::Resumed => self.scroll_to_end(),
        }
        cx.notify();
    }

    pub(super) fn require_sign_in(&mut self, cx: &App) {
        let login = self.login_command(cx);
        self.problem = Some(Problem {
            text: "The agent needs you to sign in. Run this in a terminal, then send your message again."
                .into(),
            login,
        });
    }

    pub(super) fn grow_message(&mut self, message: MessageId, from: usize, cx: &mut Context<Self>) {
        let Some(text) = self.thread.message_text(message) else {
            return;
        };
        match self.texts.get(&message) {
            Some(state) => {
                let added = text.get(from..).unwrap_or_default().to_string();
                state.update(cx, |state, cx| state.push_str(&added, cx));
            }
            None => {
                let text = text.to_string();
                let state = cx.new(|cx| TextViewState::markdown(&text, cx));
                self.texts.insert(message, state);
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

    pub fn shutdown(&mut self) {
        if let Some(live) = self.live.take() {
            live.handle.process_tree().close();
        }
    }
}
