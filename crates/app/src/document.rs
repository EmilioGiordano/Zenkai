use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::{Duration, Instant};
use zenkai_i18n::t;

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Engine, EngineError, SaveSnapshot, SaveTarget, Unsupported, Workbook};
use zenkai_grid::{GridCell, ViewState};
use zenkai_types::{CellPos, Contents, Range, SheetId, SheetInfo, WorkbookId};

use crate::agent_review::Review;
use crate::entry::display_name;
use crate::files;
use crate::find::FindBar;
use crate::pending_save::{AfterJob, JobEnd, PendingSave};
use crate::spaces::SpaceId;
use crate::stale_view::{StaleRequest, StaleSlot};

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
    stale: Arc<StaleSlot>,
    batch_running: bool,
    sheets_stale: bool,
    file_job: FileJob,
    pending_save: PendingSave,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub sheet: SheetId,
    pub sheets: Vec<SheetInfo>,
    // What the document's file holds that this workbook may not, for the open notice and a
    // restored session.
    pub unsupported: Vec<Unsupported>,
    // Restored from a recovery copy, which may lack content the file holds: no snapshot of
    // the copy can tell what saving over the file drops.
    pub restored: bool,
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
    last_edited: Option<Instant>,
    autosave_attempted: Instant,
    // Since when the recovery copy on disk has missed edits; `None` while it holds them all.
    recovery_stale_since: Option<Instant>,
    // Whether a recovery copy of this session exists on disk for the session file to point at.
    recovery_on_disk: bool,
    // Set by an edit; the running autosave drops its write and leaves the old copy in place.
    autosave_cancel: Arc<AtomicBool>,
}

