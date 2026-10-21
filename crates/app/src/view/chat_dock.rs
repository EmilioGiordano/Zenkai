use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::base::Selectable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::thread::FileCard;
use zenkai_agent::settings::PermissionMode;
use zenkai_agent::tools::AgentAccess;
use zenkai_i18n::t;
use zenkai_types::{Range, SheetId, SheetVisibility, WorkbookId};

use super::{Severity, Workspace, sidebar};
use crate::actions::*;
use crate::agent_folder::FolderHint;
use crate::agent_routing;
use crate::chat::mention_catalog::{ActiveBook, Catalog, OpenBook, SheetEntry, Used};
use crate::chat::reference::Reference;
use crate::chat::{ChatEvent, ChatPanel, reference};
use crate::chat_layout::{ChatLayout, ChatLayoutState};
use crate::document::{Document, SharedWorkbook};
use crate::keymap;
use crate::panel_width::Panel;

#[derive(Default)]
pub(super) struct ChatDock {
    pub(super) panel: Option<Entity<ChatPanel>>,
    pub(super) open: bool,
    layout: ChatLayoutState,
    pub(super) synced_change: Option<(u64, bool)>,
    events: Option<Subscription>,
}

macro_rules! leave_full_window_before {
    ($root:expr, $cx:expr; $($action:ty),+ $(,)?) => {{
        let mut root = $root;
        $(
            root = root.capture_action($cx.listener(|this, _: &$action, _, cx| {
                this.show_sheet_beside_chat(cx)
            }));
        )+
        root
    }};
}

impl Workspace {
    // Commands the composer does not swallow and that act on the sheet or the open workbooks.
    pub(super) fn leave_full_window_for_sheet_commands<E: InteractiveElement>(
        root: E,
        cx: &mut Context<Self>,
    ) -> E {
        leave_full_window_before!(
            root, cx;
            Open, NewWorkbook, NextDocument, PreviousDocument, ReopenClosedDocument,
            NextSheet, PreviousSheet, NewSheet, DuplicateSheet, DeleteSheet, RenameSheet,
            MoveSheetLeft, MoveSheetRight, Find, Replace, GoTo, GenerateData, InsertChart,
            ToggleFormulas, ZoomIn, ZoomOut, ZoomReset, Undo, Redo, PasteValues,
            SortAscending, SortDescending, FreezeTopRow, FreezeFirstColumn, FreezePanes,
            InsertRows, DeleteRows, InsertColumns, DeleteColumns, HideRows, UnhideRows,
            HideColumns, UnhideColumns, AlignLeft, AlignCenter, AlignRight, BordersAll,
            BordersOutside, BorderBottom, BordersNone, ToggleWrapText, IncreaseDecimal,
            DecreaseDecimal, FormatNumber, FormatCurrency, FormatPercent, FormatDate,
            FormatGeneral, ClearFormats, ClearAll, NoFill, AutomaticFontColor, GrowFont,
            ShrinkFont,
        )
    }

    pub(crate) fn active_workbook_for_chat(&self) -> Option<(WorkbookId, String)> {
        let document = self.documents.active()?;
        Some((document.id, document.name()))
    }

