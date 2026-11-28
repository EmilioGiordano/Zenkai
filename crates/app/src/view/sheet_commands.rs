use gpui_kit::*;
use zenkai_engine::Engine;
use zenkai_i18n::t;

use super::{Severity, Workspace};
use crate::sheet_tabs;

impl Workspace {
    // The sheet it hides is the active one; the sheet that takes its place is settled once
    // the edit has run, from the workbook as it is then.
    pub(super) fn hide_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let sheet = document.sheet;
        if !sheet_tabs::can_hide(&document.sheets, sheet) {
            self.notify(Severity::Warning, t!("notice.last_visible_sheet"), cx);
            return;
        }
        self.edit(window, cx, move |wb| wb.hide_sheet(sheet));
    }

    // Ctrl+PageDown and Ctrl+PageUp step through the tab strip, past the hidden sheets.
    pub(super) fn step_sheet(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let strip: Vec<_> = document.strip().iter().map(|tab| tab.id).collect();
        if let Some(next) = sheet_tabs::neighbour(&strip, document.sheet, forward) {
            self.switch_sheet(next, window, cx);
        }
    }

    // For a sheet an agent, a chat reference or a search names: a hidden one cannot be shown
    // without unhiding it, and the caller must not go on to select cells on another sheet.
    pub(super) fn show_sheet(
        &mut self,
        sheet: zenkai_types::SheetId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(document) = self.documents.active() else {
            return false;
        };
        if sheet == document.sheet {
            return true;
        }
        if !document.is_shown(sheet) {
            self.notify(Severity::Info, t!("notice.sheet_hidden"), cx);
            return false;
        }
        self.switch_sheet(sheet, window, cx);
        true
    }

    // What the Tab color card and the Unhide dialog chose from a sheet list that has changed
    // since cannot be applied, as the sheets they name may have moved: they close with no change.
    pub(super) fn close_sheet_dialogs_gone_stale(&mut self, generation: u64, version: u64) {
        let stale = self
            .tab_color
            .as_ref()
            .is_some_and(|card| card.generation == generation && card.sheets_version != version);
        let unhide_stale = self.unhide.as_ref().is_some_and(|dialog| {
            dialog.generation == generation && dialog.sheets_version != version
        });
        if stale {
            self.tab_color = None;
        }
        if unhide_stale {
            self.unhide = None;
        }
        self.refocus_grid |= stale || unhide_stale;
    }

    // A sheet that would be the last visible one is never deleted, as in Excel.
    pub(super) fn deleting_last_visible_sheet(&self) -> bool {
        self.documents
            .active()
            .is_some_and(|document| sheet_tabs::only_visible(&document.sheets, document.sheet))
    }
}
