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
        if !sheet_tabs::can_hide(&document.sheets) {
            self.notify(Severity::Warning, t!("notice.last_visible_sheet"), cx);
            return;
        }
        let sheet = document.sheet;
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
        let strip = sheet_tabs::shown(&document.sheets);
        if let Some(next) = sheet_tabs::neighbour(&strip, document.sheet, forward) {
            self.switch_sheet(next, window, cx);
        }
    }

    // A sheet that would be the last visible one is never deleted, as in Excel.
    pub(super) fn deleting_last_visible_sheet(&self) -> bool {
        self.documents.active().is_some_and(|document| {
            let visible = |sheet: &zenkai_types::SheetInfo| {
                sheet.visibility == zenkai_types::SheetVisibility::Visible
            };
            document
                .sheets
                .iter()
                .any(|sheet| sheet.id == document.sheet && visible(sheet))
                && document
                    .sheets
                    .iter()
                    .filter(|sheet| visible(sheet))
                    .count()
                    < 2
        })
    }
}