    pub(crate) fn agent_access_for_chat(&self, permission: PermissionMode) -> AgentAccess {
        self.documents
            .active()
            .map_or(AgentAccess::Editable, |document| {
                agent_routing::access(document, permission)
            })
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

    // Sheet names and the selection only; used ranges are read off the UI thread afterwards.
    pub(crate) fn mention_catalog_for_chat(&self, cx: &App) -> Catalog {
        let open: Vec<&Document> = self.documents.iter().collect();
        catalog_of(&open, self.documents.active(), self.selection(cx))
    }

    pub(crate) fn shared_workbook_for_chat(&self, id: WorkbookId) -> Option<SharedWorkbook> {
        self.documents
            .iter()
            .find(|document| document.id == id)
            .map(Document::shared_workbook)
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
                ChatEvent::Leave if this.chat_fills_window() => {}
                ChatEvent::Leave => {
                    let focus = this.grid.focus_handle(cx);
                    window.focus(&focus, cx);
                }
                // Shown without taking the focus: the user may be typing in the grid.
                ChatEvent::ViewChanged => cx.notify(),
                ChatEvent::NeedsAnswer => {
                    this.chat.open = true;
                    cx.notify();
                }
                ChatEvent::ResumeSession => {
                    if let Some(panel) = this.chat.panel.clone() {
                        panel.update(cx, |panel, cx| panel.continue_pending_resume(window, cx));
                    }
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
        self.sync_chat_change(cx);
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
            workbook: None,
            sheet: Some(sheet),
            range: self.selection(cx),
        };
        if !reference.has_safe_names() {
            self.notify(Severity::Warning, t!("chat.reference.unsafe_name"), cx);
            return;
        }
        let panel = self.open_chat(window, cx);
        panel.update(cx, |panel, cx| panel.add_reference(reference, window, cx));
    }

    pub(crate) fn reveal_reference(
        &mut self,
        reference: &Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, sheet)) =
            self.sheet_of_reference(reference.workbook.as_deref(), reference.sheet.as_deref())
        else {
            let sheet = reference.sheet.clone().unwrap_or_default();
            self.notify(
                Severity::Warning,
                t!("chat.reference.not_open", sheet = sheet),
                cx,
            );
            return;
        };
        self.show_sheet_beside_chat(cx);
        self.switch_to(id, window, cx);
        if self.documents.active_id() != Some(id) {
            return;
        }
        self.switch_sheet(sheet, window, cx);
        self.grid
            .update(cx, |grid, cx| grid.show_range(reference.range, cx));
    }

    // A named workbook is the only place searched; otherwise the active workbook wins, then any
    // other open workbook with a sheet of that name.
    fn sheet_of_reference(
        &self,
        workbook: Option<&str>,
        name: Option<&str>,
    ) -> Option<(WorkbookId, SheetId)> {
        let in_document = |document: &Document| match name {
            Some(name) => document
                .sheets
                .iter()
                .find(|info| info.name.to_lowercase() == name.to_lowercase())
                .map(|info| info.id),
            None => Some(document.sheet),
        };
        if let Some(workbook) = workbook {
            return self
                .documents
                .iter()
                .filter(|document| document.name().to_lowercase() == workbook.to_lowercase())
                .find_map(|document| in_document(document).map(|sheet| (document.id, sheet)));
        }
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

    pub(super) fn tell_chat_editing_is_blocked(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.chat.panel.clone() {
            panel.update(cx, |panel, cx| panel.note_editing_blocked(cx));
        }
    }

    pub(super) fn chat_open(&self) -> bool {
        self.chat.open
    }

    pub(crate) fn chat_layout(&self) -> ChatLayout {
        self.chat.layout.current()
    }

    pub(super) fn chat_fills_window(&self) -> bool {
        self.chat.open && self.chat_layout() == ChatLayout::FullWindow
    }

    pub(super) fn show_sheet_beside_chat(&mut self, cx: &mut Context<Self>) {
        if self.chat_fills_window() {
            self.chat.layout = self.chat.layout.without_full_window();
            cx.notify();
        }
    }

    pub(super) fn change_chat_layout(
        &mut self,
        change: impl FnOnce(ChatLayoutState) -> ChatLayoutState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_open = self.chat.open;
        self.chat.layout = change(self.chat.layout);
        if !was_open {
            self.open_chat(window, cx);
            return;
        }
        // The sheet is not drawn in full window, so the focus cannot stay on it.
        if self.chat_fills_window()
            && let Some(panel) = self.chat.panel.clone()
        {
            panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
        }
        cx.notify();
    }

    pub(super) fn render_chat(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let panel = self.chat.panel.clone().filter(|_| self.chat.open)?;
        let fills_window = self.chat_fills_window();
        let layout = self.chat_layout();
        let range = self
            .selection_for_chat(cx)
            .filter(|_| layout == ChatLayout::FullWindow);
        panel.update(cx, |panel, cx| panel.show_workspace_view(layout, range, cx));
        let width = self.panel_width(Panel::Chat, window);
        Some(
            div()
                .relative()
                .h_full()
                .when(fills_window, |dock| dock.flex_1().min_w_0())
                .when(!fills_window, |dock| dock.flex_shrink_0().w(px(width)))
                .child(panel)
                .when(!fills_window, |dock| {
                    dock.child(self.panel_edge(Panel::Chat, window, cx))
                }),
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

fn catalog_of(open: &[&Document], active: Option<&Document>, selection: Range) -> Catalog {
    let open_book = |document: &Document| OpenBook {
        id: document.id,
        name: document.name(),
        sheets: document
            .sheets
            .iter()
            .filter(|info| info.visibility == SheetVisibility::Visible)
            .map(|info| info.name.clone())
            .collect(),
    };
    let active = active.map(|document| {
        let sheets = document
            .sheets
            .iter()
            .filter(|info| info.visibility == SheetVisibility::Visible)
            .map(|info| SheetEntry {
                id: info.id,
                name: info.name.clone(),
                used: Used::Unknown,
            })
            .collect();
        let selection = document
            .sheets
            .iter()
            .find(|info| info.id == document.sheet)
            .map(|info| Reference {
                workbook: None,
                sheet: Some(info.name.clone()),
                range: selection,
            });
        ActiveBook {
            book: open_book(document),
            sheets,
            selection,
        }
    });
    Catalog {
        active,
        open: open.iter().map(|document| open_book(document)).collect(),
        folder: Vec::new(),
        folder_sheets: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use zenkai_engine::{Engine, Workbook};
    use zenkai_types::{Range, SheetId, WorkbookId};

    use super::catalog_of;
    use crate::chat::mention_candidates::candidates;
    use crate::document::Document;
    use crate::spaces::SpaceId;

    fn heavy_document() -> Document {
        let mut workbook = Workbook::new_empty().unwrap();
        workbook.rename_sheet(SheetId(0), "Base").unwrap();
        for name in ["Dinámica", "Datos"] {
            let sheet = workbook.add_sheet().unwrap();
            workbook.rename_sheet(sheet, name).unwrap();
        }
        Document::new(
            WorkbookId(1),
            SpaceId(0),
            1,
            workbook,
            Some(PathBuf::from(
                "C:/work/Listas Accesorias DISTRIS - 2024.11.20.xlsx",
            )),
            Vec::new(),
        )
    }

    #[test]
    fn the_catalog_the_app_builds_offers_the_sheets_of_the_active_workbook() {
        let document = heavy_document();
        let catalog = catalog_of(
            &[&document],
            Some(&document),
            Range::parse_a1("A1").unwrap(),
        );
        for query in ["Base", "Listas", "dinámica"] {
            let labels: Vec<String> = candidates(query, &catalog)
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();
            for sheet in ["@Base", "@Dinámica", "@Datos"] {
                let wanted =
                    query == "Listas" || sheet.to_lowercase().contains(&query.to_lowercase());
                assert_eq!(
                    labels.contains(&sheet.to_string()),
                    wanted,
                    "{query} {sheet}"
                );
            }
        }
    }
}
