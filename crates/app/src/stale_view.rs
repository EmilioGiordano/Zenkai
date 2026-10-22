use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zenkai_engine::{Engine, Workbook};
use zenkai_grid::GridCell;
use zenkai_types::{CellPos, Range, RowIdx, SheetId};

use crate::document::read_cells;

// Counted in cells so the copy stays small whatever the sheet shape: a few tens of megabytes
// at most, dropped when the batch ends.
const STALE_CELL_BUDGET: u64 = 120_000;

// A few cells pointing at huge text would still make the copy large, so the text copied is
// capped too.
const STALE_TEXT_BYTE_BUDGET: usize = 8 * 1024 * 1024;

// What the grid needs of a sheet, as it was before an edit batch took the write lock.
// The engine recalculates in place with no way to yield, so this copy is the only thing
// that can answer reads of rows the grid never loaded until the batch ends. It covers a
// window of rows around the viewport, not the whole sheet.
pub struct StaleView {
    sheet: SheetId,
    formulas: bool,
    covered: Vec<Range>,
    cells: HashMap<CellPos, GridCell>,
}

pub struct StaleRequest {
    pub slot: Arc<StaleSlot>,
    pub sheet: SheetId,
    pub formulas: bool,
    pub viewport: Vec<Range>,
}

// A document whose last batch held the write lock this long gets a copy before the next one;
// faster ones never pay the cell reads.
pub const SLOW_BATCH: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct StaleSlot {
    view: Mutex<Option<Arc<StaleView>>>,
    last_hold_micros: AtomicU64,
}

impl StaleSlot {
    pub fn record_hold(&self, held: Duration) {
        let micros = held.as_micros().min(u128::from(u64::MAX)) as u64;
        self.last_hold_micros.store(micros, Ordering::Relaxed);
    }

    pub fn last_batch_was_slow(&self) -> bool {
        u128::from(self.last_hold_micros.load(Ordering::Relaxed)) >= SLOW_BATCH.as_micros()
    }

