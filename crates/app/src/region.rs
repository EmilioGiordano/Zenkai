use zenkai_types::{CellPos, ColIdx, MAX_COLS, MAX_ROWS, Range, RowIdx};

// Generous for a data block, small enough that the scan never stalls the UI thread.
const MAX_REGION_CELLS: u64 = 2_000_000;

/// Excel's current region: the block around `active` bounded by empty rows and columns
/// (diagonal neighbours count, as in Excel).
pub fn current_region(active: CellPos, filled: impl Fn(CellPos) -> bool) -> Range {
    let (mut top, mut left) = (i64::from(active.row.get()), i64::from(active.col.get()));
    let (mut bottom, mut right) = (top, left);
    let last_row = i64::from(MAX_ROWS) - 1;
    let last_col = i64::from(MAX_COLS) - 1;
    let at = |row: i64, col: i64| CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col));
    let any_in_row = |row: i64, from: i64, to: i64| {
        (0..=last_row).contains(&row)
            && (from.max(0)..=to.min(last_col)).any(|col| filled(at(row, col)))
    };
    let any_in_col = |col: i64, from: i64, to: i64| {
        (0..=last_col).contains(&col)
            && (from.max(0)..=to.min(last_row)).any(|row| filled(at(row, col)))
    };
    // Checked at every step, so even one dense edge cannot run past the cap.
    let within = |top: i64, left: i64, bottom: i64, right: i64| {
        ((bottom - top + 1) as u64) * ((right - left + 1) as u64) <= MAX_REGION_CELLS
    };
    loop {
        let mut grew = false;
        // Each edge runs to its end before the others are checked, so a tall block costs
        // one pass per edge rather than one pass per row.
        while within(top, left, bottom, right) && any_in_row(top - 1, left - 1, right + 1) {
            top -= 1;
            grew = true;
        }
        while within(top, left, bottom, right) && any_in_row(bottom + 1, left - 1, right + 1) {
            bottom += 1;
            grew = true;
        }
        while within(top, left, bottom, right) && any_in_col(left - 1, top - 1, bottom + 1) {
            left -= 1;
            grew = true;
        }
        while within(top, left, bottom, right) && any_in_col(right + 1, top - 1, bottom + 1) {
            right += 1;
            grew = true;
        }
        if !grew || !within(top, left, bottom, right) {
            break;
        }
    }
    Range::new(at(top, left), at(bottom, right))
}

#[cfg(test)]
mod tests {
    use super::{MAX_REGION_CELLS, current_region};
    use zenkai_types::{CellPos, ColIdx, MAX_COLS, MAX_ROWS, Range, RowIdx};

    fn pos(a1: &str) -> CellPos {
        CellPos::parse_a1(a1).unwrap()
    }

    #[test]
    fn grows_to_the_block_including_diagonals() {
        let filled = ["B2", "C2", "B3", "C3", "D4", "F9"].map(pos);
        let region = current_region(pos("B2"), |p| filled.contains(&p));
        assert_eq!(region, Range::new(pos("B2"), pos("D4")));
    }

    #[test]
    fn an_isolated_empty_cell_is_its_own_region() {
        let region = current_region(pos("H8"), |_| false);
        assert_eq!(region, Range::single(pos("H8")));
    }

    #[test]
    fn a_block_in_the_top_left_corner_stops_at_the_sheet_edge() {
        let filled = ["A1", "B1", "A2", "B2"].map(pos);
        let region = current_region(pos("A1"), |p| filled.contains(&p));
        assert_eq!(region, Range::new(pos("A1"), pos("B2")));
    }

    #[test]
    fn a_block_in_the_last_row_and_column_stops_at_the_sheet_edge() {
        let last = CellPos::new(
            RowIdx::clamped(i64::from(MAX_ROWS) - 1),
            ColIdx::clamped(i64::from(MAX_COLS) - 1),
        );
        let above = CellPos::new(RowIdx::clamped(i64::from(MAX_ROWS) - 2), last.col);
        let filled = [last, above];
        let region = current_region(last, |p| filled.contains(&p));
        assert_eq!(region, Range::new(above, last));
    }

    #[test]
    fn a_fully_filled_sheet_stops_growing_at_the_cap() {
        let region = current_region(pos("A1"), |_| true);
        assert!(region.cell_count() <= MAX_REGION_CELLS + u64::from(MAX_ROWS));
        assert!(region.cell_count() < u64::from(MAX_ROWS) * u64::from(MAX_COLS));
    }
}
