use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::close_prompt::{Departure, Next, Purpose, Settled};
use crate::entry::{Entry, LinkStatus};

impl Workspace {
    pub(super) fn begin_departure(
        &mut self,
        departure: Departure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.departure = Some(departure);
        self.continue_departure(window, cx);
    }

    fn continue_departure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = match self.departure.as_mut() {
            Some(departure) => departure.next(),
            None => return,
        };
        match next {
            Next::Save(id) => self.save_for_departure(id, window, cx),
            Next::Done => self.finish_departure(window, cx),
        }
    }

    // The workbook comes on screen first so the user sees which one is being saved, and a
    // Save As dialog for a workbook never saved has a visible subject.
    fn save_for_departure(&mut self, id: WorkbookId, window: &mut Window, cx: &mut Context<Self>) {
        let is_link = matches!(self.documents.entry(id), Some(Entry::Link(_)));
        if self.documents.entry(id).is_none() {
            self.save_ended(id, false, window, cx);
            return;
        }
        self.switch_to(id, window, cx);
        if !is_link {
            self.save_document(id, window, cx);
            return;
        }
        let loading = matches!(
            self.documents.entry(id),
            Some(Entry::Link(link)) if link.status == LinkStatus::Loading
        );
        if !loading {
            self.save_ended(id, false, window, cx);
        }
    }

    pub(super) fn departure_load_finished(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.departure.as_ref().and_then(Departure::current) != Some(id) {
            return;
        }
        if self.documents.get(id).is_some() {
            self.save_document(id, window, cx);
        } else {
            self.save_ended(id, false, window, cx);
        }
    }

    // Every exit of the save chain reports here, whether the workbook ended up saved or not.
    pub(super) fn save_ended(
        &mut self,
        id: WorkbookId,
        saved: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settled = match self.departure.as_mut() {
            Some(departure) => departure.settled(id, saved),
            None => return,
        };
        match settled {
            Settled::Ignored => {}
            Settled::Continue => self.continue_departure(window, cx),
            Settled::Aborted => self.abort_departure(cx),
        }
    }

    // Save As chose a CSV: the export leaves the workbook unsaved, so the close stops, and the
    // user is told why.
    pub(super) fn csv_ended_departure(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waiting = self.departure.as_ref().and_then(Departure::current) == Some(id);
        self.save_ended(id, false, window, cx);
        if waiting {
            self.notify(Severity::Warning, t!("close.csv_kept_open"), cx);
        }
    }

    fn finish_departure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(departure) = self.departure.take() else {
            return;
        };
        match departure.purpose {
            Purpose::Tab(id) => self.finish_close(id, window, cx),
            Purpose::App => self.run_close(departure.plan, window, cx),
        }
    }

    fn abort_departure(&mut self, cx: &mut Context<Self>) {
        let Some(departure) = self.departure.take() else {
            return;
        };
        if departure.purpose == Purpose::App {
            self.notify(Severity::Warning, t!("close.not_closed"), cx);
        }
    }
}