    pub fn current(&self) -> Option<Arc<StaleView>> {
        self.view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn clear(&self) {
        *self.view.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn publish(&self, view: StaleView) {
        *self.view.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(view));
    }
}

impl StaleRequest {
    // Runs on the batch thread under a read lock, before the write lock is taken, so the
    // copy is published by the time any reader finds the workbook busy.
    pub fn publish_from(&self, workbook: &Workbook) {
        let windows = window_around(&self.viewport, workbook.used_end(self.sheet));
        let (covered, cells) = copy_within_budget(workbook, self.sheet, self.formulas, &windows);
        self.slot.publish(StaleView {
            sheet: self.sheet,
            formulas: self.formulas,
            covered,
            cells,
        });
    }
}

impl StaleView {
    // The cells of `ranges` the copy holds, with the part of `ranges` it covers. Anything
    // outside that part is unknown, not blank, so the caller must not treat it as deleted.
    pub fn cells_in(
        &self,
        sheet: SheetId,
        formulas: bool,
        ranges: &[Range],
    ) -> Option<(HashMap<CellPos, GridCell>, Vec<Range>)> {
        if sheet != self.sheet || formulas != self.formulas {
            return None;
        }
        let covered: Vec<Range> = ranges
            .iter()
            .flat_map(|range| {
                self.covered
                    .iter()
                    .filter_map(|covered| range.intersection(covered))
            })
            .collect();
        if covered.is_empty() {
            return None;
        }
        let cells = covered
            .iter()
            .flat_map(Range::positions)
            .filter_map(|pos| Some((pos, self.cells.get(&pos)?.clone())))
            .collect();
        Some((cells, covered))
    }
}

// Every range grows by as many rows each way as the budget allows, then is cut at the
// used area like a live read. Each entry is the viewport range and its window.
fn window_around(viewport: &[Range], used_end: CellPos) -> Vec<(Range, Range)> {
    let columns: u64 = viewport
        .iter()
        .map(|range| u64::from(range.cols()))
        .sum::<u64>()
        .max(1);
    let margin = (STALE_CELL_BUDGET / columns / 2).min(i64::MAX as u64) as i64;
    let limit = CellPos::new(used_end.row.offset(1), used_end.col.offset(1));
    viewport
        .iter()
        .filter_map(|range| {
            let window = Range::new(
                CellPos::new(range.start.row.offset(-margin), range.start.col),
                CellPos::new(range.end.row.offset(margin), range.end.col),
            )
            .clip_to(limit)?;
            Some((*range, window))
        })
        .collect()
}

fn row_of(window: Range, row: u32) -> Range {
    let row = RowIdx::clamped(i64::from(row));
    Range::new(
        CellPos::new(row, window.start.col),
        CellPos::new(row, window.end.col),
    )
}

// The rows of `window` by distance from the middle of the viewport, alternating below and
// above, so what is nearest the screen is copied first.
fn rows_outward(viewport: Range, window: Range) -> impl Iterator<Item = u32> {
    let (first, last) = (window.start.row.get(), window.end.row.get());
    let middle = ((viewport.start.row.get() + viewport.end.row.get()) / 2).clamp(first, last);
    let reach = (last - middle).max(middle - first);
    (0..=reach).flat_map(move |distance| {
        let below = middle.checked_add(distance).filter(|row| *row <= last);
        let above = middle
            .checked_sub(distance)
            .filter(|row| distance > 0 && *row >= first);
        [below, above].into_iter().flatten()
    })
}

// Copies one row at a time, a window after another in turn, and stops before the row that
// would pass the text budget. Each window's copied rows form one block around the middle of
// its viewport, which is the range reported as covered.
fn copy_within_budget(
    workbook: &Workbook,
    sheet: SheetId,
    formulas: bool,
    windows: &[(Range, Range)],
) -> (Vec<Range>, HashMap<CellPos, GridCell>) {
    let mut rows: Vec<_> = windows
        .iter()
        .map(|(viewport, window)| rows_outward(*viewport, *window))
        .collect();
    let mut copied: Vec<Option<(u32, u32)>> = vec![None; windows.len()];
    let mut cells = HashMap::new();
    let mut bytes = 0usize;
    'rounds: loop {
        let mut progressed = false;
        for (index, next) in rows.iter_mut().enumerate() {
            let Some(row) = next.next() else { continue };
            progressed = true;
            let row_cells = read_cells(workbook, sheet, &[row_of(windows[index].1, row)], formulas);
            let row_bytes: usize = row_cells.values().map(|cell| cell.text.len()).sum();
            if bytes + row_bytes > STALE_TEXT_BYTE_BUDGET {
                break 'rounds;
            }
            bytes += row_bytes;
            cells.extend(row_cells);
            copied[index] = Some(match copied[index] {
                Some((low, high)) => (low.min(row), high.max(row)),
                None => (row, row),
            });
        }
        if !progressed {
            break;
        }
    }
    let covered = windows
        .iter()
        .zip(copied)
        .filter_map(|((_, window), rows)| {
            let (low, high) = rows?;
            Some(Range::new(
                row_of(*window, low).start,
                row_of(*window, high).end,
            ))
        })
        .collect();
    (covered, cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Edit, run_batch_serving};
    use crate::spaces::SpaceId;
    use proptest::prelude::*;
    use std::sync::mpsc;
    use zenkai_types::{ColIdx, RowIdx, WorkbookId};

