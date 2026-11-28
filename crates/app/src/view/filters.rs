use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use gpui_kit::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_grid::FilterBar;
use zenkai_i18n::t;
use zenkai_types::{CellPos, ColIdx, FilterView, SheetId, WorkbookId};

use super::{Document, Severity, Workspace};
use crate::document;
use crate::filter::list_state::Outcome;
use crate::filter::list_view::{FilterListEvent, FilterListView};
use crate::filter::texts::{button_label, column_name};
use crate::filter::toggle_range::toggle_range;
use crate::job_pill::LongJob;
use crate::region;

const PENDING: u8 = 0;
const APPLIED: u8 = 1;
const SKIPPED: u8 = 2;
const UNDONE: u8 = 3;
const FAILED: u8 = 4;
const MAX_LABELLED_COLUMNS: u16 = 1_000;

// A filter being applied in the background. The engine cannot stop a write half way, so a
// cancel takes effect before it starts, or takes it back right after: the rows are left as
// they were either way.
pub(super) struct FilterJob {
    document: WorkbookId,
    cancel: Arc<AtomicBool>,
    outcome: Arc<AtomicU8>,
    canceled_by_edit: bool,
}

pub(super) struct FilterSession {
    list: Entity<FilterListView>,
    column: ColIdx,
    _events: Subscription,
}

impl FilterSession {
    pub(super) fn list(&self) -> &Entity<FilterListView> {
        &self.list
    }
}

pub(super) fn filter_bar(workbook: &Workbook, sheet: SheetId) -> Option<FilterBar> {
    let view = workbook.auto_filter(sheet)?;
    let first = view.range.start.col.get();
    let last = view
        .range
        .end
        .col
        .get()
        .min(first.saturating_add(MAX_LABELLED_COLUMNS));
    let labels = (first..=last)
        .map(|col| {
            let col = ColIdx::clamped(i64::from(col));
            let header = workbook
                .cell(sheet, CellPos::new(view.range.start.row, col))
                .text;
            button_label(&view, col, &column_name(&header, col)).into()
        })
        .collect();
    Some(FilterBar { view, labels })
}

