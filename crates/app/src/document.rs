use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::Instant;
use zenkai_i18n::t;

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Engine, EngineError, Unsupported, Workbook, xlsx_bytes};
use zenkai_grid::{GridCell, ViewState};
use zenkai_types::{CellPos, Contents, Range, SheetId, SheetInfo, WorkbookId};

use crate::agent_review::Review;
use crate::entry::display_name;
use crate::find::FindBar;
use crate::spaces::SpaceId;

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub type SharedWorkbook = Arc<RwLock<Workbook>>;

pub type Edit = Box<dyn FnOnce(&mut Workbook) -> Result<(), EngineError> + Send>;

// Saves and autosaves write through the same temporary file, so only one runs at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileJob {
    Idle,
    Saving,
    Autosaving,
}

pub struct Document {
    pub id: WorkbookId,
    pub space: SpaceId,
    untitled: u32,
    workbook: SharedWorkbook,
    batch_running: bool,
    file_job: FileJob,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub sheet: SheetId,
    pub sheets: Vec<SheetInfo>,
    pub unsupported: Vec<Unsupported>,
    pub read_only: bool,
    // Where the file came from; a download keeps agents read-only, as Excel's Protected View.
    pub origin: FileOrigin,
    pending: Vec<Edit>,
    generation: u64,
    edit_count: u64,
    // The edit count the recovery copy on disk was written at.
    autosaved_at: Option<u64>,
    pub pending_sheet: Option<SheetId>,
    pub view: ViewState,
    pub find: Option<FindBar>,
    pub review: Review,
    pub last_used: Instant,
}

fn unsupported_label(unsupported: Unsupported) -> &'static str {
    match unsupported {
        Unsupported::Charts => t!("unsupported.charts"),
        Unsupported::Images => t!("unsupported.images"),
        Unsupported::PivotTables => t!("unsupported.pivot_tables"),
        Unsupported::Macros => t!("unsupported.macros"),
        Unsupported::Comments => t!("unsupported.comments"),
        Unsupported::Tables => t!("unsupported.tables"),
        Unsupported::Hyperlinks => t!("unsupported.hyperlinks"),
        Unsupported::DataValidation => t!("unsupported.data_validation"),
        Unsupported::ExternalLinks => t!("unsupported.external_links"),
        Unsupported::AutoFilter => t!("unsupported.auto_filter"),
        Unsupported::SheetProtection => t!("unsupported.sheet_protection"),
        Unsupported::Outline => t!("unsupported.outline"),
        Unsupported::PageBreaks => t!("unsupported.page_breaks"),
    }
}

impl Document {
    pub fn new(
        id: WorkbookId,
        space: SpaceId,
        untitled: u32,
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
    ) -> Document {
        let sheets = workbook.sheets();
        Document {
            id,
            space,
            untitled,
            workbook: Arc::new(RwLock::new(workbook)),
            batch_running: false,
            file_job: FileJob::Idle,
            path,
            dirty: false,
            sheet: SheetId(0),
            sheets,
            unsupported,
            read_only: false,
            origin: FileOrigin::Local,
            pending: Vec::new(),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            edit_count: 0,
            autosaved_at: None,
            pending_sheet: None,
            view: ViewState::default(),
            find: None,
            review: Review::default(),
            last_used: Instant::now(),
        }
    }

    pub fn name(&self) -> String {
        display_name(self.path.as_deref(), self.untitled)
    }

    pub fn untitled(&self) -> u32 {
        self.untitled
    }

    pub fn name_with_marker(&self) -> String {
        let marker = if self.dirty { "• " } else { "" };
        format!("{marker}{}", self.name())
    }

    pub fn title(&self) -> String {
        format!("{} - Zenkai", self.name_with_marker())
    }

