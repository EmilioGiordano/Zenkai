use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::num::NonZero;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{Builder, Scope, ScopedJoinHandle};

use crate::ReadError;
use crate::limits::CellBudget;
use crate::package::WorksheetPart;
use crate::sheet_stream::{StreamedSheet, Tuning, stream_worksheet};
use crate::stream::{Budgets, CompressedMeter, Counted};

// A worksheet smaller than this streams in a fraction of a second, less than starting a
// thread and reading the zip directory again costs.
const MIN_THREAD_SHEET_BYTES: u64 = 8 * 1024 * 1024;

// How an open uses the cores: a thread budget, which is every core but the one left to
// the UI, and the sheets large enough to be worth threads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadPlan {
    pub(crate) threads: usize,
    pub(crate) min_sheet_bytes: u64,
    pub(crate) cells: CellBudget,
    pub(crate) tuning: Tuning,
}

// Parsing runs at 72 MB/s and inflating at 860 MB/s in the generated-sheet measurement, so
// about twelve parsers saturate one inflater; six give half that rate and bound what a
// sheet holds uncharged: the workers plus IN_FLIGHT_EXTRA pieces of PIECE_BYTES each,
// parsed or being parsed, 7 * 4.25 MB of text before the cells they become. Serial holds one.
const MAX_WORKERS_PER_SHEET: usize = 6;

// Threads that stream sheets, and for each of them the threads that scan its rows. A sheet
// thread inflates and merges while its workers parse, so a sheet thread and its workers
// together are never more than the budget: sheet_threads * (1 + workers) <= threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) sheet_threads: usize,
    pub(crate) workers: usize,
}

impl ReadPlan {
    pub(crate) fn detect() -> ReadPlan {
        let cores = std::thread::available_parallelism().map_or(1, NonZero::get);
        ReadPlan::with_threads(cores.saturating_sub(1))
    }

    pub(crate) fn with_threads(threads: usize) -> ReadPlan {
        ReadPlan {
            threads: if threads >= 2 { threads } else { 0 },
            min_sheet_bytes: MIN_THREAD_SHEET_BYTES,
            cells: CellBudget::production(),
            tuning: Tuning::production(0),
        }
    }

    pub(crate) fn single_threaded(&self) -> ReadPlan {
        ReadPlan {
            threads: 0,
            ..*self
        }
    }

    pub(crate) fn is_serial(&self) -> bool {
        self.threads == 0
    }

    // A third of the budget at most goes to threads that stream sheets at once, since the
    // parse behind each is what takes the time; the rest scan rows. A single sheet gets
    // every thread but its own.
    pub(crate) fn layout(&self, sheets: usize) -> Option<Layout> {
        if self.is_serial() || sheets == 0 {
            return None;
        }
        let sheet_threads = sheets.min((self.threads / 3).max(1));
        Some(Layout {
            sheet_threads,
            workers: ((self.threads - sheet_threads) / sheet_threads).min(MAX_WORKERS_PER_SHEET),
        })
    }
}

pub(crate) type Streamed = Result<StreamedSheet, ReadError>;

pub(crate) type Open<'a, R> =
    dyn Fn() -> Result<(zip::ZipArchive<Counted<R>>, CompressedMeter), ReadError> + Sync + 'a;

#[derive(Clone, Copy)]
pub(crate) struct SheetTask {
    pub(crate) index: usize,
    pub(crate) position: usize,
}

pub(crate) struct Job<'j, R> {
    open: &'j Open<'j, R>,
    worksheets: &'j [WorksheetPart],
    budgets: &'j Budgets,
    tasks: &'j [SheetTask],
    layout: Option<Layout>,
    tuning: Tuning,
    next: AtomicUsize,
}

// Worksheets streaming on their own threads, each with its own handle on the archive and its
// own meter. They are handed back in the archive's order, so the first failure is the one
// a single thread would have met.
pub(crate) struct SheetStreams<'scope> {
    arrived: Receiver<(usize, Streamed)>,
    held: HashMap<usize, Streamed>,
    indexes: HashSet<usize>,
    handles: Vec<ScopedJoinHandle<'scope, ()>>,
}

