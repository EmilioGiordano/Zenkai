use gpui_kit::*;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::chat::{ChatEvent, ChatPanel, reference};

#[derive(Default)]
pub(super) struct ChatDock {
    panel: Option<Entity<ChatPanel>>,
    open: bool,
    events: Option<Subscription>,
}

impl Workspace {
    pub(crate) fn active_workbook_for_chat(&self) -> (WorkbookId, String) {
        (self.documents.active_id(), self.documents.active().name())
    }

    fn chat_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChatPanel> {
        if let Some(panel) = &self.chat.panel {
            return panel.clone();
        }
        let workspace = cx.weak_entity();
        let endpoint = self.agent.endpoint();
        let panel = cx.new(|cx| ChatPanel::new(workspace, endpoint, window, cx));
        self.chat.events = Some(cx.subscribe_in(
            &panel,
            window,
            |this, _, event: &ChatEvent, window, cx| match event {
                ChatEvent::Leave => {
                    let focus = this.grid.focus_handle(cx);
                    window.focus(&focus, cx);
                }
                // Shown without taking the focus: the user may be typing in the grid.
                ChatEvent::NeedsAnswer => {
                    this.chat.open = true;
                    cx.notify();
                }
            },
        ));
        self.chat.panel = Some(panel.clone());
        panel
    }

    pub(super) fn toggle_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.open {
            self.close_chat(window, cx);
        } else {
            self.open_chat(window, cx);
        }
    }

    fn open_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChatPanel> {
        let panel = self.chat_panel(window, cx);
        self.chat.open = true;
        panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
        cx.notify();
        panel
    }

    fn close_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = self
            .chat
            .panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).has_pending_permission());
        self.chat.open = false;
        if waiting {
            self.notify(
                Severity::Warning,
                "The agent is waiting for your answer. Press Ctrl+J to show it.",
                cx,
            );
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn add_selection_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.documents.active();
        let sheet = document
            .sheets
            .iter()
            .find(|info| info.id == document.sheet)
            .map(|info| info.name.clone())
            .unwrap_or_default();
        let text = format!("{} ", reference::sheet_range(&sheet, self.selection(cx)));
        let panel = self.open_chat(window, cx);
        panel.update(cx, |panel, cx| panel.insert_text(&text, window, cx));
    }

    pub(super) fn new_agent_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.open_chat(window, cx);
        panel.update(cx, |panel, cx| panel.new_conversation(window, cx));
    }

    pub(super) fn answer_chat_permission(&mut self, allow: bool, cx: &mut Context<Self>) {
        let Some(panel) = self.chat.panel.clone() else {
            return;
        };
        if !self.chat.open || !panel.read(cx).permission_visible() {
            self.notify(
                Severity::Info,
                "No agent question is on screen. Open the agent panel (Ctrl+J) to see it.",
                cx,
            );
            return;
        }
        panel.update(cx, |panel, cx| {
            if allow {
                panel.allow_permission(cx);
            } else {
                panel.deny_permission(cx);
            }
        });
    }

    pub(super) fn render_chat(&self) -> Option<Entity<ChatPanel>> {
        self.chat.panel.clone().filter(|_| self.chat.open)
    }

    pub(super) fn shutdown_chat(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<std::sync::Arc<zenkai_agent::chat::process::ProcessTree>> {
        let panel = self.chat.panel.clone()?;
        panel.update(cx, |panel, _| panel.shutdown())
    }
}
