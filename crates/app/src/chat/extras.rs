use std::path::PathBuf;

use async_channel::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;
use zenkai_agent::chat::history::History;
use zenkai_agent::chat::state::slash_query;
use zenkai_agent::chat::thread::NoticeKind;
use zenkai_i18n::t;

use super::session_rows::{Row, Source};
use super::slash::{self, Entry, Target, ZenkaiCommand};
use super::{Backup, ChatPanel, View, closed, launch};
use crate::actions::*;
use crate::agent_settings::AgentConfig;

pub(super) fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn history_path() -> Option<PathBuf> {
    Some(crate::recovery::session_directory()?.join("chat-history.json"))
}

impl ChatPanel {
    // One task writes the file, and only after a successful load, so a save can neither land out
    // of order nor replace history that was never read.
    pub(super) fn load_history(&mut self, queue: Receiver<History>, cx: &mut Context<Self>) {
        let Some(path) = history_path() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { History::load_or_quarantine(&load_path) })
                .await;
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    tracing::warn!(%error, "conversation history not loaded; it will not be saved");
                    return;
                }
            };
            let on_disk = loaded.clone();
            closed(this.update(cx, |this, cx| {
                this.history.merge(loaded);
                this.refresh_session_sizes(cx);
                cx.notify();
            }));
            cx.background_executor()
                .spawn(async move {
                    while let Ok(mut latest) = queue.recv().await {
                        while let Ok(newer) = queue.try_recv() {
                            latest = newer;
                        }
                        latest.merge(on_disk.clone());
                        if let Err(error) = latest.save(&path) {
                            tracing::warn!(%error, "conversation history not saved");
                        }
                    }
                })
                .detach();
        })
        .detach();
    }

    pub(super) fn remember_conversation(&mut self, first_message: &str, cx: &mut Context<Self>) {
        if self.title.is_some() {
            return;
        }
        let title: String = first_message
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(48)
            .collect();
        self.title = Some(title.clone());
        let Some(agent) = super::launch::chosen_agent(&cx.global::<AgentConfig>().state.current)
            .map(|(id, _)| id)
        else {
            return;
        };
        let workbook = self
            .active_workbook(cx)
            .map(|(_, name)| name)
            .unwrap_or_default();
        if self
            .history
            .push(agent.clone(), first_message, &workbook, now_seconds())
        {
            self.pending_record = self
                .history
                .newest_for(&agent)
                .map(|record| record.identity());
        } else {
            self.pending_record = None;
        }
        if let Some(record) = self.history.newest_for(&agent) {
            let key = record.identity();
            let size = History::record_size(record);
            self.session_sizes.insert(key, size);
        }
        if self.history_saves.try_send(self.history.clone()).is_err() {
            tracing::debug!("conversation history writer is not running");
        }
    }

    pub(crate) fn toggle_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let next = match self.view {
            View::Chat => View::Sessions,
            View::Sessions => View::Chat,
        };
        self.set_view(next, cx);
        if self.view == View::Sessions {
            self.menu_index = 0;
            self.hidden_agents.clear();
            self.refresh_session_sizes(cx);
            let search = self.sessions_query.clone();
            search.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.focus_composer(window, cx);
        }
        if self.view == View::Sessions
            && self.state.abilities.list_sessions
            && let Some(live) = &self.live
            && live.ready
        {
            live.handle.list_sessions(self.history.started_sessions());
        }
        cx.notify();
    }

    // Continues a cross-agent resume once the switch has completed. The session event
    // that reports readiness carries no window, so the workspace delivers this instead.
    pub(crate) fn continue_pending_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.pending_resume.take() else {
            return;
        };
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let current = launch::chosen_agent(&settings).map(|(id, _)| id);
        if current.as_ref() != Some(&row.agent) {
            return;
        }
        self.resume(&row, window, cx);
    }

    pub(super) fn load_session(&mut self, session: String, title: Option<String>) {
        for ask in self.permissions.drain(..) {
            ask.cancel();
        }
        let old_title = std::mem::replace(&mut self.title, title);
        self.resume_backup = Some(Backup {
            thread: std::mem::take(&mut self.thread),
            texts: std::mem::take(&mut self.texts),
            title: old_title,
        });
        self.problem = None;
        self.live_session = Some(session.clone());
        self.pending_record = None;
        if let Some(live) = &self.live {
            live.handle.resume(session);
        }
    }

    pub(super) fn resume(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_resume = None;
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let current = launch::chosen_agent(&settings).map(|(id, _)| id);
        if current.as_ref() != Some(&row.agent) {
            self.pick_agent(row.agent.clone(), cx);
            self.pending_resume = Some(row.clone());
            return;
        }
        let title = row.title.clone();
        match &row.source {
            Source::Agent { session } => {
                let ready = self.live.as_ref().is_some_and(|live| live.ready);
                if !ready {
                    return;
                }
                if self.busy() {
                    self.thread
                        .notice(NoticeKind::Info, t!("chat.stop_before_switching"));
                    self.set_view(View::Chat, cx);
                    cx.notify();
                    return;
                }
                self.load_session(session.clone(), Some(title));
            }
            Source::Local => {
                self.new_conversation(window, cx);
                self.title = Some(title);
                self.thread
                    .notice(NoticeKind::Info, t!("chat.started_fresh"));
            }
        }
        self.set_view(View::Chat, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn typed_slash(&self, cx: &App) -> Option<String> {
        let text = self.composer.read(cx).value().to_string();
        if self.slash_dismissed.as_deref() == Some(text.as_str()) {
            return None;
        }
        slash_query(&text).map(str::to_string)
    }

    pub(super) fn slash_entries(&self, cx: &App) -> Vec<Entry> {
        self.typed_slash(cx)
            .map(|query| slash::entries(&query, &self.state))
            .unwrap_or_default()
    }

    pub(super) fn slash_open(&self, cx: &App) -> bool {
        self.mention_open(cx) || !self.slash_entries(cx).is_empty()
    }

    pub(crate) fn slash_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.mention_open(cx) {
            self.mention_step(forward, cx);
            return;
        }
        let count = self.slash_entries(cx).len();
        if count == 0 {
            return;
        }
        self.slash_index = if forward {
            (self.slash_index + 1) % count
        } else {
            (self.slash_index + count - 1) % count
        };
        cx.notify();
    }

    pub(crate) fn slash_close(&mut self, cx: &mut Context<Self>) {
        if self.mention_open(cx) {
            self.mention_close(cx);
            return;
        }
        self.slash_dismissed = Some(self.composer.read(cx).value().to_string());
        cx.notify();
    }

    pub(crate) fn slash_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention_open(cx) {
            self.mention_accept(window, cx);
            return;
        }
        let entries = self.slash_entries(cx);
        let Some(entry) = entries.get(self.slash_index.min(entries.len().saturating_sub(1))) else {
            return;
        };
        self.slash_index = 0;
        match entry.target.clone() {
            Target::Agent => {
                let text = format!("{} ", entry.name);
                self.composer
                    .update(cx, |state, cx| state.set_value(text, window, cx));
            }
            Target::Zenkai(command) => {
                self.composer
                    .update(cx, |state, cx| state.set_value("", window, cx));
                let action: Box<dyn Action> = match command {
                    ZenkaiCommand::Selection => Box::new(AddSelectionToChat),
                    ZenkaiCommand::Generate => Box::new(GenerateData),
                    ZenkaiCommand::Chart => Box::new(InsertChart),
                };
                window.dispatch_action(action, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn agent_name(&self, cx: &App) -> String {
        launch::chosen_agent(&cx.global::<AgentConfig>().state.current)
            .map_or_else(|| t!("chat.agent").to_string(), |(_, server)| server.name)
    }
}
