// Undoing an input that set a number format on an empty cell takes the format away too,
// as in Excel (GitHub #102).
#![allow(clippy::unwrap_used)]

use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, Range, SheetId};

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

#[test]
fn undo_typed_dates_leave_no_format_behind() {
    for date in ["1/2/2024", "2024-01-02"] {
        let mut book = Workbook::new_empty().unwrap();
        let before = book.cell(SHEET, at("B3"));
        book.set_input(SHEET, at("B3"), date).unwrap();
        assert_ne!(
            book.cell(SHEET, at("B3")).style.num_fmt,
            "general",
            "{date}"
        );

        book.undo().unwrap();
        assert_eq!(book.cell(SHEET, at("B3")), before, "{date}");

        book.set_input(SHEET, at("B3"), "7").unwrap();
        assert_eq!(book.cell(SHEET, at("B3")).text, "7", "{date}");
    }
}

#[test]
fn one_undo_takes_back_a_date_and_its_column_width_written_as_one_step() {
    let column = at("B3").col;
    let mut control = Workbook::new_empty().unwrap();
    control.set_column_width(SHEET, column, 90.0).unwrap();
    control.undo().unwrap();

    let mut book = Workbook::new_empty().unwrap();
    let before = book.cell(SHEET, at("B3"));
    book.as_one_undo_step(|book| {
        book.set_input(SHEET, at("B3"), "1/2/2024")?;
        book.set_column_width(SHEET, column, 90.0)
    })
    .unwrap();
    let written = book.sizes(SHEET);

    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, at("B3")), before);
    assert_eq!(book.sizes(SHEET), control.sizes(SHEET));

    book.redo().unwrap();
    assert_eq!(book.cell(SHEET, at("B3")).text, "1/2/2024");
    assert_eq!(book.sizes(SHEET), written);
}

#[test]
fn a_failed_one_step_edit_keeps_what_it_wrote_as_one_step() {
    let mut book = Workbook::new_empty().unwrap();
    let failed = book.as_one_undo_step(|book| {
        book.set_input(SHEET, at("A1"), "1")?;
        book.set_input(SHEET, at("A2"), "2")?;
        book.set_number_format(SHEET, Range::single(at("A1")), &"0".repeat(300))
    });
    assert!(failed.is_err());

    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, at("A1")).text, "");
    assert_eq!(book.cell(SHEET, at("A2")).text, "");

    book.set_input(SHEET, at("A3"), "3").unwrap();
    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, at("A3")).text, "");
}
