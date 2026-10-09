// Spec: a formula over a range wider than the indexed column limit recalculates when any
// column of the range changes, the first and the last included.
#![allow(clippy::unwrap_used)]

use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

const SHEET: SheetId = SheetId(0);
const LAST_COLUMN: i64 = 19;

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

#[test]
fn editing_any_column_of_a_wide_range_updates_its_sum() {
    for edited in [0, 1, 9, LAST_COLUMN - 1, LAST_COLUMN] {
        let mut book = Workbook::new_empty().unwrap();
        let total = pos(2, 0);
        book.set_input(SHEET, total, "=SUM(A1:T1)").unwrap();
        assert_eq!(book.cell(SHEET, total).text, "0");
        book.set_input(SHEET, pos(0, edited), "7").unwrap();
        assert_eq!(book.cell(SHEET, total).text, "7", "column {edited}");
        book.set_input(SHEET, pos(0, edited), "9").unwrap();
        assert_eq!(book.cell(SHEET, total).text, "9", "column {edited}");
    }
}

#[test]
fn a_cell_outside_a_wide_range_does_not_change_its_sum() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(2, 0), "=SUM(A1:T1)").unwrap();
    book.set_input(SHEET, pos(0, LAST_COLUMN + 1), "5").unwrap();
    assert_eq!(book.cell(SHEET, pos(2, 0)).text, "0");
}