    // The UI thread never waits: `None` means a recalculation holds the write lock.
    pub fn workbook(&self) -> Option<RwLockReadGuard<'_, Workbook>> {
        match self.workbook.try_read() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(poisoned)) => {
                tracing::error!("workbook lock poisoned by a failed job; reading it anyway");
                Some(poisoned.into_inner())
            }
        }
    }

    // Called when a batch of edits ends; a busy workbook is checked after the next one.
    pub fn revalidate_review(&mut self) {
        if self.review.is_empty() {
            return;
        }
        let Ok(workbook) = self.workbook.try_read() else {
            return;
        };
        self.review
            .retain_unchanged(|cell| workbook.input(cell.sheet, cell.pos));
    }

    pub fn queue(&mut self, edit: Edit) {
        self.edit_count += 1;
        self.pending.push(edit);
    }

    pub fn workbook_mut(&self) -> Option<RwLockWriteGuard<'_, Workbook>> {
        match self.workbook.try_write() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(poisoned)) => {
                tracing::error!("workbook lock poisoned by a failed job; writing it anyway");
                Some(poisoned.into_inner())
            }
        }
    }

    // Read-only jobs (find, save, autosave) share the workbook; they are refused while
    // edits are queued or running so they never describe a document about to change.
    pub fn begin_read(&self) -> Option<SharedWorkbook> {
        (!self.has_pending()).then(|| Arc::clone(&self.workbook))
    }

    pub fn file_job(&self) -> FileJob {
        self.file_job
    }

    pub fn begin_file_job(&mut self, job: FileJob) -> Option<SharedWorkbook> {
        if self.file_job != FileJob::Idle {
            return None;
        }
        let shared = self.begin_read()?;
        self.file_job = job;
        Some(shared)
    }

    pub fn end_file_job(&mut self, generation: u64) {
        if self.is_current(generation) {
            self.file_job = FileJob::Idle;
        }
    }

    pub fn take_batch(&mut self) -> Option<(SharedWorkbook, Vec<Edit>)> {
        if self.batch_running || self.pending.is_empty() {
            return None;
        }
        self.batch_running = true;
        Some((
            Arc::clone(&self.workbook),
            std::mem::take(&mut self.pending),
        ))
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn edit_count(&self) -> u64 {
        self.edit_count
    }

    pub fn recovery_is_current(&self) -> bool {
        self.autosaved_at == Some(self.edit_count)
    }

    pub fn set_autosaved_at(&mut self, edits: Option<u64>) {
        self.autosaved_at = edits;
    }

    pub fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.dirty && self.edit_count == 0
    }

    pub fn has_pending(&self) -> bool {
        self.batch_running || !self.pending.is_empty()
    }

    pub fn unsupported_labels(&self) -> String {
        let labels: Vec<&str> = self
            .unsupported
            .iter()
            .map(|u| unsupported_label(*u))
            .collect();
        labels.join(", ")
    }

    pub fn is_macro_enabled(&self) -> bool {
        self.path
            .as_ref()
            .and_then(|p| p.extension())
            .is_some_and(|e| e.eq_ignore_ascii_case("xlsm"))
    }

    // A job finishing after a document was opened or created meanwhile has an old
    // generation and must not touch the new one.
    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    pub fn finish_batch(&mut self, generation: u64) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.batch_running = false;
        if let Some((sheets, dropped)) = self
            .workbook()
            .map(|workbook| (workbook.sheets(), workbook.dropped_on_save()))
        {
            self.sheets = sheets;
            for lost in dropped {
                if !self.unsupported.contains(&lost) {
                    self.unsupported.push(lost);
                }
            }
            self.unsupported.sort();
        }
        let last = u32::try_from(self.sheets.len().saturating_sub(1)).unwrap_or(0);
        if self.sheet.0 > last {
            self.sheet = SheetId(last);
        }
        true
    }

    // With `formulas`, cells holding a formula show it instead of its value (Ctrl+`).
    pub fn cells(&self, ranges: &[Range], formulas: bool) -> Option<HashMap<CellPos, GridCell>> {
        let workbook = self.workbook()?;
        let end = workbook.used_end(self.sheet);
        let limit = CellPos::new(end.row.offset(1), end.col.offset(1));
        let clipped: Vec<Range> = ranges
            .iter()
            .filter_map(|range| range.clip_to(limit))
            .collect();
        let cells = clipped
            .iter()
            .flat_map(Range::positions)
            .filter_map(|pos| {
                let view = workbook.cell(self.sheet, pos);
                let blank = view.text.is_empty()
                    && view.style.fill.is_none()
                    && !view.style.border_top
                    && !view.style.border_left
                    && !view.style.border_bottom
                    && !view.style.border_right;
                (!blank).then(|| {
                    let formula = formulas
                        .then(|| workbook.input(self.sheet, pos))
                        .filter(|input| input.starts_with('='));
                    let cell = match formula {
                        Some(formula) => GridCell {
                            text: formula.into(),
                            kind: zenkai_types::ValueKind::Text,
                            style: view.style,
                        },
                        None => GridCell {
                            text: view.text.into(),
                            kind: view.kind,
                            style: view.style,
                        },
                    };
                    (pos, cell)
                })
            })
            .collect();
        Some(cells)
    }
}

