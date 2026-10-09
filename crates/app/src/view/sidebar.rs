use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState, SelectAll};
use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::entry::Entry;
use crate::sidebar_rows::{self, Move, Row, SpaceRows};
use crate::space_settings;
use crate::spaces::{Neighbour, SpaceColor, SpaceId, new_space_name};

pub(super) const WIDTH: f32 = 248.0;

mod render;

// A button toggling the sidebar answers the primary button and the keyboard only.
pub(super) fn is_primary_click(event: &ClickEvent) -> bool {
    match event {
        ClickEvent::Mouse(click) => {
            click.down.button == MouseButton::Left && click.up.button == MouseButton::Left
        }
        _ => true,
    }
}

pub(super) struct SidebarState {
    pub visible: bool,
    pub focus: FocusHandle,
    pub cursor: Option<Row>,
    pub recent_open: bool,
    pub renaming: Option<Renaming>,
}

pub(super) struct Renaming {
    space: SpaceId,
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl SidebarState {
    pub fn new(cx: &mut App) -> SidebarState {
        SidebarState {
            visible: false,
            focus: cx.focus_handle(),
            cursor: None,
            recent_open: false,
            renaming: None,
        }
    }
}

impl Workspace {
    fn unopened_recent(&self) -> Vec<&PathBuf> {
        self.recent
            .iter()
            .filter(|path| !self.documents.has_path(path))
            .collect()
    }

    fn sidebar_rows(&self) -> Vec<Row> {
        let spaces: Vec<SpaceRows> = self
            .documents
            .spaces()
            .iter()
            .map(|space| SpaceRows {
                id: space.id,
                collapsed: space.collapsed,
                files: self.documents.members(space.id).map(Entry::id).collect(),
            })
            .collect();
        sidebar_rows::rows(
            &spaces,
            self.unopened_recent().len(),
            self.sidebar.recent_open,
        )
    }

    pub(super) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.visible = !self.sidebar.visible;
        if !self.sidebar.visible && self.sidebar.focus.contains_focused(window, cx) {
            let focus = self.grid.focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    // F6 cycles between the panes, as in Excel: the grid, then the sidebar, then back.
    pub(super) fn focus_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.visible && self.sidebar.focus.contains_focused(window, cx) {
            self.leave_sidebar(window, cx);
            return;
        }
        self.sidebar.visible = true;
        if self.sidebar.cursor.is_none() {
            self.sidebar.cursor = self.documents.active_id().map(Row::File);
        }
        window.focus(&self.sidebar.focus, cx);
        cx.notify();
    }

    pub(super) fn leave_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn sidebar_step(&mut self, movement: Move, cx: &mut Context<Self>) {
        let rows = self.sidebar_rows();
        self.sidebar.cursor = sidebar_rows::step(&rows, self.sidebar.cursor, movement);
        cx.notify();
    }

    pub(super) fn set_space_color(
        &mut self,
        id: SpaceId,
        color: SpaceColor,
        cx: &mut Context<Self>,
    ) {
        self.documents.set_space_color(id, color);
        cx.notify();
    }

    pub(super) fn delete_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = match self.sidebar.cursor {
            Some(Row::File(id)) => Some(id),
            _ => self.documents.active_id(),
        };
        if let Some(id) = id {
            self.delete_file_of(id, window, cx);
        }
    }