    fn pos(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    fn at(row: u32, col: u16) -> CellPos {
        CellPos::new(
            RowIdx::clamped(i64::from(row)),
            ColIdx::clamped(i64::from(col)),
        )
    }

    fn set(row: u32, col: u16, text: &str) -> Edit {
        let text = text.to_string();
        Box::new(move |workbook| workbook.set_inputs(SheetId(0), at(row, col), &[vec![text]]))
    }

    fn workbook_with_column(rows: u32, text: &str) -> Workbook {
        let mut workbook = Workbook::new_empty().unwrap();
        let column: Vec<Vec<String>> = (0..rows).map(|_| vec![text.to_string()]).collect();
        workbook
            .set_inputs(SheetId(0), CellPos::default(), &column)
            .unwrap();
        workbook.warm_used_areas();
        workbook
    }

    fn request(slot: &Arc<StaleSlot>, viewport: &str) -> StaleRequest {
        StaleRequest {
            slot: Arc::clone(slot),
            sheet: SheetId(0),
            formulas: false,
            viewport: vec![range(viewport)],
        }
    }

    #[test]
    fn the_window_grows_by_rows_and_stops_at_the_used_area() {
        let window = window_around(&[range("A1000:C1050")], pos("C5000"));
        assert_eq!(window.len(), 1);
        let area = window[0].1;
        assert_eq!(area.start, pos("A1"));
        assert_eq!(area.end.col, pos("C1").col);
        assert!(area.end.row.get() <= 5001);
    }

    #[test]
    fn the_window_never_exceeds_the_cell_budget_by_much() {
        let viewport = [range("A100000:Z100050")];
        let window = window_around(&viewport, pos("Z900000"));
        let cells: u64 = window.iter().map(|(_, area)| area.cell_count()).sum();
        let viewport_cells: u64 = viewport.iter().map(Range::cell_count).sum();
        assert!(cells <= STALE_CELL_BUDGET + viewport_cells);
    }

    #[test]
    fn huge_text_shrinks_the_covered_rows_and_keeps_the_copy_under_the_byte_budget() {
        let workbook = workbook_with_column(600, &"x".repeat(20_000));
        let slot = Arc::new(StaleSlot::default());
        request(&slot, "A290:A310").publish_from(&workbook);
        let view = slot.current().unwrap();
        let bytes: usize = view.cells.values().map(|cell| cell.text.len()).sum();
        assert!(bytes <= STALE_TEXT_BYTE_BUDGET);
        let covered = view.covered[0];
        assert!(covered.contains(pos("A300")));
        assert!(!covered.contains(pos("A1")) && !covered.contains(pos("A600")));
        let (cells, reported) = view
            .cells_in(SheetId(0), false, &[range("A1:A600")])
            .unwrap();
        assert!(reported.iter().all(|r| covered.intersection(r) == Some(*r)));
        assert!(cells.keys().all(|at| covered.contains(*at)));
    }

    #[test]
    fn rows_are_visited_outward_from_the_middle_of_the_viewport() {
        let rows: Vec<u32> = rows_outward(range("A8:A12"), range("A1:A20")).collect();
        assert_eq!(&rows[..5], &[9, 10, 8, 11, 7]);
        assert_eq!(rows.len(), 20);
    }

    #[test]
    fn a_range_outside_the_window_is_unknown_not_blank() {
        let workbook = workbook_with_column(50, "old");
        let slot = Arc::new(StaleSlot::default());
        request(&slot, "A1:A10").publish_from(&workbook);
        let view = slot.current().unwrap();
        let (cells, covered) = view
            .cells_in(
                SheetId(0),
                false,
                &[range("A1:A10"), range("A900000:A900010")],
            )
            .unwrap();
        assert_eq!(cells[&pos("A5")].text.as_ref(), "old");
        assert!(covered.iter().all(|r| !r.contains(pos("A900005"))));
        assert!(
            view.cells_in(SheetId(0), false, &[range("A900000:A900010")])
                .is_none()
        );
    }

    #[test]
    fn a_copy_made_for_another_sheet_or_view_is_not_served() {
        let workbook = workbook_with_column(5, "old");
        let slot = Arc::new(StaleSlot::default());
        request(&slot, "A1:A5").publish_from(&workbook);
        let view = slot.current().unwrap();
        assert!(
            view.cells_in(SheetId(1), false, &[range("A1:A5")])
                .is_none()
        );
        assert!(view.cells_in(SheetId(0), true, &[range("A1:A5")]).is_none());
    }

    #[test]
    fn rows_the_grid_never_loaded_read_their_old_values_during_a_slow_batch() {
        let rows = 3_000;
        let mut document = Document::new(
            WorkbookId(0),
            SpaceId(0),
            1,
            workbook_with_column(rows, "old"),
            None,
            Vec::new(),
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        document.queue_in_test(Box::new(move |workbook| {
            let all_new: Vec<Vec<String>> = (0..rows).map(|_| vec!["new".to_string()]).collect();
            workbook.set_inputs(SheetId(0), CellPos::default(), &all_new)?;
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        let generation = document.generation();
        let (shared, edits) = document.take_batch().unwrap();
        let slot = document.stale_slot();
        slot.record_hold(SLOW_BATCH);
        let batch = std::thread::spawn({
            let request = request(&slot, "A1:A40");
            move || run_batch_serving(&shared, edits, Some(request))
        });
        started_rx.recv().unwrap();

        assert!(document.workbook().is_none());
        let unloaded = range("A2000:A2100");
        for _ in 0..20 {
            let (cells, covered) = document.stale_cells(&[unloaded], false).unwrap();
            assert_eq!(covered, vec![unloaded]);
            assert_eq!(cells.len(), 101);
            assert!(cells.values().all(|cell| cell.text.as_ref() == "old"));
        }

        release_tx.send(()).unwrap();
        assert!(batch.join().unwrap().is_empty());
        assert!(document.finish_batch(generation));
        assert!(document.stale_cells(&[unloaded], false).is_none());
        let live = document.cells(&[unloaded], false).unwrap();
        assert!(live.values().all(|cell| cell.text.as_ref() == "new"));
    }

    fn run_with_copy(slot: &Arc<StaleSlot>, edit: Edit) {
        let shared = std::sync::RwLock::new(workbook_with_column(3, "old"));
        assert!(run_batch_serving(&shared, vec![edit], Some(request(slot, "A1:A3"))).is_empty());
    }

    #[test]
    fn a_document_with_no_slow_batch_yet_builds_no_copy() {
        let slot = Arc::new(StaleSlot::default());
        assert!(!slot.last_batch_was_slow());
        let seen = Arc::clone(&slot);
        run_with_copy(
            &slot,
            Box::new(move |_| {
                assert!(seen.current().is_none());
                Ok(())
            }),
        );
    }

    #[test]
    fn a_fast_batch_keeps_the_next_one_copy_free() {
        let slot = Arc::new(StaleSlot::default());
        slot.record_hold(SLOW_BATCH);
        slot.record_hold(SLOW_BATCH / 2);
        assert!(!slot.last_batch_was_slow());
    }

    #[test]
    fn a_slow_batch_makes_the_next_one_build_a_copy() {
        let slot = Arc::new(StaleSlot::default());
        slot.record_hold(SLOW_BATCH);
        assert!(slot.last_batch_was_slow());
        let seen = Arc::clone(&slot);
        run_with_copy(
            &slot,
            Box::new(move |_| {
                assert!(seen.current().is_some());
                Ok(())
            }),
        );
    }

    #[test]
    fn a_batch_records_how_long_it_held_the_lock() {
        let slot = Arc::new(StaleSlot::default());
        run_with_copy(
            &slot,
            Box::new(|_| {
                std::thread::sleep(SLOW_BATCH);
                Ok(())
            }),
        );
        assert!(slot.last_batch_was_slow());
    }

    #[test]
    fn a_batch_without_a_request_publishes_nothing() {
        let mut document = Document::new(
            WorkbookId(0),
            SpaceId(0),
            1,
            workbook_with_column(3, "old"),
            None,
            Vec::new(),
        );
        document.queue_in_test(set(0, 0, "new"));
        let (shared, edits) = document.take_batch().unwrap();
        assert!(run_batch_serving(&shared, edits, None).is_empty());
        assert!(document.stale_slot().current().is_none());
    }

    fn all_text(workbook: &Workbook) -> HashMap<CellPos, String> {
        let end = workbook.used_end(SheetId(0));
        let everything = Range::new(CellPos::default(), end);
        read_cells(workbook, SheetId(0), &[everything], false)
            .into_iter()
            .map(|(pos, cell)| (pos, cell.text.to_string()))
            .collect()
    }

    fn typed() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("1".to_string()),
            Just("hello".to_string()),
            Just("=A1+1".to_string()),
            Just("=SUM(A1:A5)".to_string()),
            Just("".to_string()),
        ]
    }

    proptest! {
        #[test]
        fn batches_with_a_published_copy_end_where_a_sequential_run_does(
            edits in proptest::collection::vec((0u32..12, 0u16..3, typed()), 1..24),
            cuts in proptest::collection::vec(1usize..6, 1..8),
        ) {
            let mut sequential = Workbook::new_empty().unwrap();
            for (row, col, text) in &edits {
                sequential.set_inputs(SheetId(0), at(*row, *col), &[vec![text.clone()]]).unwrap();
            }

            let shared = std::sync::RwLock::new(Workbook::new_empty().unwrap());
            let slot = Arc::new(StaleSlot::default());
            let mut rest = edits.as_slice();
            for cut in cuts.iter().cycle() {
                if rest.is_empty() {
                    break;
                }
                let (now, later) = rest.split_at((*cut).min(rest.len()));
                rest = later;
                let batch: Vec<Edit> = now
                    .iter()
                    .map(|(row, col, text)| set(*row, *col, text))
                    .collect();
                prop_assert!(run_batch_serving(&shared, batch, Some(request(&slot, "A1:C12"))).is_empty());
                slot.clear();
            }
            let batched = shared.read().unwrap();
            prop_assert_eq!(all_text(&batched), all_text(&sequential));
        }
    }
}
