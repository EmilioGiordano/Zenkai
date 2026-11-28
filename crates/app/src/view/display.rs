use gpui_kit::*;
use zenkai_engine::Engine;
use zenkai_i18n::t;

use super::{Severity, Workspace};
use crate::agent_settings;
use crate::palette::Toggles;

#[derive(Clone, Copy)]
enum SheetFlag {
    Gridlines,
    Headings,
}

pub(super) fn formula_bar_shown(cx: &App) -> bool {
    agent_settings::settings(cx).general.formula_bar
}

impl Workspace {
    pub(super) fn palette_toggles(&self, cx: &App) -> Toggles {
        let hidden = self.grid.read(cx).hidden();
        let on_screen = self.documents.sheet_on_screen().is_some();
        Toggles {
            gridlines: on_screen.then_some(!hidden.gridlines),
            headings: on_screen.then_some(!hidden.headings),
            formula_bar: formula_bar_shown(cx),
        }
    }

    pub(super) fn toggle_gridlines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sheet_flag(SheetFlag::Gridlines, window, cx);
    }

    pub(super) fn toggle_headings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sheet_flag(SheetFlag::Headings, window, cx);
    }

    // The flag is set to what the grid shows, flipped, so the notice always says what
    // the user sees change. The edit goes through the same path as Freeze panes: undoable,
    // marks the workbook modified and is refused when it is read-only.
    fn toggle_sheet_flag(&mut self, flag: SheetFlag, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.sheet_on_screen() else {
            return;
        };
        let sheet = document.sheet;
        let hidden = self.grid.read(cx).hidden();
        let (show, queued) = match flag {
            SheetFlag::Gridlines => {
                let show = hidden.gridlines;
                (
                    show,
                    self.edit(window, cx, move |wb| wb.set_gridlines_shown(sheet, show)),
                )
            }
            SheetFlag::Headings => {
                let show = hidden.headings;
                (
                    show,
                    self.edit(window, cx, move |wb| wb.set_headings_shown(sheet, show)),
                )
            }
        };
        if !queued {
            return;
        }
        let message = match (flag, show) {
            (SheetFlag::Gridlines, true) => t!("notice.gridlines_shown"),
            (SheetFlag::Gridlines, false) => t!("notice.gridlines_hidden"),
            (SheetFlag::Headings, true) => t!("notice.headings_shown"),
            (SheetFlag::Headings, false) => t!("notice.headings_hidden"),
        };
        self.notify(Severity::Info, message, cx);
    }

    // The notice comes from the result of writing the setting, so it never claims a change
    // that did not happen.
    pub(super) fn toggle_formula_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = formula_bar_shown(cx);
        if shown {
            self.leave_formula_bar(window, cx);
        }
        let workspace = cx.entity().downgrade();
        agent_settings::change_then(
            cx,
            move |settings| settings.general.formula_bar = !shown,
            move |outcome, cx| {
                let updated = workspace.update(cx, |this, cx| match outcome {
                    Ok(()) if shown => {
                        this.notify(Severity::Info, t!("notice.formula_bar_hidden"), cx)
                    }
                    Ok(()) => this.notify(Severity::Info, t!("notice.formula_bar_shown"), cx),
                    Err(text) => this.notify(Severity::Warning, text, cx),
                });
                if let Err(error) = updated {
                    tracing::debug!(%error, "workspace closed before the formula bar notice");
                }
            },
        );
    }

    // Another window or a file edit may hide the bar while this window is using it.
    pub(super) fn close_hidden_formula_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !formula_bar_shown(cx) {
            self.leave_formula_bar(window, cx);
        }
    }

    // Text typed in the bar is entered first, as Save does, and focus never stays on the
    // Name Box that is about to stop being drawn, unless Go To has put its input there.
    fn leave_formula_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_formula_bar(true, window, cx);
        if self.go_to.is_none() && self.name_box_focus.is_focused(window) {
            let grid = self.grid.focus_handle(cx);
            window.focus(&grid, cx);
        }
    }
}