    // Never deletes for good: the file goes to the Recycle Bin, then its workbook is closed.
    pub(super) fn delete_file_of(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.documents.entry(id) else {
            return;
        };
        let Some(path) = entry.path().map(PathBuf::from) else {
            self.notify(
                Severity::Warning,
                t!("notice.only_saved_files_deletable"),
                cx,
            );
            return;
        };
        let detail = t!("sidebar.delete_detail", path = path.display());
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("sidebar.delete_title", name = entry.name()),
            Some(&detail),
            &[t!("button.cancel"), t!("sidebar.move_to_recycle_bin")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(1) {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    if !path.is_file() {
                        return Err(t!("sidebar.not_on_disk").to_string());
                    }
                    trash::delete(&path).map_err(|error| error.to_string())
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.finish_close(id, window, cx);
                    this.notify(Severity::Info, t!("notice.moved_to_recycle_bin"), cx);
                }
                Err(error) => this.notify(
                    Severity::Error,
                    t!("notice.delete_failed", error = error),
                    cx,
                ),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during delete");
            }
        })
        .detach();
    }

    pub(super) fn cycle_space_color(&mut self, cx: &mut Context<Self>) {
        let target = self
            .sidebar_cursor_space()
            .unwrap_or_else(|| self.documents.current_space());
        let Some(current) = self.documents.spaces().get(target).map(|space| space.color) else {
            return;
        };
        self.set_space_color(target, current.next(), cx);
    }

    pub(super) fn sidebar_cursor_space(&self) -> Option<SpaceId> {
        match self.sidebar.cursor? {
            Row::Space(id) => Some(id),
            Row::File(id) => self.documents.get(id).map(|document| document.space),
            _ => None,
        }
    }

    fn sidebar_collapse(&mut self, expanded: bool, cx: &mut Context<Self>) {
        match self.sidebar.cursor {
            Some(Row::Space(id)) => self.documents.expand_space(id, expanded),
            Some(Row::RecentHeader | Row::Recent(_)) => self.sidebar.recent_open = expanded,
            Some(Row::File(id)) if !expanded => {
                if let Some(space) = self.documents.get(id).map(|document| document.space) {
                    self.sidebar.cursor = Some(Row::Space(space));
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn sidebar_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.sidebar.cursor {
            Some(Row::NewWorkbook) => self.create_document(window, cx),
            Some(Row::SearchFiles) => self.toggle_search(window, cx),
            Some(Row::NewSpace) => self.new_space(window, cx),
            Some(Row::Space(id)) => self.documents.toggle_space(id),
            Some(Row::File(id)) => {
                self.switch_to(id, window, cx);
            }
            Some(Row::RecentHeader) => self.sidebar.recent_open = !self.sidebar.recent_open,
            Some(Row::Recent(index)) => {
                if let Some(path) = self.unopened_recent().get(index).copied().cloned() {
                    self.open_path(path, window, cx);
                }
            }
            None => {}
        }
        cx.notify();
    }

    fn sidebar_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.sidebar.cursor {
            Some(Row::File(id)) => self.close_document(id, window, cx),
            _ => self.delete_space(cx),
        }
    }

    pub(super) fn new_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.visible = true;
        let id = self.documents.add_space(
            new_space_name(),
            space_settings::current(cx).new_space_color,
        );
        self.sidebar.cursor = Some(Row::Space(id));
        self.begin_space_rename(id, window, cx);
    }

    pub(super) fn rename_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .sidebar_cursor_space()
            .unwrap_or_else(|| self.documents.current_space());
        self.sidebar.visible = true;
        self.begin_space_rename(target, window, cx);
    }

    fn begin_space_rename(&mut self, space: SpaceId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.documents.spaces().get(space).map(|s| s.name.clone()) else {
            return;
        };
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(t!("space.name_placeholder"));
            state.set_value(name, window, cx);
            state
        });
        let subscription =
            cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let name = input.read(cx).value().to_string();
                    this.documents.rename_space(space, &name);
                    this.close_space_rename(window, cx);
                }
            });
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        // The input only handles the action once it has been drawn.
        window.on_next_frame(|window, _| {
            window.on_next_frame(|window, cx| window.dispatch_action(SelectAll.boxed_clone(), cx));
        });
        window.refresh();
        self.sidebar.renaming = Some(Renaming {
            space,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn close_space_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.renaming = None;
        window.focus(&self.sidebar.focus, cx);
        cx.notify();
    }

    pub(super) fn delete_space(&mut self, cx: &mut Context<Self>) {
        let target = self
            .sidebar_cursor_space()
            .unwrap_or_else(|| self.documents.current_space());
        let count = self.documents.members(target).count();
        if !self.documents.delete_space(target) {
            self.notify(Severity::Warning, t!("notice.last_space"), cx);
            return;
        }
        self.sidebar.cursor = None;
        if count > 0 {
            self.notify(
                Severity::Info,
                t!("notice.moved_workbooks", count = count),
                cx,
            );
        }
        cx.notify();
    }

    fn move_document(&mut self, id: WorkbookId, space: SpaceId, cx: &mut Context<Self>) {
        if self.documents.move_to_space(id, space) {
            self.sidebar.cursor = Some(Row::File(id));
            self.documents.expand_space(space, true);
        }
        cx.notify();
    }

    pub(super) fn shift_document(&mut self, side: Neighbour, cx: &mut Context<Self>) {
        let id = match self.sidebar.cursor {
            Some(Row::File(id)) => Some(id),
            _ => self.documents.active_id(),
        };
        let Some(id) = id else {
            return;
        };
        if self.documents.shift_space(id, side) {
            self.sidebar.cursor = Some(Row::File(id));
            if let Some(space) = self.documents.get(id).map(|document| document.space) {
                self.documents.expand_space(space, true);
            }
        }
        cx.notify();
    }
}