impl<'scope> SheetStreams<'scope> {
    // None when no thread could start, or when there is nothing worth a thread.
    pub(crate) fn start<'env, R: Read + Seek>(
        scope: &'scope Scope<'scope, 'env>,
        job: &'env Job<'env, R>,
    ) -> Option<SheetStreams<'scope>> {
        let threads = job.layout?.sheet_threads;
        let (sender, arrived) = channel();
        let parent = tracing::Span::current();
        let mut handles = Vec::new();
        for number in 0..threads {
            let sender = sender.clone();
            let parent = parent.clone();
            let spawned = Builder::new()
                .name(format!("xlsx-sheet-{number}"))
                .spawn_scoped(scope, move || run(job, &sender, &parent));
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    tracing::warn!(%error, "a worksheet thread did not start");
                    break;
                }
            }
        }
        if handles.is_empty() {
            return None;
        }
        Some(SheetStreams {
            arrived,
            held: HashMap::new(),
            indexes: job.tasks.iter().map(|task| task.index).collect(),
            handles,
        })
    }

    // The streamed sheet of the archive entry at `index`, waiting for its thread; None
    // when that entry is not streamed on a thread. Keyed by entry, not worksheet, since an
    // archive may hold a worksheet path twice and each is read, as on one thread.
    pub(crate) fn take(&mut self, index: usize) -> Option<Streamed> {
        if !self.indexes.contains(&index) {
            return None;
        }
        loop {
            if let Some(streamed) = self.held.remove(&index) {
                return Some(streamed);
            }
            match self.arrived.recv() {
                Ok((arrived, streamed)) => {
                    self.held.insert(arrived, streamed);
                }
                Err(_) => {
                    return Some(Err(ReadError::Unreadable(
                        "a worksheet reader stopped".to_string(),
                    )));
                }
            }
        }
    }

    pub(crate) fn finish(self) {
        for handle in self.handles {
            if handle.join().is_err() {
                tracing::error!("a worksheet thread panicked");
            }
        }
    }
}

impl<'j, R> Job<'j, R> {
    pub(crate) fn new(
        open: &'j Open<'j, R>,
        worksheets: &'j [WorksheetPart],
        budgets: &'j Budgets,
        tasks: &'j [SheetTask],
        plan: &ReadPlan,
    ) -> Job<'j, R> {
        let layout = plan.layout(tasks.len());
        Job {
            open,
            worksheets,
            budgets,
            tasks,
            layout,
            tuning: Tuning {
                workers: layout.map_or(0, |layout| layout.workers),
                ..plan.tuning
            },
            next: AtomicUsize::new(0),
        }
    }
}

fn run<R: Read + Seek>(
    job: &Job<'_, R>,
    sender: &Sender<(usize, Streamed)>,
    parent: &tracing::Span,
) {
    let mut opened = None;
    loop {
        let at = job.next.fetch_add(1, Ordering::Relaxed);
        let Some(task) = job.tasks.get(at) else {
            return;
        };
        if job.budgets.cancelled() {
            return;
        }
        let streamed = parent.in_scope(|| stream_task(job, &mut opened, *task));
        if sender.send((task.index, streamed)).is_err() {
            return;
        }
    }
}

