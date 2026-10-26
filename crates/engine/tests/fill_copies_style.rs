#![allow(clippy::unwrap_used)]

use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, Rgb, RowIdx, SheetId, StyleChange};

const SHEET: SheetId = SheetId(0);

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn column(first: i64, last: i64, col: i64) -> Range {
    Range::new(pos(first, col), pos(last, col))
}

#[test]
fn fill_down_copies_the_number_format_of_a_typed_date() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(0, 1), "3/15/2024").unwrap();
    let source = book.cell(SHEET, pos(0, 1));
    assert_ne!(source.style.num_fmt, "general");
    book.fill(SHEET, column(0, 4, 1), true).unwrap();
    for row in 1..5 {
        let cell = book.cell(SHEET, pos(row, 1));
        assert_eq!(cell.text, source.text, "row {row}");
        assert_eq!(cell.style.num_fmt, source.style.num_fmt, "row {row}");
    }
}

#[test]
fn fill_down_copies_bold_and_fill_of_the_source() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(0, 0), "7").unwrap();
    let red = Rgb(0x00FF_0000);
    book.apply_style(SHEET, column(0, 0, 0), StyleChange::Bold(true))
        .unwrap();
    book.apply_style(SHEET, column(0, 0, 0), StyleChange::Fill(Some(red)))
        .unwrap();
    book.fill(SHEET, column(0, 3, 0), true).unwrap();
    for row in 1..4 {
        let style = book.cell(SHEET, pos(row, 0)).style;
        assert!(style.bold, "row {row}");
        assert_eq!(style.fill, Some(red), "row {row}");
    }
}

#[test]
fn fill_down_of_an_empty_styled_source_copies_its_style() {
    let mut book = Workbook::new_empty().unwrap();
    book.apply_style(SHEET, column(0, 0, 0), StyleChange::Bold(true))
        .unwrap();
    book.fill(SHEET, column(0, 2, 0), true).unwrap();
    assert!(book.cell(SHEET, pos(2, 0)).style.bold);
}

#[test]
fn fill_right_copies_the_style_of_the_first_column() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(0, 0), "3/15/2024").unwrap();
    book.set_input(SHEET, pos(1, 0), "1/2/2020").unwrap();
    book.apply_style(SHEET, column(0, 1, 0), StyleChange::Italic(true))
        .unwrap();
    book.fill(SHEET, Range::new(pos(0, 0), pos(1, 3)), false)
        .unwrap();
    for row in 0..2 {
        let source = book.cell(SHEET, pos(row, 0));
        for col in 1..4 {
            let cell = book.cell(SHEET, pos(row, col));
            assert!(cell.style.italic, "{row},{col}");
            assert_eq!(cell.style.num_fmt, source.style.num_fmt, "{row},{col}");
            assert_eq!(cell.text, source.text, "{row},{col}");
        }
    }
}

#[test]
fn fill_down_copies_each_column_style_of_several_columns() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(0, 0), "3/15/2024").unwrap();
    book.set_input(SHEET, pos(0, 1), "5").unwrap();
    book.apply_style(SHEET, column(0, 0, 1), StyleChange::Bold(true))
        .unwrap();
    let date_format = book.cell(SHEET, pos(0, 0)).style.num_fmt;
    book.fill(SHEET, Range::new(pos(0, 0), pos(3, 1)), true)
        .unwrap();
    for row in 1..4 {
        let first = book.cell(SHEET, pos(row, 0));
        assert_eq!(first.style.num_fmt, date_format, "row {row}");
        assert!(!first.style.bold, "row {row}");
        assert!(book.cell(SHEET, pos(row, 1)).style.bold, "row {row}");
    }
}

#[test]
fn undo_restores_the_old_styles_of_the_targets_and_redo_fills_again() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, pos(0, 0), "3/15/2024").unwrap();
    book.set_input(SHEET, pos(1, 0), "9").unwrap();
    book.apply_style(SHEET, column(1, 1, 0), StyleChange::Italic(true))
        .unwrap();
    let date_format = book.cell(SHEET, pos(0, 0)).style.num_fmt;
    let old = book.cell(SHEET, pos(1, 0));
    book.fill(SHEET, column(0, 3, 0), true).unwrap();
    assert_eq!(book.cell(SHEET, pos(1, 0)).style.num_fmt, date_format);
    assert!(!book.cell(SHEET, pos(1, 0)).style.italic);

    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, pos(1, 0)), old);
    assert_eq!(book.cell(SHEET, pos(3, 0)), book.cell(SHEET, pos(30, 0)));
    book.redo().unwrap();
    assert_eq!(book.cell(SHEET, pos(1, 0)).style.num_fmt, date_format);
    assert_eq!(book.cell(SHEET, pos(3, 0)).style.num_fmt, date_format);
}
