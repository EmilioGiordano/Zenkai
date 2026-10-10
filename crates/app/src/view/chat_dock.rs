use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::base::Selectable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_agent::chat::thread::FileCard;
use zenkai_i18n::t;
use zenkai_types::{SheetId, WorkbookId};

use super::{Severity, Workspace, sidebar};
use crate::actions::ToggleAgentChat;
use crate::agent_folder::FolderHint;
use crate::chat::reference::Reference;
use crate::chat::{ChatEvent, ChatPanel, reference};
use crate::document::Document;
use crate::keymap;
use crate::panel_width::Panel;

#[derive(Default)]
pub(super) struct ChatDock {
    panel: Option<Entity<ChatPanel>>,
    open: bool,
    events: Option<Subscription>,
}

impl Workspace {
    pub(crate) fn active_workbook_for_chat(&self) -> Option<(WorkbookId, String)> {
        let document = self.documents.active()?;
        Some((document.id, document.name()))
    }

    pub(crate) fn add_chat_card(&mut self, card: FileCard, cx: &mut Context<Self>) {
        if let Some(panel) = self.chat.panel.clone() {
            panel.update(cx, |panel, cx| panel.file_created(card, cx));
        }
    }

    pub(crate) fn open_from_chat(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_path(path, window, cx);
    }

    pub(crate) fn selection_for_chat(&self, cx: &App) -> Option<String> {
        let document = self.documents.active()?;
        let sheet = document
            .sheets
            .iter()
            .find(|info| info.id == document.sheet)?;
        Some(reference::sheet_range(&sheet.name, self.selection(cx)))
    }

    pub(crate) fn folder_hint_for_chat(&self) -> FolderHint {
        let space = self.documents.current_space();
        FolderHint {
            space_files: self
                .documents
                .members(space)
                .filter_map(|entry| entry.path().map(Path::to_path_buf))
                .collect(),
            active_file: self
                .documents
                .active()
                .and_then(|document| document.path.clone()),
        }
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

    pub(super) fn render_chat_toggle(&self, cx: &App) -> impl IntoElement {
        let label = if self.chat.open {
            t!("chat.hide.tooltip")
        } else {
            t!("chat.show.tooltip")
        };
        div().occlude().child(
            Button::new("toggle-chat")
                .ghost()
                .compact()
                .icon(IconName::PanelRight)
                .selected(self.chat.open)
                .tooltip(keymap::labeled(cx, label, &ToggleAgentChat))
                .on_click(|event, window, cx| {
                    if sidebar::is_primary_click(event) {
                        window.dispatch_action(ToggleAgentChat.boxed_clone(), cx)
                    }
                }),
        )
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
        panel.update(cx, |panel, cx| {
            panel.focus_composer(window, cx);
            panel.warm_up(window, cx);
        });
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
            self.notify(Severity::Warning, t!("chat.waiting_for_answer"), cx);
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn add_selection_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let sheet = document
            .sheets
            .iter()
            .find(|info| info.id == document.sheet)
            .map(|info| info.name.clone())
            .unwrap_or_default();
        let reference = Reference {
            sheet: Some(sheet),
            range: self.selection(cx),
        };
        let panel = self.open_chat(window, cx);
        panel.update(cx, |panel, cx| panel.add_reference(reference, window, cx));
    }

    pub(crate) fn reveal_reference(
        &mut self,
        reference: &Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, sheet)) = self.sheet_of_reference(reference.sheet.as_deref()) else {
            let sheet = reference.sheet.clone().unwrap_or_default();
            self.notify(
                Severity::Warning,
                t!("chat.reference.not_open", sheet = sheet),
                cx,
            );
            return;
        };
        self.switch_to(id, window, cx);
        if self.documents.active_id() != Some(id) {
            return;
        }
        self.switch_sheet(sheet, window, cx);
        self.grid
            .update(cx, |grid, cx| grid.show_range(reference.range, cx));
    }

    // The active workbook wins, then any other open workbook with a sheet of that name.
    fn sheet_of_reference(&self, name: Option<&str>) -> Option<(WorkbookId, SheetId)> {
        let in_document = |document: &Document| match name {
            Some(name) => document
                .sheets
                .iter()
                .find(|info| info.name.to_lowercase() == name.to_lowercase())
                .map(|info| info.id),
            None => Some(document.sheet),
        };
        self.documents
            .active()
            .and_then(|document| in_document(document).map(|sheet| (document.id, sheet)))
            .or_else(|| {
                name?;
                self.documents
                    .iter()
                    .find_map(|document| in_document(document).map(|sheet| (document.id, sheet)))
            })
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
            self.notify(Severity::Info, t!("chat.no_question"), cx);
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

    pub(super) fn chat_open(&self) -> bool {
        self.chat.open
    }

    pub(super) fn render_chat(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let panel = self.chat.panel.clone().filter(|_| self.chat.open)?;
        let width = self.panel_width(Panel::Chat, window);
        Some(
            div()
                .relative()
                .flex_shrink_0()
                .h_full()
                .w(px(width))
                .child(panel)
                .child(self.panel_edge(Panel::Chat, window, cx)),
        )
    }

    pub(super) fn shutdown_chat(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<std::sync::Arc<zenkai_agent::chat::process::ProcessTree>> {
        let panel = self.chat.panel.clone()?;
        panel.update(cx, |panel, _| panel.shutdown())
    }
}