fn stream_task<R: Read + Seek>(
    job: &Job<'_, R>,
    opened: &mut Option<(zip::ZipArchive<Counted<R>>, CompressedMeter)>,
    task: SheetTask,
) -> Streamed {
    if opened.is_none() {
        *opened = Some((job.open)()?);
    }
    let Some((archive, meter)) = opened.as_mut() else {
        return Err(ReadError::Unreadable(
            "a worksheet reader stopped".to_string(),
        ));
    };
    let part = &job.worksheets[task.position];
    let entry = archive
        .by_index(task.index)
        .map_err(|e| ReadError::Unreadable(e.to_string()))?;
    let _sheet_span =
        tracing::info_span!("open_sheet", sheet = task.position, bytes = entry.size()).entered();
    stream_worksheet(
        entry,
        meter,
        &part.path,
        &part.name,
        job.budgets,
        job.tuning,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::test_sheets::{SheetShape, sheet_xml, workbook_with_entries, workbook_with_sheets};
    use crate::{OpenedBook, workbook_differences};

    fn generated(seed: u64) -> Vec<u8> {
        let shape = SheetShape {
            shared_strings: 3,
            ..SheetShape::plain(300, seed)
        };
        sheet_xml(&shape).into_bytes()
    }

    fn threaded() -> ReadPlan {
        ReadPlan {
            threads: 6,
            tuning: Tuning::production(0),
            min_sheet_bytes: 0,
            cells: CellBudget::production(),
        }
    }

    type Seen = Vec<(String, usize)>;

    fn read(
        bytes: &[u8],
        plan: &ReadPlan,
        reject: Option<&str>,
    ) -> (Result<OpenedBook, ReadError>, Seen) {
        let seen = Mutex::new(Seen::new());
        let result = crate::read_xlsx_planned(bytes, plan, "b", "en", "UTC", &|name, part| {
            seen.lock().unwrap().push((name.to_string(), part.len()));
            match reject {
                Some(rejected) if rejected == name => {
                    Err(ReadError::Unsupported("rejected".to_string()))
                }
                _ => Ok(()),
            }
        });
        (result, seen.into_inner().unwrap())
    }

    fn assert_same_read(sheets: &[Vec<u8>], plan: &ReadPlan, reject: Option<&str>) -> bool {
        assert_same_bytes(&workbook_with_sheets(sheets), plan, reject)
    }

    fn assert_same_bytes(bytes: &[u8], plan: &ReadPlan, reject: Option<&str>) -> bool {
        let (alone, alone_seen) = read(bytes, &ReadPlan::with_threads(0), reject);
        let (together, together_seen) = read(bytes, plan, reject);
        assert_eq!(alone_seen, together_seen);
        match (alone, together) {
            (Ok(alone), Ok(together)) => {
                assert!(workbook_differences(&alone.book, &together.book, 10).is_empty());
                assert_eq!(alone.sheet_configs, together.sheet_configs);
                assert_eq!(alone.layout, together.layout);
                true
            }
            (Err(alone), Err(together)) => {
                assert_eq!(format!("{alone:?}"), format!("{together:?}"));
                false
            }
            (alone, together) => panic!(
                "one thread {:?}, several {:?}",
                alone.map(|_| ()),
                together.map(|_| ())
            ),
        }
    }

    #[test]
    fn sheets_on_threads_build_the_workbook_of_one_thread() {
        let sheets: Vec<Vec<u8>> = (1..=5).map(generated).collect();
        assert!(assert_same_read(&sheets, &threaded(), None));
    }

    #[test]
    fn a_worksheet_path_twice_in_the_archive_reads_as_on_one_thread() {
        let sheets = [generated(1), generated(2)];
        let again = generated(3);
        let bytes = workbook_with_entries(&sheets, &[("xl/worksheets/sheet1.xml", &again)]);
        assert_same_bytes(&bytes, &threaded(), None);
    }

    #[test]
    fn the_first_failing_sheet_in_archive_order_is_the_one_reported() {
        let rows_out_of_order = b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"2\"><c r=\"A2\"><v>1</v></c></row><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>".to_vec();
        let not_closed = b"<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row>".to_vec();
        for sheets in [
            [generated(1), rows_out_of_order.clone(), not_closed.clone()],
            [generated(1), not_closed.clone(), rows_out_of_order.clone()],
            [not_closed, generated(1), rows_out_of_order],
        ] {
            assert!(!assert_same_read(&sheets, &threaded(), None));
        }
    }

    #[test]
    fn a_part_the_caller_rejects_fails_the_read_as_on_one_thread() {
        let sheets: Vec<Vec<u8>> = (1..=4).map(generated).collect();
        assert!(!assert_same_read(
            &sheets,
            &threaded(),
            Some("xl/worksheets/sheet2.xml"),
        ));
    }

    #[test]
    fn a_refusal_for_the_memory_budget_is_the_one_a_single_thread_gives() {
        let sheets: Vec<Vec<u8>> = (1..=4).map(generated).collect();
        let tight = ReadPlan {
            cells: CellBudget::max_cells_for_test(500),
            ..threaded()
        };
        let bytes = workbook_with_sheets(&sheets);
        let (together, _) = read(&bytes, &tight, None);
        let (alone, _) = read(&bytes, &tight.single_threaded(), None);
        let (Err(alone), Err(together)) = (alone, together) else {
            panic!("the budget must refuse the workbook");
        };
        assert!(matches!(alone, ReadError::Unsafe(_)));
        assert_eq!(format!("{alone:?}"), format!("{together:?}"));
    }

    #[test]
    fn sheets_below_the_size_threshold_stay_on_the_calling_thread() {
        let sheets: Vec<Vec<u8>> = (1..=3).map(generated).collect();
        let plan = ReadPlan {
            min_sheet_bytes: u64::MAX,
            ..threaded()
        };
        assert!(assert_same_read(&sheets, &plan, None));
    }

    #[test]
    fn one_thread_per_core_but_the_one_left_to_the_interface() {
        assert!(ReadPlan::with_threads(0).is_serial());
        assert!(ReadPlan::with_threads(1).is_serial());
        assert_eq!(ReadPlan::with_threads(7).threads, 7);
    }

    #[test]
    fn sheet_threads_and_their_workers_never_pass_the_budget() {
        for threads in 2..=64 {
            for sheets in 1..=12 {
                let layout = ReadPlan::with_threads(threads).layout(sheets).unwrap();
                assert!(layout.sheet_threads >= 1 && layout.sheet_threads <= sheets);
                assert!(layout.sheet_threads * (1 + layout.workers) <= threads);
            }
        }
        assert_eq!(ReadPlan::with_threads(1).layout(3), None);
        assert_eq!(
            ReadPlan::with_threads(64).layout(1).unwrap().workers,
            MAX_WORKERS_PER_SHEET
        );
    }
}
