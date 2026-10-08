use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

use zenkai_engine::{Engine, EngineError, Unsupported, Workbook};
use zenkai_grid::GridCell;
use zenkai_types::{CellPos, Contents, Range, SheetId, SheetInfo};

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
    workbook: SharedWorkbook,
    batch_running: bool,
    file_job: FileJob,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub sheet: SheetId,
    pub sheets: Vec<SheetInfo>,
    pub unsupported: Vec<Unsupported>,
    pub read_only: bool,
    pending: Vec<Edit>,
    generation: u64,
}

impl Document {
    pub fn new(
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
    ) -> Document {
        let sheets = workbook.sheets();
        Document {
            workbook: Arc::new(RwLock::new(workbook)),
            batch_running: false,
            file_job: FileJob::Idle,
            path,
            dirty: false,
            sheet: SheetId(0),
            sheets,
            unsupported,
            read_only: false,
            pending: Vec::new(),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn name_with_marker(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Book1".to_string(), |n| n.to_string_lossy().into_owned());
        let marker = if self.dirty { "• " } else { "" };
        format!("{marker}{name}")
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

    pub fn queue(&mut self, edit: Edit) {
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

    pub fn has_pending(&self) -> bool {
        self.batch_running || !self.pending.is_empty()
    }

    pub fn unsupported_labels(&self) -> String {
        let labels: Vec<&str> = self.unsupported.iter().map(|u| u.label()).collect();
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
        if let Some(sheets) = self.workbook().map(|workbook| workbook.sheets()) {
            self.sheets = sheets;
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

pub fn run_batch(shared: &RwLock<Workbook>, edits: Vec<Edit>) -> Vec<EngineError> {
    let mut guard = shared.write().unwrap_or_else(PoisonError::into_inner);
    let workbook: &mut Workbook = &mut guard;
    let run = zenkai_engine::run_with_engine_stack(|| {
        let errors = edits
            .into_iter()
            .filter_map(|edit| edit(workbook).err())
            .collect::<Vec<_>>();
        workbook.warm_used_areas();
        Ok(errors)
    });
    run.unwrap_or_else(|error| vec![error])
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
        let document = Document::new(workbook, None, Vec::new());
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
        Document::new(Workbook::new_empty().unwrap(), None, Vec::new())
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
