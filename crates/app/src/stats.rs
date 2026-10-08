use std::num::NonZero;
use std::thread;

use crate::document::contents_of;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, Contents, Range, RowIdx, SheetId};

const MAX_SCANNED: u64 = 500_000;
// Below this many cells the threads cost more than the scan.
const PARALLEL_CELLS: u64 = 20_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SelectionStats {
    pub count: u64,
    pub numbers: u64,
    pub sum: f64,
}

impl SelectionStats {
    fn merge(self, other: SelectionStats) -> SelectionStats {
        SelectionStats {
            count: self.count + other.count,
            numbers: self.numbers + other.numbers,
            sum: self.sum + other.sum,
        }
    }

    pub fn average(&self) -> Option<f64> {
        (self.numbers > 0).then(|| self.sum / self.numbers as f64)
    }
}

pub fn compute(workbook: &Workbook, sheet: SheetId, selection: Range) -> Option<SelectionStats> {
    let end = workbook.used_end(sheet);
    let Some(clipped) = selection.clip_to(end) else {
        return Some(SelectionStats::default());
    };
    if clipped.cell_count() > MAX_SCANNED {
        return None;
    }
    let contents = contents_of(workbook, sheet)?;
    let workers = if clipped.cell_count() < PARALLEL_CELLS {
        1
    } else {
        thread::available_parallelism().map_or(1, NonZero::get) as u32
    };
    let band_rows = clipped.rows().div_ceil(workers);
    let bands: Vec<Range> = (0..workers)
        .filter_map(|band| {
            let first = clipped.start.row.get() + band * band_rows;
            (first <= clipped.end.row.get()).then(|| {
                let last = (first + band_rows - 1).min(clipped.end.row.get());
                Range::new(
                    CellPos::new(RowIdx::clamped(i64::from(first)), clipped.start.col),
                    CellPos::new(RowIdx::clamped(i64::from(last)), clipped.end.col),
                )
            })
        })
        .collect();
    let totals = thread::scope(|scope| {
        let handles: Vec<_> = bands
            .iter()
            .map(|band| scope.spawn(|| tally(*band, &contents)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .fold(SelectionStats::default(), SelectionStats::merge)
    });
    Some(totals)
}

fn tally(band: Range, contents: &(impl Fn(CellPos) -> Contents + Sync)) -> SelectionStats {
    let mut stats = SelectionStats::default();
    for pos in band.positions() {
        match contents(pos) {
            Contents::Empty => {}
            Contents::Number(n) => {
                stats.count += 1;
                stats.numbers += 1;
                stats.sum += n;
            }
            Contents::NonNumeric => stats.count += 1,
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_types::SheetId;

    #[test]
    fn a_missing_sheet_has_no_stats() {
        let workbook = Workbook::new_empty().unwrap();
        assert_eq!(
            compute(&workbook, SheetId(9), Range::parse_a1("A1:A10").unwrap()),
            None
        );
    }

    #[test]
    fn a_large_selection_adds_up_across_threads() {
        let mut workbook = Workbook::new_empty().unwrap();
        let rows: Vec<Vec<String>> = (1..=30_000)
            .map(|n| vec![n.to_string(), "x".into()])
            .collect();
        workbook
            .set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let all = Range::parse_a1("A1:B30000").unwrap();
        let stats = compute(&workbook, SheetId(0), all).unwrap();
        assert_eq!(stats.count, 60_000);
        assert_eq!(stats.numbers, 30_000);
        assert_eq!(stats.sum, 450_015_000.0);
    }
}