impl Workspace {
    pub(super) fn toggle_auto_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        self.close_filter_list(window, cx);
        let selection = self.grid.read(cx).selection();
        let range = match &self.filter_view {
            Some(view) => Some(view.range),
            None => self
                .documents
                .active()
                .and_then(Document::workbook)
                .and_then(|workbook| {
                    let contents = document::contents_of(&workbook, sheet)?;
                    let region =
                        region::current_region(selection.active, |pos| contents(pos).is_filled());
                    Some(toggle_range(
                        selection.range(),
                        region,
                        workbook.used_end(sheet),
                    ))
                }),
        };
        let Some(range) = range else {
            self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
            return;
        };
        self.edit(window, cx, move |wb| wb.toggle_auto_filter(sheet, range));
    }

    pub(super) fn clear_all_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        if self
            .filter_view
            .as_ref()
            .is_some_and(FilterView::is_filtering)
        {
            self.close_filter_list(window, cx);
            self.edit(window, cx, move |wb| wb.clear_filters(sheet));
        }
    }

    // Alt+Down on a header cell of the filter range.
    pub(super) fn open_filter_list_at_active(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.grid.read(cx).selection().active;
        let on_header = self
            .grid
            .read(cx)
            .filter()
            .is_some_and(|bar| bar.is_header_cell(active));
        if on_header {
            self.open_filter_list(active.col, window, cx);
        }
    }

    pub(super) fn open_filter_list(
        &mut self,
        col: ColIdx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .filter_list
            .as_ref()
            .is_some_and(|open| open.column == col)
        {
            self.close_filter_list(window, cx);
            return;
        }
        let Some(view) = self.filter_view.clone() else {
            return;
        };
        if self.prompts.blocks_input() || self.filter_job.is_some() {
            return;
        }
        let Some(anchor) = self.grid.read(cx).filter_anchor(col) else {
            return;
        };
        let header = self
            .documents
            .active()
            .and_then(|document| Some((document.workbook()?, document.sheet)))
            .map(|(wb, sheet)| wb.cell(sheet, CellPos::new(view.range.start.row, col)).text)
            .unwrap_or_default();
        let name = column_name(&header, col);
        let filtered = view.criteria.iter().any(|(column, _)| *column == col);
        let list = cx.new(|cx| FilterListView::new(col, name, filtered, anchor, window, cx));
        let events = cx.subscribe_in(&list, window, Self::on_filter_list_event);
        let request = list.read(cx).request();
        self.filter_list = Some(FilterSession {
            list,
            column: col,
            _events: events,
        });
        self.grid
            .update(cx, |grid, cx| grid.set_filter_open(Some(col), cx));
        self.read_filter_values(request, String::new(), cx);
        cx.notify();
    }

    pub(super) fn close_filter_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_list.take().is_none() {
            return;
        }
        self.grid
            .update(cx, |grid, cx| grid.set_filter_open(None, cx));
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn on_filter_list_event(
        &mut self,
        list: &Entity<FilterListView>,
        event: &FilterListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let col = list.read(cx).column();
        match event {
            FilterListEvent::Close => self.close_filter_list(window, cx),
            FilterListEvent::Search { text, request } => {
                self.read_filter_values(*request, text.clone(), cx)
            }
            FilterListEvent::Retry { request } => {
                let text = list.read(cx).search_text().to_string();
                self.read_filter_values(*request, text, cx);
            }
            FilterListEvent::Sort { descending } => {
                let descending = *descending;
                self.close_filter_list(window, cx);
                self.edit(window, cx, move |wb| wb.sort_filter(sheet, col, descending));
            }
            FilterListEvent::ClearColumn => {
                self.close_filter_list(window, cx);
                self.edit(window, cx, move |wb| wb.set_column_filter(sheet, col, None));
            }
            FilterListEvent::Apply(outcome) => {
                let outcome = outcome.clone();
                self.close_filter_list(window, cx);
                self.apply_filter(sheet, col, outcome, window, cx);
            }
        }
    }

    // The values are read off the UI thread; an answer for a list that has since changed
    // its request is dropped by the list.
    fn read_filter_values(&mut self, request: u64, search: String, cx: &mut Context<Self>) {
        let (Some(session), Some(document)) = (&self.filter_list, self.documents.active()) else {
            return;
        };
        let (list, col) = (session.list.clone(), session.column);
        let (shared, sheet) = (document.shared_workbook(), document.sheet);
        cx.spawn(async move |this, cx| {
            let result =
                cx.background_executor()
                    .spawn(async move {
                        document::read_shared(&shared).filter_values(sheet, col, &search)
                    })
                    .await;
            let update = this.update(cx, |_, cx| {
                let list_result = match result {
                    Ok(values) => Some(values),
                    Err(error) => {
                        tracing::warn!(%error, "could not read the values of a filter column");
                        None
                    }
                };
                list.update(cx, |list, cx| list.loaded(request, list_result, cx));
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while reading filter values");
            }
        })
        .detach();
    }

    fn apply_filter(
        &mut self,
        sheet: SheetId,
        col: ColIdx,
        outcome: Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = match outcome {
            Outcome::Unchanged => return,
            Outcome::Clear => None,
            Outcome::Set(shown) => Some(shown),
        };
        if self.prompts.blocks_input() {
            return;
        }
        let Some(id) = self.documents.active_id() else {
            return;
        };
        if self.clipboard_source.take().is_some() {
            self.grid.update(cx, |grid, cx| grid.set_marquee(None, cx));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = Arc::new(AtomicU8::new(PENDING));
        let (job_cancel, job_outcome) = (cancel.clone(), outcome.clone());
        let queued = self.edit_document(id, window, cx, move |wb| {
            if job_cancel.load(Ordering::SeqCst) {
                job_outcome.store(SKIPPED, Ordering::SeqCst);
                return Ok(());
            }
            let ended = match wb.set_column_filter(sheet, col, shown) {
                Err(error) => {
                    tracing::warn!(%error, "a filter could not be applied");
                    FAILED
                }
                Ok(()) if job_cancel.load(Ordering::SeqCst) => match wb.undo() {
                    Ok(()) => UNDONE,
                    Err(error) => {
                        tracing::warn!(%error, "a canceled filter could not be taken back");
                        APPLIED
                    }
                },
                Ok(()) => APPLIED,
            };
            job_outcome.store(ended, Ordering::SeqCst);
            Ok(())
        });
        if queued {
            self.filter_job = Some(FilterJob {
                document: id,
                cancel,
                outcome,
                canceled_by_edit: false,
            });
            self.start_long_job(LongJob::Filtering, cx);
        }
    }

    pub(super) fn cancel_filter(&mut self, cx: &mut Context<Self>) {
        self.stop_filter_job(false, cx);
    }

    // Any edit while a filter is being applied cancels it first.
    pub(super) fn cancel_filter_for_edit(&mut self, cx: &mut Context<Self>) {
        self.stop_filter_job(true, cx);
    }

    fn stop_filter_job(&mut self, by_edit: bool, cx: &mut Context<Self>) {
        if let Some(job) = &mut self.filter_job {
            job.cancel.store(true, Ordering::SeqCst);
            job.canceled_by_edit |= by_edit;
            cx.notify();
        }
    }

    // Called when a batch of edits ends: the job's answer is read here, on the UI thread.
    pub(super) fn finish_filter_job(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(job) = self.filter_job.as_ref().filter(|job| job.document == id) else {
            return;
        };
        let ended = job.outcome.load(Ordering::SeqCst);
        if ended == PENDING && self.documents.get(id).is_some() {
            return;
        }
        let Some(job) = self.filter_job.take() else {
            return;
        };
        self.end_long_job(LongJob::Filtering, cx);
        match ended {
            FAILED => self.notify(Severity::Warning, t!("filter.apply_failed"), cx),
            SKIPPED | UNDONE if job.canceled_by_edit => {
                self.notify(Severity::Info, t!("filter.canceled_by_edit"), cx)
            }
            _ => {}
        }
    }
}
