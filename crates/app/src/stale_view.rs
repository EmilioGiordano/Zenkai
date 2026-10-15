use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use zenkai_engine::{Engine, Workbook};
use zenkai_grid::GridCell;
use zenkai_types::{CellPos, Range, SheetId};

use crate::document::read_cells;

// Counted in cells so the copy stays small whatever the sheet shape: a few tens of megabytes
// at most, dropped when the batch ends.
const STALE_CELL_BUDGET: u64 = 120_000;

// What the grid needs of a sheet, as it was before an edit batch took the write lock.
// The engine recalculates in place with no way to yield, so this copy is the only thing
// that can answer reads of rows the grid never loaded until the batch ends. It covers a
// window of rows around the viewport, not the whole sheet.
pub struct StaleView {
    sheet: SheetId,
    formulas: bool,
    window: Vec<Range>,
    cells: HashMap<CellPos, GridCell>,
}

pub struct StaleRequest {
    pub slot: Arc<StaleSlot>,
    pub sheet: SheetId,
    pub formulas: bool,
    pub viewport: Vec<Range>,
}

#[derive(Default)]
pub struct StaleSlot(Mutex<Option<Arc<StaleView>>>);

impl StaleSlot {
    pub fn current(&self) -> Option<Arc<StaleView>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn publish(&self, view: StaleView) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(view));
    }
}

impl StaleRequest {
    // Runs on the batch thread under a read lock, before the write lock is taken, so the
    // copy is published by the time any reader finds the workbook busy.
    pub fn publish_from(&self, workbook: &Workbook) {
        let window = window_around(&self.viewport, workbook.used_end(self.sheet));
        let cells = read_cells(workbook, self.sheet, &window, self.formulas);
        self.slot.publish(StaleView {
            sheet: self.sheet,
            formulas: self.formulas,
            window,
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
                self.window
                    .iter()
                    .filter_map(|window| range.intersection(window))
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
// used area like a live read.
fn window_around(viewport: &[Range], used_end: CellPos) -> Vec<Range> {
    let columns: u64 = viewport
        .iter()
        .map(|range| u64::from(range.cols()))
        .sum::<u64>()
        .max(1);
    let margin = i64::try_from(STALE_CELL_BUDGET / columns / 2).unwrap_or(i64::MAX);
    let limit = CellPos::new(used_end.row.offset(1), used_end.col.offset(1));
    viewport
        .iter()
        .filter_map(|range| {
            Range::new(
                CellPos::new(range.start.row.offset(-margin), range.start.col),
                CellPos::new(range.end.row.offset(margin), range.end.col),
            )
            .clip_to(limit)
        })
        .collect()
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
        assert_eq!(window[0].start, pos("A1"));
        assert_eq!(window[0].end.col, pos("C1").col);
        assert!(window[0].end.row.get() <= 5001);
    }

    #[test]
    fn the_window_never_exceeds_the_cell_budget_by_much() {
        let viewport = [range("A100000:Z100050")];
        let window = window_around(&viewport, pos("Z900000"));
        let cells: u64 = window.iter().map(Range::cell_count).sum();
        let viewport_cells: u64 = viewport.iter().map(Range::cell_count).sum();
        assert!(cells <= STALE_CELL_BUDGET + viewport_cells);
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
        document.queue(Box::new(move |workbook| {
            let all_new: Vec<Vec<String>> = (0..rows).map(|_| vec!["new".to_string()]).collect();
            workbook.set_inputs(SheetId(0), CellPos::default(), &all_new)?;
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        let generation = document.generation();
        let (shared, edits) = document.take_batch().unwrap();
        let slot = document.stale_slot();
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
        document.queue(set(0, 0, "new"));
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
