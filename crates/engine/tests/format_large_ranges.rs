#![allow(clippy::unwrap_used)]

use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, StyleChange};

const SHEET: SheetId = SheetId(0);

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn bold_at(book: &Workbook, row: i64, col: i64) -> bool {
    book.cell(SHEET, pos(row, col)).style.bold
}

fn book_with_a_small_data_area() -> Workbook {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(0, 0), "1").unwrap();
    book.set_input(SHEET, pos(2, 2), "2").unwrap();
    book
}

#[test]
fn a_range_past_the_cell_limit_is_formatted_over_the_used_area_in_one_undo_step() {
    let mut book = book_with_a_small_data_area();
    let range = Range::parse_a1("A1:Z50000").unwrap();
    book.apply_style(SHEET, range, StyleChange::Bold(true))
        .unwrap();
    assert!(bold_at(&book, 0, 0));
    assert!(bold_at(&book, 1, 1));
    assert!(bold_at(&book, 2, 2));
    assert!(!bold_at(&book, 0, 3));
    assert!(!bold_at(&book, 3, 0));
    book.undo().unwrap();
    assert!(!bold_at(&book, 0, 0));
    assert!(!bold_at(&book, 2, 2));
}

#[test]
fn a_number_format_and_clear_formats_follow_the_same_policy() {
    let mut book = book_with_a_small_data_area();
    let range = Range::parse_a1("A1:Z50000").unwrap();
    book.set_number_format(SHEET, range, "0.00").unwrap();
    assert_eq!(book.cell(SHEET, pos(0, 0)).text, "1.00");
    book.clear_formats(SHEET, range).unwrap();
    assert_eq!(book.cell(SHEET, pos(0, 0)).text, "1");
}

#[test]
fn a_range_inside_the_limit_keeps_its_format_for_cells_typed_later() {
    let mut book = book_with_a_small_data_area();
    let range = Range::parse_a1("A1:J1000").unwrap();
    book.apply_style(SHEET, range, StyleChange::Bold(true))
        .unwrap();
    book.set_input(SHEET, pos(500, 5), "x").unwrap();
    assert!(bold_at(&book, 500, 5));
}

#[test]
fn a_big_range_wholly_beyond_the_data_is_refused() {
    let mut book = book_with_a_small_data_area();
    let range = Range::parse_a1("K10:Z100000").unwrap();
    let error = book
        .apply_style(SHEET, range, StyleChange::Bold(true))
        .unwrap_err();
    assert!(error.to_string().contains("empty cells"), "{error}");
}

#[test]
fn a_used_area_past_the_styling_cap_is_refused() {
    let mut book = book_with_a_small_data_area();
    book.set_input(SHEET, pos(999_999, 10), "far").unwrap();
    let range = Range::parse_a1("A1:Z1000000").unwrap();
    let error = book
        .apply_style(SHEET, range, StyleChange::Bold(true))
        .unwrap_err();
    assert!(error.to_string().contains("select whole rows"), "{error}");
    assert!(!bold_at(&book, 0, 0));
}
