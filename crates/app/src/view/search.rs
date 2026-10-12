use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::*;
use zenkai_i18n::t;

use super::{Workspace, command_overlay};
use crate::actions::CloseSearch;
use crate::file_search::{self, Group, Target};

pub(super) struct SearchOverlay {
    state: Entity<CommandState>,
    // Kept as built: the palette reports a choice as a group and a row.
    groups: Vec<Group>,
}

impl Workspace {
    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self
            .search
            .as_ref()
            .is_some_and(|search| search.state.focus_handle(cx).contains_focused(window, cx));
        if focused {
            self.close_search(window, cx);
            return;
        }
        self.palette = None;
        self.revert_theme_preview(cx);
        let groups = file_search::collect(&self.documents, &self.recent);
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |state, cx| state.focus(window, cx));
        self.search = Some(SearchOverlay { state, groups });
        cx.notify();
    }

    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn confirm_search(
        &mut self,
        section: usize,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .search
            .as_ref()
            .and_then(|search| search.groups.get(section))
            .and_then(|group| group.hits.get(row))
            .map(|hit| hit.target.clone());
        self.close_search(window, cx);
        match target {
            Some(Target::Workbook(id)) => self.switch_to(id, window, cx),
            Some(Target::Sheet(id, sheet)) => {
                self.switch_to(id, window, cx);
                if self.documents.active_id() == Some(id) {
                    self.switch_sheet(sheet, window, cx);
                }
            }
            Some(Target::Recent(path)) => self.open_path(path, window, cx),
            None => {}
        }
    }

    pub(super) fn render_search(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.search.as_ref()?;
        let entity = cx.entity().downgrade();
        let command = search
            .groups
            .iter()
            .map(|group| {
                CommandGroup::new()
                    .label(group.heading)
                    .items(group.hits.iter().map(|hit| {
                        let (label, detail) = (hit.label.clone(), hit.detail.clone());
                        CommandItem::new()
                            .label(hit.label.clone())
                            .keywords([hit.detail.clone()])
                            .child(move |_, cx| {
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .child(div().child(label.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(detail.clone()),
                                    )
                            })
                    }))
            })
            .fold(Command::new(&search.state), Command::group)
            .placeholder(t!("search.placeholder"))
            .on_confirm(move |path, window, cx| {
                let update = entity.update(cx, |this, cx| {
                    this.confirm_search(path.section, path.row, window, cx)
                });
                if let Err(error) = update {
                    tracing::debug!(%error, "workspace dropped");
                }
            })
            .on_cancel(|window, cx| window.dispatch_action(Box::new(CloseSearch), cx));
        Some(command_overlay(command, "SearchOverlay", || Box::new(CloseSearch)).into_any_element())
    }
}