pub fn contents_of(
    workbook: &Workbook,
    sheet: SheetId,
) -> Option<impl Fn(CellPos) -> Contents + Sync + '_> {
    workbook
        .contents(sheet)
        .inspect_err(|error| tracing::error!(%error, "could not read the sheet contents"))
        .ok()
}

pub fn read_shared(shared: &RwLock<Workbook>) -> RwLockReadGuard<'_, Workbook> {
    shared.read().unwrap_or_else(PoisonError::into_inner)
}

// The read lock is held only while the bytes are made: writing and verifying them takes
// far longer, and an edit waiting for the write lock blocks every reader meanwhile.
pub fn write_snapshot(
    shared: &RwLock<Workbook>,
    job: FileJob,
    write: impl FnOnce(&[u8]) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let started = Instant::now();
    let bytes = {
        let workbook = read_shared(shared);
        xlsx_bytes(&workbook)
    };
    let locked_ms = started.elapsed().as_millis();
    let written = bytes.and_then(|bytes| write(&bytes));
    let total_ms = started.elapsed().as_millis();
    match &written {
        Ok(()) => tracing::info!(?job, locked_ms, total_ms, "workbook written"),
        Err(error) => {
            tracing::warn!(?job, %error, locked_ms, total_ms, "writing the workbook failed")
        }
    }
    written
}

