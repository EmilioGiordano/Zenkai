// Undoing an input that set a number format on an empty cell takes the format away too,
// as in Excel (GitHub #102).
#![allow(clippy::unwrap_used)]

use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, SheetId};

const SHEET: SheetId = SheetId(0);

fn at(text: &str) -> CellPos {
    CellPos::parse_a1(text).unwrap()
}

#[test]
fn undo_typed_percent_leaves_no_format_behind() {
    let mut book = Workbook::new_empty().unwrap();
    let before = book.cell(SHEET, at("B3"));
    book.set_input(SHEET, at("B3"), "5%").unwrap();
    assert_eq!(book.cell(SHEET, at("B3")).text, "5%");

    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, at("B3")), before);

    book.set_input(SHEET, at("B3"), "=ROW()*2").unwrap();
    assert_eq!(book.cell(SHEET, at("B3")).text, "6");
}

#[test]
fn undo_filled_inputs_leave_no_format_behind() {
    let mut book = Workbook::new_empty().unwrap();
    let before = book.cell(SHEET, at("A1"));
    let rows = vec![vec!["5%".to_string(); 2]; 2];
    book.set_inputs(SHEET, at("A1"), &rows).unwrap();
    assert_eq!(book.cell(SHEET, at("B2")).text, "5%");

    book.undo().unwrap();
    for name in ["A1", "A2", "B1", "B2"] {
        assert_eq!(book.cell(SHEET, at(name)), before, "{name}");
    }

    book.redo().unwrap();
    assert_eq!(book.cell(SHEET, at("B2")).text, "5%");
}
