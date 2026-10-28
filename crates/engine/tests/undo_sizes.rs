// Spec: undo restores a column or row exactly. Undoing a width set on a column without one leaves
// no column entry behind, so the column draws and saves at the default width again.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{
    CellPos, ColIdx, DEFAULT_COL_WIDTH, Range, RowIdx, SheetId, SheetSizes, StyleChange,
};

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

fn whole_column(index: i64) -> Range {
    Range::new(
        CellPos::new(RowIdx::clamped(0), column(index)),
        CellPos::new(RowIdx::clamped(i64::MAX), column(index)),
    )
}

#[test]
fn undoing_a_hide_on_a_column_without_an_entry_leaves_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    let before = before(&book, dir.path());
    book.set_columns_hidden(SHEET, whole_column(4), true)
        .unwrap();
    assert_eq!(book.sizes(SHEET).columns[0].width, 0.0);
    book.undo().unwrap();
    assert_restored(&book, dir.path(), &before);
}

#[test]
fn showing_a_column_hidden_without_an_entry_draws_and_saves_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    let before = before(&book, dir.path());
    book.set_columns_hidden(SHEET, whole_column(4), true)
        .unwrap();
    book.set_columns_hidden(SHEET, whole_column(4), false)
        .unwrap();
    assert_restored(&book, dir.path(), &before);
}

#[test]
fn an_entry_without_a_custom_width_draws_at_the_grid_default() {
    let mut book = Workbook::new_empty().unwrap();
    book.apply_style(SHEET, whole_column(1), StyleChange::Bold(true))
        .unwrap();
    let sizes = book.sizes(SHEET);
    assert!(
        sizes
            .columns
            .iter()
            .all(|span| span.width == DEFAULT_COL_WIDTH)
    );
}

#[test]
fn an_unhidden_styled_column_saves_and_reopens_at_the_default_width() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    book.apply_style(SHEET, whole_column(4), StyleChange::Bold(true))
        .unwrap();
    book.set_columns_hidden(SHEET, whole_column(4), true)
        .unwrap();
    book.set_columns_hidden(SHEET, whole_column(4), false)
        .unwrap();
    let path = dir.path().join("styled.xlsx");
    save_xlsx(book.save_snapshot(&path, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&path).unwrap().workbook;
    assert!(
        reopened
            .sizes(SHEET)
            .columns
            .iter()
            .all(|span| span.width == DEFAULT_COL_WIDTH)
    );
}

#[test]
fn undoing_a_row_height_leaves_no_row_entry_where_there_was_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    let before = before(&book, dir.path());
    assert!(before.sizes.rows.is_empty());
    book.set_row_height(SHEET, RowIdx::clamped(3), 40.0)
        .unwrap();
    assert_eq!(book.sizes(SHEET).rows.len(), 1);
    book.undo().unwrap();
    assert_restored(&book, dir.path(), &before);
}

fn without_custom_width(source: &Path, target: &Path) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(target).unwrap());
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if entry.name() == "xl/worksheets/sheet1.xml" {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains(" customWidth=\"1\""));
            bytes = text.replace(" customWidth=\"1\"", "").into_bytes();
        }
        writer
            .start_file(entry.name(), zip::write::FileOptions::default())
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
}

// Excel honours a column's width attribute whether or not customWidth is set.
#[test]
fn a_file_column_width_without_custom_width_draws_and_saves_as_stored() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.xlsx");
    without_custom_width(&source(dir.path()), &plain);
    let book = open_xlsx(&plain).unwrap().workbook;
    let width = |book: &Workbook| {
        book.sizes(SHEET)
            .columns
            .iter()
            .find(|span| span.first == column(2))
            .map(|span| span.width)
    };
    let opened = width(&book).unwrap();
    assert!(opened > 100.0, "{opened}");
    let saved = dir.path().join("saved.xlsx");
    save_xlsx(book.save_snapshot(&saved, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&saved).unwrap().workbook;
    assert_eq!(width(&reopened), Some(opened));
}

#[test]
fn a_width_dragged_to_ironcalcs_default_stays_that_width() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_column_width(SHEET, column(1), 75.0).unwrap();
    assert_eq!(book.sizes(SHEET).columns[0].width, 75.0);
}
