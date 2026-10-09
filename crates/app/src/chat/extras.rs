use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;
use zenkai_agent::chat::history::History;
use zenkai_agent::chat::state::{ConfigKind, slash_query};
use zenkai_agent::chat::thread::NoticeKind;

use super::session_rows::{Row, Source};
use super::slash::{self, Entry, Target, ZenkaiCommand};
use super::{Backup, ChatPanel, Menu, View, closed, launch};
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
    pub(super) fn load_history(&mut self, cx: &mut Context<Self>) {
        let Some(path) = history_path() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { History::load(&path) })
                .await;
            closed(this.update(cx, |this, cx| {
                match loaded {
                    Ok(history) => this.history = history,
                    Err(error) => tracing::warn!(%error, "conversation history not loaded"),
                }
                cx.notify();
            }));
        })
        .detach();
    }

    // Titles the conversation after its first message and keeps its metadata, never its text.
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
        self.history
            .push(agent, first_message, &workbook, now_seconds());
        let (Some(path), history) = (history_path(), self.history.clone()) else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = history.save(&path) {
                    tracing::warn!(%error, "conversation history not saved");
                }
            })
            .detach();
    }

    pub(crate) fn toggle_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.view = match self.view {
            View::Chat => View::Sessions,
            View::Sessions => View::Chat,
        };
        if self.view == View::Sessions {
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
            live.handle.list_sessions();
        }
        cx.notify();
    }

    pub(super) fn resume(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let title = row.title.clone();
        match &row.source {
            Source::Agent { session } => {
                let ready = self.live.as_ref().is_some_and(|live| live.ready);
                if !ready {
                    return;
                }
                if self.busy() {
                    self.thread.notice(
                        NoticeKind::Info,
                        "Stop the agent before opening another conversation.",
                    );
                    self.view = View::Chat;
                    cx.notify();
                    return;
                }
                for ask in self.permissions.drain(..) {
                    ask.cancel();
                }
                // The old transcript comes back if the agent cannot load the session.
                let old_title = self.title.replace(title);
                self.resume_backup = Some(Backup {
                    thread: std::mem::take(&mut self.thread),
                    texts: std::mem::take(&mut self.texts),
                    title: old_title,
                });
                self.problem = None;
                if let Some(live) = &self.live {
                    live.handle.resume(session.clone());
                }
            }
            Source::Local => {
                self.new_conversation(window, cx);
                self.title = Some(title);
                self.thread.notice(
                    NoticeKind::Info,
                    "Started fresh: this agent cannot reopen an earlier conversation, so only its title was kept.",
                );
            }
        }
        self.view = View::Chat;
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub(crate) fn toggle_model_menu(&mut self, cx: &mut Context<Self>) {
        if self.state.select(ConfigKind::Model).is_none()
            && self.state.select(ConfigKind::Effort).is_none()
        {
            return;
        }
        self.menu = match self.menu {
            Some(Menu::Model) => None,
            None => Some(Menu::Model),
        };
        cx.notify();
    }

    pub(super) fn choose(&mut self, kind: ConfigKind, value: &str, cx: &mut Context<Self>) {
        let Some(select) = self.state.select(kind) else {
            return;
        };
        if !select.offers(value) {
            return;
        }
        if let Some(live) = &self.live {
            live.handle
                .set_config(select.id.clone(), select.source, value.to_string());
        }
        cx.notify();
    }

    pub(crate) fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let Some(select) = self.state.select(ConfigKind::Mode) else {
            return;
        };
        let values: Vec<String> = select.choices.iter().map(|c| c.value.clone()).collect();
        let position = values
            .iter()
            .position(|v| *v == select.current)
            .unwrap_or(0);
        if let Some(next) = values.get((position + 1) % values.len().max(1)).cloned() {
            self.choose(ConfigKind::Mode, &next, cx);
        }
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
        !self.slash_entries(cx).is_empty()
    }

    pub(crate) fn slash_step(&mut self, forward: bool, cx: &mut Context<Self>) {
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
        self.slash_dismissed = Some(self.composer.read(cx).value().to_string());
        cx.notify();
    }

    pub(crate) fn slash_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            .map_or_else(|| "Agent".to_string(), |(_, server)| server.name)
    }
}
