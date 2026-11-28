// Spec: a selection of every row or column sets the sheet's default size, as Excel does,
// instead of one entry for each of a million rows. The size undoes with the edits around
// it, is written to sheetFormatPr and is read back from it.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::io::Read;
use std::path::{Path, PathBuf};

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

const SHEET: SheetId = SheetId(0);

fn sheet_xml(path: &Path) -> String {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn saved(book: &Workbook, path: &Path) -> String {
    save_xlsx(book.save_snapshot(path, SaveTarget::Document).unwrap()).unwrap();
    sheet_xml(path)
}

fn format_element(xml: &str) -> &str {
    let start = xml.find("<sheetFormatPr").unwrap();
    let end = xml[start..].find("/>").unwrap() + start;
    &xml[start..end]
}

fn source(dir: &Path) -> PathBuf {
    let path = dir.join("source.xlsx");
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 1.0).unwrap();
    sheet.write_number(1, 1, 2.0).unwrap();
    sheet.set_default_row_height(18);
    book.save(&path).unwrap();
    path
}

#[test]
fn a_new_sheet_has_no_default_sizes() {
    let book = Workbook::new_empty().unwrap();
    let sizes = book.sizes(SHEET);
    assert_eq!(sizes.default_row_height, None);
    assert_eq!(sizes.default_column_width, None);
}

#[test]
fn a_default_size_is_one_setting_not_one_entry_per_line() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_default_row_height(SHEET, 30.0).unwrap();
    book.set_default_column_width(SHEET, 145.0).unwrap();
    let sizes = book.sizes(SHEET);
    assert_eq!(sizes.default_row_height, Some(30.0));
    assert_eq!(sizes.default_column_width, Some(145.0));
    assert!(sizes.rows.is_empty());
    assert!(sizes.columns.is_empty());
}

#[test]
fn a_default_size_undoes_and_redoes() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_default_row_height(SHEET, 30.0).unwrap();
    book.set_default_column_width(SHEET, 145.0).unwrap();
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET).default_column_width, None);
    assert_eq!(book.sizes(SHEET).default_row_height, Some(30.0));
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET).default_row_height, None);
    book.redo().unwrap();
    book.redo().unwrap();
    let sizes = book.sizes(SHEET);
    assert_eq!(sizes.default_row_height, Some(30.0));
    assert_eq!(sizes.default_column_width, Some(145.0));
}

#[test]
fn a_default_size_and_the_entries_it_replaces_are_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    let row = RowIdx::clamped(2);
    book.set_row_height(SHEET, row, 60.0).unwrap();
    let before = book.sizes(SHEET);
    book.as_one_undo_step(|book| {
        book.set_default_row_height(SHEET, 30.0)?;
        book.set_row_height(SHEET, row, 30.0)
    })
    .unwrap();
    assert_eq!(book.sizes(SHEET).rows, [(row, 30.0)]);
    assert_eq!(book.sizes(SHEET).default_row_height, Some(30.0));
    book.undo().unwrap();
    assert_eq!(book.sizes(SHEET), before);
}

#[test]
fn a_default_of_zero_or_less_is_refused() {
    let mut book = Workbook::new_empty().unwrap();
    for pixels in [0.0, -4.0, f32::NAN, f32::INFINITY] {
        assert!(book.set_default_row_height(SHEET, pixels).is_err());
        assert!(book.set_default_column_width(SHEET, pixels).is_err());
    }
    assert_eq!(book.sizes(SHEET).default_row_height, None);
    assert_eq!(book.sizes(SHEET).default_column_width, None);
}

#[test]
fn the_defaults_are_written_to_the_sheet_format_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    book.set_default_row_height(SHEET, 30.0).unwrap();
    book.set_default_column_width(SHEET, 145.0).unwrap();
    let path = dir.path().join("sized.xlsx");
    let xml = saved(&book, &path);
    let format = format_element(&xml);
    assert!(format.contains(r#" defaultRowHeight="22.5""#), "{format}");
    assert!(format.contains(r#" customHeight="1""#), "{format}");
    assert!(format.contains(r#" defaultColWidth="20""#), "{format}");
    assert!(!xml.contains("<row "), "{xml}");
    assert!(!xml.contains("<col "), "{xml}");
    let reopened = open_xlsx(&path).unwrap().workbook;
    assert_eq!(reopened.sizes(SHEET), book.sizes(SHEET));
}

#[test]
fn a_default_changed_in_an_opened_file_replaces_the_one_it_had() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = open_xlsx(&source(dir.path())).unwrap().workbook;
    assert_eq!(book.sizes(SHEET).default_row_height, Some(24.0));
    book.set_default_row_height(SHEET, 40.0).unwrap();
    let path = dir.path().join("changed.xlsx");
    let xml = saved(&book, &path);
    let format = format_element(&xml);
    assert_eq!(format.matches("defaultRowHeight=").count(), 1, "{format}");
    assert!(format.contains(r#" defaultRowHeight="30""#), "{format}");
    let reopened = open_xlsx(&path).unwrap().workbook;
    assert_eq!(reopened.sizes(SHEET).default_row_height, Some(40.0));
    assert_eq!(
        reopened.input(SHEET, CellPos::new(RowIdx::clamped(1), ColIdx::clamped(1))),
        "2"
    );
}

#[test]
fn undoing_a_default_in_an_opened_file_saves_the_file_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let origin = source(dir.path());
    let mut book = open_xlsx(&origin).unwrap().workbook;
    let untouched = saved(&book, &dir.path().join("untouched.xlsx"));
    book.set_default_row_height(SHEET, 40.0).unwrap();
    book.set_default_column_width(SHEET, 90.0).unwrap();
    book.undo().unwrap();
    book.undo().unwrap();
    let undone = saved(&book, &dir.path().join("undone.xlsx"));
    assert_eq!(format_element(&undone), format_element(&untouched));
    assert_eq!(book.sizes(SHEET).default_row_height, Some(24.0));
}

#[test]
fn a_file_default_without_custom_height_is_the_applications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.xlsx");
    let mut book = Source::new();
    book.add_worksheet().write_number(0, 0, 1.0).unwrap();
    book.save(&path).unwrap();
    let opened = open_xlsx(&path).unwrap().workbook;
    assert_eq!(opened.sizes(SHEET).default_row_height, None);
    assert_eq!(opened.sizes(SHEET).default_column_width, None);
}
