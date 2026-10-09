use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Datelike;
use gpui_kit::*;
use zenkai_datagen::Date;
use zenkai_engine::Engine;
use zenkai_i18n::t;
use zenkai_types::Range;

use super::{Severity, Workspace};
use crate::document;
use crate::generate_dialog::{DialogEvent, GenerateDialog, Layout, Opening, Target};

pub(super) struct GenerateSession {
    dialog: Entity<GenerateDialog>,
    _events: Subscription,
}

// Seeds stay short enough to read and retype; the dialog shows the one it used.
fn fresh_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64 % 100_000)
}

fn today() -> Option<Date> {
    let now = chrono::Local::now().date_naive();
    Date::from_ymd(i64::from(now.year()), now.month(), now.day())
}

impl Workspace {
    pub(super) fn open_generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.generate.is_some() || self.user_is_editing(cx) {
            return;
        }
        let Some(document) = self.documents.active() else {
            return;
        };
        if document.read_only {
            self.notify(Severity::Warning, t!("generate.read_only"), cx);
            return;
        }
        let Some(today) = today() else {
            self.notify(Severity::Error, t!("generate.date_out_of_range"), cx);
            return;
        };
        let (target, sheet) = (
            Target {
                document: document.id,
                generation: document.generation(),
                sheet: document.sheet,
            },
            document.sheet,
        );
        let sheet_name = document
            .sheets
            .get(sheet.0 as usize)
            .map(|info| info.name.clone())
            .unwrap_or_default();
        let selection = self.selection(cx);
        let Some(workbook) = document.workbook() else {
            self.notify(Severity::Warning, t!("generate.recalculating"), cx);
            return;
        };
        let layout = {
            let Some(contents) = document::contents_of(&workbook, sheet) else {
                return;
            };
            Layout::from_selection(
                selection,
                workbook.used_end(sheet),
                |pos| contents(pos).is_filled(),
                |pos| workbook.input(sheet, pos),
            )
        };
        drop(workbook);
        let opening = Opening {
            layout,
            seed: fresh_seed(),
            today,
            target,
            sheet_name: sheet_name.into(),
        };
        let dialog = cx.new(|cx| GenerateDialog::new(opening, window, cx));
        let events = cx.subscribe_in(&dialog, window, Self::on_generate_event);
        dialog.update(cx, |dialog, cx| dialog.focus_first(window, cx));
        self.generate = Some(GenerateSession {
            dialog,
            _events: events,
        });
        cx.notify();
    }

    fn on_generate_event(
        &mut self,
        _: &Entity<GenerateDialog>,
        event: &DialogEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DialogEvent::Close => self.close_generate(window, cx),
            DialogEvent::Relayout(range) => self.relayout_generate(*range, window, cx),
            DialogEvent::Write => self.write_generated(window, cx),
        }
    }

    fn close_generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generate = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn generate_target_is_active(&self, target: Target) -> bool {
        self.documents.active().is_some_and(|document| {
            document.id == target.document
                && document.generation() == target.generation
                && document.sheet == target.sheet
        })
    }

    fn relayout_generate(&mut self, range: Range, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.generate.as_ref().map(|session| session.dialog.clone()) else {
            return;
        };
        let target = dialog.read(cx).target();
        if !self.generate_target_is_active(target) {
            return;
        }
        let Some(workbook) = self
            .documents
            .active()
            .and_then(|document| document.workbook())
        else {
            return;
        };
        let layout = Layout::from_range(range, |pos| workbook.input(target.sheet, pos));
        drop(workbook);
        dialog.update(cx, |dialog, cx| dialog.apply_layout(layout, window, cx));
    }

    // Headers and rows go in as one rectangular write, so one Ctrl+Z undoes both.
    fn write_generated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(write) = self
            .generate
            .as_ref()
            .and_then(|session| session.dialog.update(cx, |dialog, _| dialog.take_write()))
        else {
            return;
        };
        self.close_generate(window, cx);
        if !self.generate_target_is_active(write.target) {
            self.notify(Severity::Warning, t!("generate.target_changed"), cx);
            return;
        }
        let sheet = write.target.sheet;
        let (origin, rows, selection) =
            (write.block.origin, write.block.rows, write.block.selection);
        self.edit(window, cx, move |workbook| {
            workbook.set_inputs(sheet, origin, &rows)
        });
        self.grid.update(cx, |grid, cx| {
            grid.select(selection.start, selection.end, cx)
        });
        self.notify(Severity::Info, write.notice, cx);
    }

    pub(super) fn render_generate(&self) -> Option<Entity<GenerateDialog>> {
        self.generate.as_ref().map(|session| session.dialog.clone())
    }
}