pub fn run_batch(shared: &RwLock<Workbook>, edits: Vec<Edit>) -> Vec<EngineError> {
    let waited = Instant::now();
    let mut guard = shared.write().unwrap_or_else(PoisonError::into_inner);
    let started = Instant::now();
    let workbook: &mut Workbook = &mut guard;
    let count = edits.len();
    let run = zenkai_engine::run_with_engine_stack(|| {
        let errors = edits
            .into_iter()
            .filter_map(|edit| edit(workbook).err())
            .collect::<Vec<_>>();
        workbook.warm_used_areas();
        Ok(errors)
    });
    let errors = run.unwrap_or_else(|error| vec![error]);
    let (waited_ms, ran_ms) = (
        started.duration_since(waited).as_millis(),
        started.elapsed().as_millis(),
    );
    match errors.first() {
        None => tracing::info!(count, waited_ms, ran_ms, "edit batch done"),
        Some(error) => {
            tracing::warn!(count, waited_ms, ran_ms, failed = errors.len(), %error, "edit batch had errors")
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_view_shows_formulas_and_keeps_values() {
        use zenkai_engine::Engine;
        let mut workbook = Workbook::new_empty().unwrap();
        let rows = vec![vec!["2".to_string(), "=A1*3".to_string()]];
        workbook
            .set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let document = Document::new(WorkbookId(0), SpaceId(0), 1, workbook, None, Vec::new());
        let range = Range::parse_a1("A1:B1").unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        assert_eq!(
            document.cells(&[range], false).unwrap()[&b1].text.as_ref(),
            "6"
        );
        let formulas = document.cells(&[range], true).unwrap();
        assert_eq!(formulas[&b1].text.as_ref(), "=A1*3");
        assert_eq!(formulas[&CellPos::default()].text.as_ref(), "2");
    }

    fn document() -> Document {
        Document::new(
            WorkbookId(0),
            SpaceId(0),
            1,
            Workbook::new_empty().unwrap(),
            None,
            Vec::new(),
        )
    }

    fn noop() -> Edit {
        Box::new(|_| Ok(()))
    }

    #[test]
    fn stale_job_is_not_current_in_a_new_document() {
        let old = document();
        let generation = old.generation();
        let mut current = document();
        assert!(!current.is_current(generation));
        assert!(!current.finish_batch(generation));
    }

    #[test]
    fn readers_share_the_workbook_with_the_ui() {
        let document = document();
        let job = document.begin_read().unwrap();
        let job_guard = read_shared(&job);
        assert!(document.workbook().is_some());
        assert!(document.begin_read().is_some());
        drop(job_guard);
    }

    #[test]
    fn ui_does_not_wait_for_a_batch_holding_the_write_lock() {
        let mut document = document();
        document.queue(noop());
        let (shared, edits) = document.take_batch().unwrap();
        let guard = shared.write().unwrap();
        assert!(document.workbook().is_none());
        assert!(document.workbook_mut().is_none());
        drop(guard);
        assert!(run_batch(&shared, edits).is_empty());
        assert!(document.workbook().is_some());
    }

    #[test]
    fn reads_and_second_batches_wait_for_the_running_batch() {
        let mut document = document();
        document.queue(noop());
        let generation = document.generation();
        assert!(document.take_batch().is_some());
        document.queue(noop());
        assert!(document.take_batch().is_none());
        assert!(document.begin_read().is_none());
        assert!(document.finish_batch(generation));
        assert!(document.take_batch().is_some());
    }

    #[test]
    fn a_save_or_autosave_waits_for_the_one_running() {
        let mut document = document();
        let generation = document.generation();
        assert!(document.begin_file_job(FileJob::Saving).is_some());
        assert_eq!(document.file_job(), FileJob::Saving);
        assert!(document.begin_file_job(FileJob::Saving).is_none());
        assert!(document.begin_file_job(FileJob::Autosaving).is_none());
        assert!(document.begin_read().is_some());
        document.end_file_job(generation);
        assert!(document.begin_file_job(FileJob::Autosaving).is_some());
        assert!(document.begin_file_job(FileJob::Saving).is_none());
    }

    #[test]
    fn a_stale_save_does_not_release_the_new_documents_job() {
        let old = document();
        let stale = old.generation();
        let mut current = document();
        assert!(current.begin_file_job(FileJob::Saving).is_some());
        current.end_file_job(stale);
        assert_eq!(current.file_job(), FileJob::Saving);
    }

    #[test]
    fn a_snapshot_is_written_without_the_lock() {
        let document = document();
        let shared = document.begin_read().unwrap();
        let mut written = Vec::new();
        write_snapshot(&shared, FileJob::Autosaving, |bytes| {
            assert!(shared.try_write().is_ok(), "the read lock is still held");
            written = bytes.to_vec();
            Ok(())
        })
        .unwrap();
        assert!(written.starts_with(b"PK"));
    }

    #[test]
    fn a_failed_snapshot_write_is_reported() {
        let document = document();
        let shared = document.begin_read().unwrap();
        let failed = write_snapshot(&shared, FileJob::Saving, |_| {
            Err(EngineError::VerifyFailed("disk full".to_string()))
        });
        assert!(matches!(failed, Err(EngineError::VerifyFailed(_))));
    }

    #[test]
    fn the_recovery_copy_is_current_until_the_next_edit() {
        let mut document = document();
        assert!(!document.recovery_is_current());
        document.set_autosaved_at(Some(document.edit_count()));
        assert!(document.recovery_is_current());
        document.queue(noop());
        assert!(!document.recovery_is_current());
        document.set_autosaved_at(Some(document.edit_count()));
        document.set_autosaved_at(None);
        assert!(!document.recovery_is_current());
    }

    #[test]
    fn edits_run_in_queue_order() {
        use std::sync::Mutex;
        let mut document = document();
        let order = Arc::new(Mutex::new(Vec::new()));
        for n in 0..3 {
            let order = Arc::clone(&order);
            document.queue(Box::new(move |_| {
                order.lock().unwrap().push(n);
                Ok(())
            }));
        }
        let (shared, edits) = document.take_batch().unwrap();
        assert!(run_batch(&shared, edits).is_empty());
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn poisoned_lock_stays_readable() {
        let document = document();
        let shared = document.begin_read().unwrap();
        let poisoner = Arc::clone(&shared);
        let joined = std::thread::spawn(move || {
            let _guard = poisoner.write().unwrap();
            panic!("job failed");
        })
        .join();
        assert!(joined.is_err());
        assert!(document.workbook().is_some());
        assert!(document.workbook_mut().is_some());
    }
}
