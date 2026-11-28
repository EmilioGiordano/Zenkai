// Spec: a column width is stored as Excel stores it, with the padding of the cell in it
// (20 characters are 20.7109375 and 145 pixels), so a file's widths draw at Excel's pixels,
// the dialog shows Excel's characters, and a width set here is saved as Excel would.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::io::Read;
use std::path::{Path, PathBuf};

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, SaveTarget, Workbook, XlsxReader, open_xlsx_with, save_xlsx};
use zenkai_types::{ColIdx, DEFAULT_COL_WIDTH, SheetId};

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

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let (_, rest) = tag.split_once(&format!(" {name}=\""))?;
    rest.split_once('"').map(|(value, _)| value)
}

// Every <col> as (min, max, width) text, in file order.
fn columns(xml: &str) -> Vec<(String, String, String)> {
    xml.split("<col ")
        .skip(1)
        .map(|tag| {
            let tag = format!(" {}", tag.split_once('>').unwrap().0);
            let get = |name| attribute(&tag, name).unwrap().to_string();
            (get("min"), get("max"), get("width"))
        })
        .collect()
}

fn source(dir: &Path) -> PathBuf {
    let path = dir.join("source.xlsx");
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 1.0).unwrap();
    sheet.set_column_width(1, 20.0).unwrap();
    sheet.set_column_width(3, 12.0).unwrap();
    sheet.set_column_width(4, 40.0).unwrap();
    book.save(&path).unwrap();
    path
}

fn width_of(book: &Workbook, col: u16) -> Option<f32> {
    let col = ColIdx::clamped(i64::from(col));
    book.sizes(SHEET)
        .columns
        .iter()
        .find(|span| span.first <= col && col <= span.last)
        .map(|span| span.width)
}

#[test]
fn a_width_of_20_characters_in_a_file_draws_145_pixels_wide_in_both_readers() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    assert!(sheet_xml(&path).contains(r#"width="20.7109375""#));
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let book = open_xlsx_with(&path, reader).unwrap().workbook;
        assert_eq!(width_of(&book, 1), Some(145.0), "{reader:?}");
        assert_eq!(width_of(&book, 3), Some(89.0), "{reader:?}");
        assert_eq!(width_of(&book, 4), Some(285.0), "{reader:?}");
    }
}

#[test]
fn both_readers_agree_on_every_width() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    let fast = open_xlsx_with(&path, XlsxReader::Fast).unwrap().workbook;
    let ironcalc = open_xlsx_with(&path, XlsxReader::IronCalc)
        .unwrap()
        .workbook;
    assert_eq!(fast.sizes(SHEET), ironcalc.sizes(SHEET));
}

#[test]
fn a_width_set_here_is_saved_as_excel_stores_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    book.set_column_width(SHEET, ColIdx::clamped(2), 145.0)
        .unwrap();
    book.set_column_width(SHEET, ColIdx::clamped(3), 64.0)
        .unwrap();
    let xml = saved(&book, &dir.path().join("sized.xlsx"));
    let found = columns(&xml);
    assert!(
        found.contains(&("3".into(), "3".into(), "20.7109375".into())),
        "{found:?}"
    );
    assert!(
        found.contains(&("4".into(), "4".into(), "9.140625".into())),
        "{found:?}"
    );
}

#[test]
fn a_default_column_is_64_pixels_and_a_sheet_default_is_saved_as_stored() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    assert_eq!(width_of(&book, 0), None);
    assert_eq!(DEFAULT_COL_WIDTH, 64.0);
    book.set_default_column_width(SHEET, 145.0).unwrap();
    assert_eq!(book.sizes(SHEET).default_column_width, Some(145.0));
    let xml = saved(&book, &dir.path().join("default.xlsx"));
    assert!(xml.contains(r#" defaultColWidth="20.7109375""#), "{xml}");
}

#[test]
fn a_file_saved_without_edits_keeps_every_col_width_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    let before = columns(&sheet_xml(&path));
    assert_eq!(before.len(), 3);
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let book = open_xlsx_with(&path, reader).unwrap().workbook;
        let after = columns(&saved(&book, &dir.path().join("again.xlsx")));
        assert_eq!(after, before, "{reader:?}");
    }
}

#[test]
fn a_width_edited_in_an_opened_file_saves_as_excel_would_and_leaves_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path());
    let mut book = open_xlsx_with(&path, XlsxReader::Fast).unwrap().workbook;
    book.set_column_width(SHEET, ColIdx::clamped(3), 145.0)
        .unwrap();
    let after = columns(&saved(&book, &dir.path().join("edited.xlsx")));
    let mut expected = columns(&sheet_xml(&path));
    expected[1].2 = "20.7109375".to_string();
    assert_eq!(after, expected);
}
