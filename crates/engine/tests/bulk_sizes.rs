// Spec: sizes set on many lines at once are set in one pass and undo in one step, so a
// selection of hundreds of thousands of lines does not take forever; a sheet default is one
// undo step with the lines it resets.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::io::Read;
use std::path::Path;

use zenkai_engine::{Engine, SaveTarget, Workbook, save_xlsx};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, StyleChange};

const SHEET: SheetId = SheetId(0);

fn sheet_xml(book: &Workbook, path: &Path) -> String {
    save_xlsx(book.save_snapshot(path, SaveTarget::Document).unwrap()).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn row(index: i64) -> RowIdx {
    RowIdx::clamped(index)
}

fn column(index: i64) -> ColIdx {
    ColIdx::clamped(index)
}

fn whole_column(index: i64) -> Range {
    Range::new(
        CellPos::new(row(0), column(index)),
        CellPos::new(RowIdx::LAST, column(index)),
    )
}

fn heights(book: &Workbook) -> Vec<(u32, f32)> {
    let mut heights: Vec<_> = book
        .sizes(SHEET)
        .rows
        .iter()
        .map(|(row, height)| (row.get(), *height))
        .collect();
    heights.sort_by_key(|(row, _)| *row);
    heights
}

// A row with an entry of its own that no size was set on, such as the file's automatic ones.
fn give_an_automatic_entry(book: &mut Workbook, index: i64) {
    let range = Range::single(CellPos::new(row(index), column(0)));
    book.set_rows_hidden(SHEET, range, true).unwrap();
    book.set_rows_hidden(SHEET, range, false).unwrap();
}

#[test]
fn a_sheet_row_height_and_the_automatic_rows_it_resets_are_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    for index in [2, 5, 9] {
        give_an_automatic_entry(&mut book, index);
    }
    book.set_row_height(SHEET, row(0), 60.0).unwrap();
    let before = book.sizes(SHEET);
    book.set_default_row_height(SHEET, 27.0).unwrap();
    assert_eq!(heights(&book), [(0, 60.0), (2, 27.0), (5, 27.0), (9, 27.0)]);
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET), before);
    book.redo().unwrap();
    assert_eq!(book.sizes(SHEET).default_row_height, Some(27.0));
    assert_eq!(heights(&book).len(), 4);
}

#[test]
fn a_sheet_column_width_resets_the_styled_columns_that_drew_at_the_default_and_keeps_their_style() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    book.apply_style(SHEET, whole_column(2), StyleChange::Bold(true))
        .unwrap();
    let before = book.sizes(SHEET);
    book.set_default_column_width(SHEET, 145.0).unwrap();
    let sizes = book.sizes(SHEET);
    assert_eq!(sizes.default_column_width, Some(145.0));
    assert_eq!(sizes.columns.len(), 1);
    assert_eq!(sizes.columns[0].width, 145.0);
    let xml = sheet_xml(&book, &dir.path().join("styled.xlsx"));
    let entry = xml
        .split("<col ")
        .find(|tag| tag.starts_with(r#"min="3" max="3""#))
        .unwrap();
    assert!(entry.contains(r#"width="20.7109375""#), "{entry}");
    assert!(entry.contains(" style=\""), "{entry}");
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET), before);
}

#[test]
fn many_row_heights_are_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    let before = book.sizes(SHEET);
    let list: Vec<(RowIdx, f32)> = (0..3000).map(|index| (row(index), 30.0)).collect();
    book.set_row_heights(SHEET, &list).unwrap();
    assert_eq!(heights(&book).len(), 3000);
    assert!(heights(&book).iter().all(|(_, height)| *height == 30.0));
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET), before);
}

#[test]
fn hiding_a_hundred_thousand_rows_is_one_pass_and_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    let range = Range::new(
        CellPos::new(row(1), column(0)),
        CellPos::new(row(100_000), column(0)),
    );
    book.set_rows_hidden(SHEET, range, true).unwrap();
    assert_eq!(heights(&book).len(), 100_000);
    book.set_rows_hidden(SHEET, range, false).unwrap();
    assert!(
        book.sizes(SHEET)
            .rows
            .iter()
            .all(|(_, height)| *height > 0.0)
    );
    book.undo().unwrap();
    assert_eq!(heights(&book).len(), 100_000);
    book.undo().unwrap();
    assert!(heights(&book).is_empty());
}

#[test]
fn runs_of_columns_take_their_widths_in_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_column_width(SHEET, column(2), 100.0).unwrap();
    let before = book.sizes(SHEET);
    book.set_column_widths(
        SHEET,
        &[(column(1), column(3), 145.0), (column(5), column(5), 200.0)],
    )
    .unwrap();
    let sizes = book.sizes(SHEET);
    let width_at = |index: u16| {
        sizes
            .columns
            .iter()
            .find(|span| span.first.get() <= index && index <= span.last.get())
            .map(|span| span.width)
    };
    assert_eq!(width_at(1), Some(145.0));
    assert_eq!(width_at(2), Some(145.0));
    assert_eq!(width_at(3), Some(145.0));
    assert_eq!(width_at(4), None);
    assert_eq!(width_at(5), Some(200.0));
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET), before);
}

#[test]
fn a_hidden_column_keeps_being_hidden_when_its_run_is_sized() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_columns_hidden(SHEET, whole_column(2), true)
        .unwrap();
    book.set_column_widths(SHEET, &[(column(1), column(3), 145.0)])
        .unwrap();
    let sizes = book.sizes(SHEET);
    let hidden = sizes
        .columns
        .iter()
        .find(|span| span.first.get() <= 2 && 2 <= span.last.get())
        .unwrap();
    assert_eq!(hidden.width, 0.0);
}

#[test]
fn one_line_is_measured_without_reading_the_whole_sheet() {
    let mut book = Workbook::new_empty().unwrap();
    let default_row = book.row_size(SHEET, row(3));
    assert!(!default_row.own);
    assert_eq!(default_row.pixels, 20.0);
    book.set_row_height(SHEET, row(3), 40.0).unwrap();
    let own = book.row_size(SHEET, row(3));
    assert!(own.own);
    assert_eq!(own.pixels, 40.0);
    book.set_default_column_width(SHEET, 145.0).unwrap();
    let default_column = book.column_size(SHEET, column(7));
    assert!(!default_column.own);
    assert_eq!(default_column.pixels, 145.0);
    book.set_column_width(SHEET, column(7), 90.0).unwrap();
    let own = book.column_size(SHEET, column(7));
    assert!(own.own);
    assert_eq!(own.pixels, 90.0);
}

#[test]
fn only_the_filled_cells_of_the_range_are_read_and_the_walk_stops_at_the_limit() {
    let mut book = Workbook::new_empty().unwrap();
    for (text, at) in [
        ("1", "A1"),
        ("2", "B1"),
        ("3", "A2"),
        ("4", "C3"),
        ("5", "A9"),
    ] {
        book.set_input(SHEET, CellPos::parse_a1(at).unwrap(), text)
            .unwrap();
    }
    let range = Range::parse_a1("A1:B3").unwrap();
    let named =
        |cells: Vec<CellPos>| -> Vec<String> { cells.iter().map(ToString::to_string).collect() };
    assert_eq!(
        named(book.filled_cells_within(SHEET, range, 100)),
        ["A1", "B1", "A2"]
    );
    assert_eq!(
        named(book.filled_cells_within(SHEET, range, 2)),
        ["A1", "B1"]
    );
    let column = Range::parse_a1("A1:A1048576").unwrap();
    assert_eq!(
        named(book.filled_cells_within(SHEET, column, 100)),
        ["A1", "A2", "A9"]
    );
}