pub fn labels(kinds: &[Unsupported]) -> String {
    let labels: Vec<&str> = kinds.iter().map(|kind| unsupported_label(*kind)).collect();
    labels.join(", ")
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
            stale: Arc::default(),
            batch_running: false,
            sheets_stale: false,
            file_job: FileJob::Idle,
            pending_save: PendingSave::None,
            path,
            dirty: false,
            sheet: SheetId(0),
            sheets,
            unsupported,
            restored: false,
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
            last_edited: None,
            autosave_attempted: Instant::now(),
            recovery_stale_since: None,
            recovery_on_disk: false,
            autosave_cancel: Arc::new(AtomicBool::new(false)),
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

    pub fn stale_slot(&self) -> Arc<StaleSlot> {
        Arc::clone(&self.stale)
    }

    // While a batch holds the write lock: the cells of `ranges` as they were before it, and
    // the part of `ranges` that answer covers. Never a substitute for a live read.
    pub fn stale_cells(
        &self,
        ranges: &[Range],
        formulas: bool,
    ) -> Option<(HashMap<CellPos, GridCell>, Vec<Range>)> {
        self.stale.current()?.cells_in(self.sheet, formulas, ranges)
    }

    pub fn queue(&mut self, edit: Edit) {
        self.edit_count += 1;
        self.last_edited = Some(Instant::now());
        self.autosave_cancel.store(true, Ordering::Relaxed);
        // Clean work (saved, or a discarded copy) owes no recovery copy, so a stale instant
        // left from before it must not make the first new edit overdue. Callers mark the
        // document dirty after queueing.
        if !self.needs_recovery() {
            self.recovery_stale_since = None;
        }
        self.recovery_stale_since.get_or_insert_with(Instant::now);
        self.pending.push(edit);
    }

    pub fn shared_workbook(&self) -> SharedWorkbook {
        Arc::clone(&self.workbook)
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

    pub fn request_save(&mut self) {
        self.pending_save.request();
    }

    // A job of an older generation belongs to a replaced document and settles nothing.
    pub fn settle_save(&mut self, generation: u64, end: JobEnd) -> AfterJob {
        if !self.is_current(generation) {
            return AfterJob::Nothing;
        }
        self.pending_save.settle(end)
    }

    pub fn is_following_up(&self) -> bool {
        self.pending_save.is_following()
    }

    pub fn begin_follow_up(&mut self) -> bool {
        self.pending_save.begin_follow_up()
    }

    pub fn clear_pending_save(&mut self) {
        self.pending_save.clear();
    }

    pub fn begin_file_job(&mut self, job: FileJob) -> Option<SharedWorkbook> {
        if self.file_job != FileJob::Idle {
            return None;
        }
        let shared = self.begin_read()?;
        self.file_job = job;
        if job == FileJob::Autosaving {
            self.autosave_cancel = Arc::new(AtomicBool::new(false));
        }
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

    // `None` for work restored from a previous run, whose edits this run never saw.
    pub fn since_last_edit(&self) -> Option<Duration> {
        self.last_edited.map(|edited| edited.elapsed())
    }

    pub fn since_last_autosave_attempt(&self) -> Duration {
        self.autosave_attempted.elapsed()
    }

    pub fn mark_autosave_attempt(&mut self) {
        self.autosave_attempted = Instant::now();
    }

    pub fn autosave_cancel(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.autosave_cancel)
    }

    pub fn recovery_is_current(&self) -> bool {
        self.autosaved_at == Some(self.edit_count)
    }

    pub fn set_autosaved_at(&mut self, edits: Option<u64>) {
        self.autosaved_at = edits;
        self.recovery_on_disk = edits.is_some();
    }

    pub fn recovery_written(&mut self, edits: u64) {
        self.set_autosaved_at(Some(edits));
        self.recovery_stale_since = (edits != self.edit_count).then(Instant::now);
    }

    pub fn recovery_stale_for(&self) -> Option<Duration> {
        self.recovery_stale_since.map(|since| since.elapsed())
    }

    pub fn has_recovery_copy(&self) -> bool {
        self.recovery_on_disk
    }

    pub fn mark_recovery_adopted(&mut self) {
        self.recovery_on_disk = true;
    }

    pub fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.dirty && self.edit_count == 0
    }

    pub fn has_pending(&self) -> bool {
        self.batch_running || !self.pending.is_empty()
    }

    // A copy that lacks some of the file's content keeps it listed, so a restored session
    // still warns before saving over the file. Returns whether the list grew.
    pub fn recovery_lacks(&mut self, lacks: &[Unsupported]) -> bool {
        let before = self.unsupported.len();
        for kind in lacks {
            if !self.unsupported.contains(kind) {
                self.unsupported.push(*kind);
            }
        }
        self.unsupported.sort();
        self.unsupported.len() != before
    }

    pub fn saved_over_file(&mut self, kept: Vec<Unsupported>) {
        self.unsupported = kept;
        self.restored = false;
    }

    // Before a save is tried: the engine answers for every other case once it has run.
    pub fn may_lose_unseen_content(&self) -> bool {
        self.restored && !self.unsupported.is_empty()
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
        self.stale.clear();
        self.sync_sheets();
        true
    }

    pub fn sheets_stale(&self) -> bool {
        self.sheets_stale
    }

    // A busy workbook leaves the sheet list as it was and marks it stale for a retry.
    pub fn sync_sheets(&mut self) {
        let Some(sheets) = self.workbook().map(|workbook| workbook.sheets()) else {
            self.sheets_stale = true;
            return;
        };
        self.sheets_stale = false;
        self.sheets = sheets;
        let last = u32::try_from(self.sheets.len().saturating_sub(1)).unwrap_or(0);
        if self.sheet.0 > last {
            self.sheet = SheetId(last);
        }
    }

    // With `formulas`, cells holding a formula show it instead of its value (Ctrl+`).
    pub fn cells(&self, ranges: &[Range], formulas: bool) -> Option<HashMap<CellPos, GridCell>> {
        let workbook = self.workbook()?;
        Some(read_cells(&workbook, self.sheet, ranges, formulas))
    }
}

