use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

const SHEET: SheetId = SheetId(0);

#[test]
fn freezing_rows_and_columns_is_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_frozen(SHEET, 2, 2).unwrap();
    assert_eq!(book.frozen(SHEET), (2, 2));
    book.undo().unwrap();
    assert_eq!(book.frozen(SHEET), (0, 0));
    book.redo().unwrap();
    assert_eq!(book.frozen(SHEET), (2, 2));
}

#[test]
fn undoing_a_new_freeze_restores_the_previous_one() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_frozen(SHEET, 1, 1).unwrap();
    book.set_frozen(SHEET, 2, 2).unwrap();
    book.undo().unwrap();
    assert_eq!(book.frozen(SHEET), (1, 1));
}

#[test]
fn freezing_does_not_recalculate_volatile_formulas() {
    let mut book = Workbook::new_empty().unwrap();
    let at = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(0));
    book.set_input(SHEET, at, "=RAND()").unwrap();
    let before = book.cell(SHEET, at).text;
    book.set_frozen(SHEET, 2, 2).unwrap();
    assert_eq!(book.cell(SHEET, at).text, before);
}
