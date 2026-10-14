use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

const SHEET: SheetId = SheetId(0);

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn rows(cells: &[&[&str]]) -> Vec<Vec<String>> {
    cells
        .iter()
        .map(|row| row.iter().map(ToString::to_string).collect())
        .collect()
}

fn book_with_source_row() -> Workbook {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(
        SHEET,
        pos(0, 0),
        &rows(&[&["5", "=A1*2", "note", "=SUM(A$1:B1)", "Item 1"]]),
    )
    .unwrap();
    book
}

fn row_inputs(book: &Workbook, row: i64) -> Vec<String> {
    (0..5).map(|col| book.input(SHEET, pos(row, col))).collect()
}

#[test]
fn fill_down_mixes_shared_formulas_and_text_in_one_undo_step() {
    let mut book = book_with_source_row();
    book.fill(SHEET, Range::new(pos(0, 0), pos(399, 4)), true)
        .unwrap();
    assert_eq!(
        row_inputs(&book, 399),
        ["5", "=A400*2", "note", "=SUM(A$1:B400)", "Item 1"]
    );
    assert_eq!(book.cell(SHEET, pos(399, 1)).text, "10");
    assert_eq!(book.cell(SHEET, pos(399, 3)).text, "4005");
    book.undo().unwrap();
    assert_eq!(row_inputs(&book, 1), ["", "", "", "", ""]);
    assert_eq!(
        row_inputs(&book, 0),
        ["5", "=A1*2", "note", "=SUM(A$1:B1)", "Item 1"]
    );
    book.redo().unwrap();
    assert_eq!(
        row_inputs(&book, 399),
        ["5", "=A400*2", "note", "=SUM(A$1:B400)", "Item 1"]
    );
    assert_eq!(book.cell(SHEET, pos(399, 1)).text, "10");
}

#[test]
fn fill_right_shares_a_formula_along_a_row() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(
        SHEET,
        pos(0, 0),
        &rows(&[&["1", "=A1+1"], &["10", "=A2+1"]]),
    )
    .unwrap();
    book.fill(SHEET, Range::new(pos(0, 1), pos(1, 6)), false)
        .unwrap();
    assert_eq!(book.input(SHEET, pos(0, 6)), "=F1+1");
    assert_eq!(book.input(SHEET, pos(1, 6)), "=F2+1");
    assert_eq!(book.cell(SHEET, pos(1, 6)).text, "16");
    book.undo().unwrap();
    assert_eq!(book.input(SHEET, pos(0, 2)), "");
    assert_eq!(book.input(SHEET, pos(1, 6)), "");
}

#[test]
fn fill_down_past_the_last_row_makes_the_cell_an_error_like_excel() {
    let mut book = Workbook::new_empty().unwrap();
    let last = 1_048_575;
    book.set_input(SHEET, pos(last - 2, 0), "=A1048575")
        .unwrap();
    book.fill(SHEET, Range::new(pos(last - 2, 0), pos(last, 0)), true)
        .unwrap();
    assert!(book.input(SHEET, pos(last - 1, 0)).starts_with('='));
    assert!(book.cell(SHEET, pos(last, 0)).text.starts_with('#'));
}
