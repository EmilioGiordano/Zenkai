// Spec: undo restores a column exactly. Undoing a width set on a column without one leaves
// no column entry behind, so the column draws and saves at the default width again.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::io::Read;
use std::path::{Path, PathBuf};

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId, SheetSizes};

const SHEET: SheetId = SheetId(0);

fn column(index: i64) -> ColIdx {
    ColIdx::clamped(index)
}

fn saved_columns(book: &Workbook, path: &Path) -> Option<String> {
    let snapshot = book.save_snapshot(path, SaveTarget::Document).unwrap();
    save_xlsx(snapshot).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    let start = text.find("<cols>")?;
    let end = text.find("</cols>").unwrap();
    Some(text[start..end].to_string())
}

fn source(dir: &Path) -> PathBuf {
    let path = dir.join("source.xlsx");
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 1.0).unwrap();
    sheet.write_number(0, 1, 2.0).unwrap();
    sheet.set_column_width(2, 20.0).unwrap();
    book.save(&path).unwrap();
    path
}

struct Before {
    sizes: SheetSizes,
    columns: Option<String>,
}

fn before(book: &Workbook, dir: &Path) -> Before {
    Before {
        sizes: book.sizes(SHEET),
        columns: saved_columns(book, &dir.join("before.xlsx")),
    }
}

fn assert_restored(book: &Workbook, dir: &Path, before: &Before) {
    assert_eq!(book.sizes(SHEET), before.sizes);
    assert_eq!(
        saved_columns(book, &dir.join("undone.xlsx")),
        before.columns
    );
}

#[test]
fn undoing_a_new_width_leaves_no_column_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    let before = before(&book, dir.path());
    assert!(before.sizes.columns.is_empty());
    book.set_column_width(SHEET, column(1), 140.0).unwrap();
    let widened = book.sizes(SHEET);
    book.undo().unwrap();
    assert_restored(&book, dir.path(), &before);
    book.redo().unwrap();
    assert_eq!(book.sizes(SHEET), widened);
}

#[test]
fn undoing_a_width_in_an_opened_file_saves_the_columns_it_had() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = open_xlsx(&source(dir.path())).unwrap().workbook;
    let before = before(&book, dir.path());
    assert!(before.columns.is_some());
    book.set_column_width(SHEET, column(1), 140.0).unwrap();
    book.set_column_width(SHEET, column(2), 30.0).unwrap();
    book.undo().unwrap();
    book.undo().unwrap();
    assert_restored(&book, dir.path(), &before);
}

#[test]
fn undoing_a_date_that_widened_its_column_restores_the_column() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    let before = before(&book, dir.path());
    let at = CellPos::new(RowIdx::clamped(0), column(1));
    book.as_one_undo_step(|book| {
        book.set_input(SHEET, at, "12/25/2026 10:30")?;
        book.set_column_width(SHEET, at.col, 120.0)
    })
    .unwrap();
    book.undo().unwrap();
    assert_restored(&book, dir.path(), &before);
    assert!(book.input(SHEET, at).is_empty());
}