pub fn read_cells(
    workbook: &Workbook,
    sheet: SheetId,
    ranges: &[Range],
    formulas: bool,
) -> HashMap<CellPos, GridCell> {
    let end = workbook.used_end(sheet);
    let limit = CellPos::new(end.row.offset(1), end.col.offset(1));
    let clipped: Vec<Range> = ranges
        .iter()
        .filter_map(|range| range.clip_to(limit))
        .collect();
    clipped
        .iter()
        .flat_map(Range::positions)
        .filter_map(|pos| {
            let view = workbook.cell(sheet, pos);
            let blank = view.text.is_empty()
                && view.style.fill.is_none()
                && !view.style.border_top
                && !view.style.border_left
                && !view.style.border_bottom
                && !view.style.border_right;
            (!blank).then(|| {
                let formula = formulas
                    .then(|| workbook.input(sheet, pos))
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
        .collect()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangedFile {
    // A save stops when the file it would replace cannot be copied from, so the user decides.
    Ask,
    Replace,
}

// A base other than the target is only read from, and a target that no longer exists holds
// nothing to lose, so writing the target whole is safe. The target itself as a changed or
// unusable base may hold another program's work: like Excel, a save asks before replacing it.
fn retries_whole(error: &EngineError, target: &Path, changed: ChangedFile) -> bool {
    match error {
        EngineError::ChangedOnDisk(base) | EngineError::BaseUnusable { path: base, .. } => {
            changed == ChangedFile::Replace
                || matches!(target.try_exists(), Ok(false))
                || !files::same_file(base, target, files::Unresolved::AssumeSame)
        }
        _ => false,
    }
}

// The read lock is held only while the snapshot is taken, which fingerprints every sheet and
// copies the ones the save writes: writing and verifying it takes far longer, and an edit
// waiting for the write lock blocks every reader meanwhile.
fn with_snapshot<T: std::fmt::Debug>(
    shared: &RwLock<Workbook>,
    job: FileJob,
    target: &Path,
    kind: SaveTarget,
    changed: ChangedFile,
    attempt: impl Fn(SaveSnapshot) -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    let started = Instant::now();
    let snapshot = read_shared(shared).save_snapshot(target, kind);
    let locked_ms = started.elapsed().as_millis();
    let first = snapshot.and_then(&attempt);
    let retry = first
        .as_ref()
        .err()
        .is_some_and(|error| retries_whole(error, target, changed));
    // The engine forgot the file it copies from, or the one sheet it could not splice, so the
    // second snapshot writes that whole.
    let finished = match (retry, first) {
        (true, Err(error)) => {
            tracing::warn!(?job, %error, "the file copied from changed, writing it whole");
            read_shared(shared)
                .save_snapshot(target, kind)
                .and_then(&attempt)
        }
        (_, first) => first,
    };
    let total_ms = started.elapsed().as_millis();
    match &finished {
        Ok(outcome) => tracing::info!(?job, ?outcome, locked_ms, total_ms, "workbook save ended"),
        Err(error) => {
            tracing::warn!(?job, %error, locked_ms, total_ms, "writing the workbook failed")
        }
    }
    finished
}

// Returns what the written copy lacks of the document's own file.
pub fn write_snapshot(
    shared: &RwLock<Workbook>,
    job: FileJob,
    target: &Path,
    kind: SaveTarget,
    write: impl Fn(SaveSnapshot) -> Result<(), EngineError>,
) -> Result<Vec<Unsupported>, EngineError> {
    with_snapshot(
        shared,
        job,
        target,
        kind,
        ChangedFile::Replace,
        |snapshot| {
            let lacks = snapshot.dropped().to_vec();
            write(snapshot).map(|()| lacks)
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Written { lacks: Vec<Unsupported> },
    Abandoned,
}

// The snapshot is taken regardless, but once the edit that cancelled it is known the write
// never starts: the writer only renames a finished temp file over the target, so the copy
// already on disk stays whole and the next idle retries.
pub fn write_snapshot_unless(
    shared: &RwLock<Workbook>,
    job: FileJob,
    target: &Path,
    kind: SaveTarget,
    cancelled: &AtomicBool,
    write: impl Fn(SaveSnapshot) -> Result<(), EngineError>,
) -> Result<SnapshotOutcome, EngineError> {
    with_snapshot(
        shared,
        job,
        target,
        kind,
        ChangedFile::Replace,
        |snapshot| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(SnapshotOutcome::Abandoned);
            }
            let lacks = snapshot.dropped().to_vec();
            write(snapshot).map(|()| SnapshotOutcome::Written { lacks })
        },
    )
}

fn allows(accepted: &[Unsupported], dropped: &[Unsupported]) -> bool {
    dropped.iter().all(|kind| accepted.contains(kind))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentSave {
    Written { kept: Vec<Unsupported> },
    Refused { dropped: Vec<Unsupported> },
    // Nothing was written: the file to replace changed on disk or cannot be copied from.
    BaseChanged,
}

// Checked on every attempt: a retry writes the whole file and may drop more than the first.
pub fn save_document_file(
    shared: &RwLock<Workbook>,
    target: &Path,
    accepted: &[Unsupported],
    changed: ChangedFile,
    write: impl Fn(SaveSnapshot) -> Result<(), EngineError>,
) -> Result<DocumentSave, EngineError> {
    let saved = with_snapshot(
        shared,
        FileJob::Saving,
        target,
        SaveTarget::Document,
        changed,
        |snapshot| {
            if !allows(accepted, snapshot.dropped()) {
                return Ok(DocumentSave::Refused {
                    dropped: snapshot.dropped().to_vec(),
                });
            }
            let kept = snapshot.kept().to_vec();
            write(snapshot).map(|()| DocumentSave::Written { kept })
        },
    );
    match saved {
        Err(EngineError::BaseUnusable { .. } | EngineError::ChangedOnDisk(_))
            if changed == ChangedFile::Ask =>
        {
            Ok(DocumentSave::BaseChanged)
        }
        other => other,
    }
}

// For a file Save As gave a name of its own, which nothing is lost from. Returns what the
// written file holds that Zenkai does not show.
pub fn write_document_copy(
    shared: &RwLock<Workbook>,
    target: &Path,
    write: impl Fn(SaveSnapshot) -> Result<(), EngineError>,
) -> Result<Vec<Unsupported>, EngineError> {
    with_snapshot(
        shared,
        FileJob::Saving,
        target,
        SaveTarget::Document,
        ChangedFile::Replace,
        |snapshot| {
            let kept = snapshot.kept().to_vec();
            write(snapshot).map(|()| kept)
        },
    )
}

// The engine recalculates in place and cannot yield, so the write lock is held for the whole
// batch. The grid's reads meanwhile come from a copy of its surroundings taken just before.
pub fn run_batch_serving(
    shared: &RwLock<Workbook>,
    edits: Vec<Edit>,
    stale: Option<StaleRequest>,
) -> Vec<EngineError> {
    if let Some(request) = &stale
        && request.slot.last_batch_was_slow()
    {
        request.publish_from(&read_shared(shared));
    }
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
    if let Some(request) = &stale {
        request.slot.record_hold(started.elapsed());
    }
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
    fn a_stale_job_settles_nothing_in_a_new_document() {
        let old = document();
        let generation = old.generation();
        let mut current = document();
        current.request_save();
        assert_eq!(
            current.settle_save(generation, JobEnd::Autosaved),
            AfterJob::Nothing
        );
        let own = current.generation();
        assert_eq!(
            current.settle_save(own, JobEnd::Autosaved),
            AfterJob::SaveAgain
        );
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
    fn a_busy_workbook_marks_the_sheet_list_stale_until_it_is_synced() {
        let mut document = document();
        document.queue(noop());
        let generation = document.generation();
        let (shared, _) = document.take_batch().unwrap();
        let guard = shared.write().unwrap();
        assert!(document.finish_batch(generation));
        assert!(document.sheets_stale());
        drop(guard);
        document.sync_sheets();
        assert!(!document.sheets_stale());
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
        assert!(run_batch_serving(&shared, edits, None).is_empty());
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
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        write_snapshot(
            &shared,
            FileJob::Autosaving,
            &target,
            SaveTarget::Copy,
            |snapshot| {
                assert!(shared.try_write().is_ok(), "the read lock is still held");
                zenkai_engine::save_xlsx(snapshot)
            },
        )
        .unwrap();
        assert!(std::fs::read(&target).unwrap().starts_with(b"PK"));
    }

    #[test]
    fn an_edit_cancels_the_running_autosave_and_a_new_one_starts_clean() {
        let mut document = document();
        let generation = document.generation();
        assert!(document.begin_file_job(FileJob::Autosaving).is_some());
        let cancel = document.autosave_cancel();
        assert!(!cancel.load(Ordering::Relaxed));
        document.queue(noop());
        assert!(cancel.load(Ordering::Relaxed));
        document.end_file_job(generation);
        let (_, edits) = document.take_batch().unwrap();
        assert_eq!(edits.len(), 1);
        let shared = Arc::clone(&document.workbook);
        assert!(run_batch_serving(&shared, edits, None).is_empty());
        document.finish_batch(generation);
        assert!(document.begin_file_job(FileJob::Autosaving).is_some());
        assert!(!document.autosave_cancel().load(Ordering::Relaxed));
    }

    #[test]
    fn the_recovery_copy_is_stale_from_the_first_edit_it_misses() {
        let mut document = document();
        assert!(document.recovery_stale_for().is_none());
        document.queue(noop());
        assert!(document.recovery_stale_for().is_some());
        document.recovery_written(document.edit_count());
        assert!(document.recovery_stale_for().is_none());
        assert!(document.has_recovery_copy());
        let at_start = document.edit_count();
        document.queue(noop());
        document.recovery_written(at_start);
        assert!(document.recovery_stale_for().is_some());
    }

    #[test]
    fn the_first_edit_after_a_save_restarts_the_staleness_clock() {
        let mut document = document();
        document.path = Some(PathBuf::from("book.xlsx"));
        document.queue(noop());
        document.dirty = true;
        document.recovery_stale_since = Instant::now().checked_sub(Duration::from_secs(3600));
        document.dirty = false;
        document.queue(noop());
        document.dirty = true;
        let stale = document.recovery_stale_for().unwrap();
        let every = Duration::from_secs(60);
        let due = crate::autosave::due(Some(Duration::ZERO), every, Some(stale), every);
        assert_ne!(due, crate::autosave::Due::Overdue);
    }

    #[test]
    fn a_workbook_has_no_recovery_copy_until_one_is_written_or_adopted() {
        let mut document = document();
        assert!(!document.has_recovery_copy());
        document.mark_recovery_adopted();
        assert!(document.has_recovery_copy());
        document.set_autosaved_at(None);
        assert!(!document.has_recovery_copy());
    }

    #[test]
    fn a_cancelled_snapshot_skips_the_write() {
        let document = document();
        let shared = document.begin_read().unwrap();
        let cancelled = AtomicBool::new(true);
        let outcome = write_snapshot_unless(
            &shared,
            FileJob::Autosaving,
            Path::new("unused.xlsx"),
            SaveTarget::Copy,
            &cancelled,
            |_| panic!("the old recovery copy must stay untouched"),
        );
        assert_eq!(outcome.unwrap(), SnapshotOutcome::Abandoned);
    }

    #[test]
    fn an_uncancelled_snapshot_is_written() {
        let document = document();
        let shared = document.begin_read().unwrap();
        let cancelled = AtomicBool::new(false);
        let outcome = write_snapshot_unless(
            &shared,
            FileJob::Autosaving,
            Path::new("unused.xlsx"),
            SaveTarget::Copy,
            &cancelled,
            |_| Ok(()),
        );
        assert_eq!(
            outcome.unwrap(),
            SnapshotOutcome::Written { lacks: Vec::new() }
        );
    }

    #[test]
    fn a_failed_snapshot_write_is_reported() {
        let document = document();
        let shared = document.begin_read().unwrap();
        let target = Path::new("unused.xlsx");
        let failed = write_snapshot(&shared, FileJob::Saving, target, SaveTarget::Copy, |_| {
            Err(EngineError::VerifyFailed("disk full".to_string()))
        });
        assert!(matches!(failed, Err(EngineError::VerifyFailed(_))));
    }

    fn saved_then_changed_by_another_program(target: &Path) -> Workbook {
        let a1 = CellPos::default();
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), a1, "first").unwrap();
        zenkai_engine::save_xlsx(book.save_snapshot(target, SaveTarget::Document).unwrap())
            .unwrap();
        let mut other = Workbook::new_empty().unwrap();
        other.set_input(SheetId(0), a1, "someone else's").unwrap();
        zenkai_engine::save_xlsx(other.save_snapshot(target, SaveTarget::Copy).unwrap()).unwrap();
        book.set_input(SheetId(0), a1, "mine").unwrap();
        book
    }

    fn saved_text(path: &Path) -> String {
        let reopened = zenkai_engine::open_xlsx(path).unwrap().workbook;
        reopened.cell(SheetId(0), CellPos::default()).text
    }

    fn write_counting(
        shared: &RwLock<Workbook>,
        target: &Path,
        kind: SaveTarget,
    ) -> (Result<(), EngineError>, u32) {
        let attempts = std::cell::Cell::new(0);
        let written = write_snapshot(shared, FileJob::Saving, target, kind, |snapshot| {
            attempts.set(attempts.get() + 1);
            zenkai_engine::save_xlsx(snapshot)
        });
        (written.map(|_| ()), attempts.get())
    }

    #[test]
    fn a_save_over_a_file_another_program_changed_asks_and_writes_on_replace() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        let shared = RwLock::new(saved_then_changed_by_another_program(&target));
        let asked = save_document_file(
            &shared,
            &target,
            &[],
            ChangedFile::Ask,
            zenkai_engine::save_xlsx,
        );
        assert_eq!(asked.unwrap(), DocumentSave::BaseChanged);
        assert_eq!(saved_text(&target), "someone else's");
        let replaced = save_document_file(
            &shared,
            &target,
            &[],
            ChangedFile::Replace,
            zenkai_engine::save_xlsx,
        );
        assert!(matches!(replaced, Ok(DocumentSave::Written { .. })));
        assert_eq!(saved_text(&target), "mine");
    }

    #[test]
    fn a_recovery_copy_of_a_changed_file_is_written_whole_in_the_same_save() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("book.xlsx");
        let copy = dir.path().join("recovery.xlsx");
        let shared = RwLock::new(saved_then_changed_by_another_program(&original));
        let (written, attempts) = write_counting(&shared, &copy, SaveTarget::Copy);
        written.unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(saved_text(&copy), "mine");
        assert_eq!(saved_text(&original), "someone else's");
    }

    #[test]
    fn a_save_goes_ahead_only_when_it_drops_nothing_unaccepted() {
        let filter = [Unsupported::AutoFilter];
        let both = [Unsupported::Charts, Unsupported::AutoFilter];
        assert!(allows(&[], &[]));
        assert!(!allows(&[], &filter));
        assert!(allows(&filter, &filter));
        assert!(!allows(&filter, &both));
        assert!(allows(&both, &filter));
    }

    #[test]
    fn a_save_that_drops_nothing_writes_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        let shared = RwLock::new(Workbook::new_empty().unwrap());
        let saved = save_document_file(
            &shared,
            &target,
            &[],
            ChangedFile::Ask,
            zenkai_engine::save_xlsx,
        );
        assert_eq!(saved.unwrap(), DocumentSave::Written { kept: Vec::new() });
        assert!(std::fs::read(&target).unwrap().starts_with(b"PK"));
    }

    #[test]
    fn a_restored_document_warns_until_it_is_saved_over_its_file() {
        let mut document = document();
        document.restored = true;
        assert!(!document.may_lose_unseen_content());
        assert!(document.recovery_lacks(&[Unsupported::Outline, Unsupported::Charts]));
        assert!(!document.recovery_lacks(&[Unsupported::Charts]));
        assert_eq!(
            document.unsupported,
            [Unsupported::Charts, Unsupported::Outline]
        );
        assert!(document.may_lose_unseen_content());
        document.saved_over_file(vec![Unsupported::Charts]);
        assert_eq!(document.unsupported, [Unsupported::Charts]);
        assert!(!document.may_lose_unseen_content());
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
        assert!(run_batch_serving(&shared, edits, None).is_empty());
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

    fn unusable(path: &Path) -> EngineError {
        EngineError::BaseUnusable {
            path: path.to_path_buf(),
            reason: "test".to_string(),
        }
    }

    #[test]
    fn an_unusable_base_is_replaced_without_asking_only_for_another_target() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("book.xlsx");
        let other = dir.path().join("recovery.xlsx");
        std::fs::write(&base, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        assert!(!retries_whole(&unusable(&base), &base, ChangedFile::Ask));
        assert!(retries_whole(&unusable(&base), &base, ChangedFile::Replace));
        assert!(retries_whole(&unusable(&base), &other, ChangedFile::Ask));
    }

    #[test]
    fn a_path_that_cannot_be_resolved_is_assumed_to_be_the_same_file_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.xlsx");
        let spelled = dir.path().join("sub").join("..").join("gone.xlsx");
        assert!(files::same_file(
            &missing,
            &spelled,
            files::Unresolved::AssumeSame
        ));
        assert!(!files::same_file(
            &missing,
            &spelled,
            files::Unresolved::ComparePaths
        ));
    }

    #[test]
    fn a_save_over_a_deleted_file_is_written_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        let shared = open_with_damaged_file(&target);
        std::fs::remove_file(&target).unwrap();
        let saved = save_document_file(
            &shared,
            &target,
            &[],
            ChangedFile::Ask,
            zenkai_engine::save_xlsx,
        );
        assert!(
            matches!(saved, Ok(DocumentSave::Written { .. })),
            "{saved:?}"
        );
        assert_eq!(saved_text(&target), "mine");
    }

    #[test]
    fn a_changed_target_asks_and_other_errors_never_retry() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        let other = dir.path().join("other.xlsx");
        std::fs::write(&target, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let changed = |path: &Path| EngineError::ChangedOnDisk(path.to_path_buf());
        assert!(!retries_whole(&changed(&target), &target, ChangedFile::Ask));
        assert!(retries_whole(
            &changed(&target),
            &target,
            ChangedFile::Replace
        ));
        assert!(retries_whole(&changed(&other), &target, ChangedFile::Ask));
        let full = EngineError::VerifyFailed("disk full".to_string());
        assert!(!retries_whole(&full, &target, ChangedFile::Replace));
    }

    fn damaged_keeping_stamp(path: &Path) {
        let stamp = std::fs::metadata(path).unwrap();
        std::fs::write(path, vec![0u8; stamp.len() as usize]).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(stamp.modified().unwrap()).unwrap();
    }

    fn open_with_damaged_file(target: &Path) -> RwLock<Workbook> {
        let a1 = CellPos::default();
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), a1, "first").unwrap();
        zenkai_engine::save_xlsx(book.save_snapshot(target, SaveTarget::Document).unwrap())
            .unwrap();
        let mut book = zenkai_engine::open_xlsx(target).unwrap().workbook;
        damaged_keeping_stamp(target);
        book.set_input(SheetId(0), a1, "mine").unwrap();
        RwLock::new(book)
    }

    #[test]
    fn a_save_over_an_unusable_file_stops_without_writing_until_replace_is_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        let shared = open_with_damaged_file(&target);
        let before = std::fs::read(&target).unwrap();
        let asked = save_document_file(
            &shared,
            &target,
            &[],
            ChangedFile::Ask,
            zenkai_engine::save_xlsx,
        );
        assert_eq!(asked.unwrap(), DocumentSave::BaseChanged);
        assert_eq!(std::fs::read(&target).unwrap(), before);
        let replaced = save_document_file(
            &shared,
            &target,
            &[],
            ChangedFile::Replace,
            zenkai_engine::save_xlsx,
        );
        assert!(matches!(replaced, Ok(DocumentSave::Written { .. })));
        assert_eq!(saved_text(&target), "mine");
    }

    #[test]
    fn a_recovery_copy_is_written_whole_even_when_the_document_file_is_unusable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("book.xlsx");
        let recovery = dir.path().join("recovery.xlsx");
        let shared = open_with_damaged_file(&target);
        let before = std::fs::read(&target).unwrap();
        let written = write_snapshot(
            &shared,
            FileJob::Autosaving,
            &recovery,
            SaveTarget::Copy,
            zenkai_engine::save_xlsx,
        );
        assert!(written.is_ok(), "{written:?}");
        assert_eq!(saved_text(&recovery), "mine");
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }
}
