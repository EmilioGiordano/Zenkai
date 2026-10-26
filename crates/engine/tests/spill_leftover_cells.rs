// The cells a shrinking dynamic array leaves are gone from the used range and the saved
// file, as in Excel.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read};

use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, Range, SheetId};

const SHEET: SheetId = SheetId(0);

fn at(text: &str) -> CellPos {
    CellPos::parse_a1(text).unwrap()
}

fn shrunk_sequence() -> Workbook {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, at("A1"), "10").unwrap();
    book.set_input(SHEET, at("B1"), "=SEQUENCE(A1)").unwrap();
    assert_eq!(book.used_end(SHEET), at("B10"));
    book.set_input(SHEET, at("A1"), "3").unwrap();
    book
}

fn sheet_xml(xlsx: &[u8]) -> String {
    let mut archive = zip::ZipArchive::new(Cursor::new(xlsx)).unwrap();
    let mut xml = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

#[test]
fn a_shrunk_spill_ends_the_used_range_at_its_last_cell() {
    let book = shrunk_sequence();
    assert_eq!(book.cell(SHEET, at("B3")).text, "3");
    assert_eq!(book.used_end(SHEET), at("B3"));
}

#[test]
fn a_shrunk_spill_saves_without_the_cells_it_left() {
    let xlsx = shrunk_sequence().to_xlsx().unwrap();
    let xml = sheet_xml(&xlsx);
    assert!(xml.contains(r#"<dimension ref="A1:B3"/>"#), "{xml}");
    for row in 4..=10 {
        assert!(!xml.contains(&format!(r#"r="B{row}""#)), "B{row} in {xml}");
    }
    let reopened = Workbook::from_xlsx_bytes(&xlsx, "shrunk").unwrap();
    assert_eq!(reopened.used_end(SHEET), at("B3"));
}

#[test]
fn undo_and_redo_of_the_shrink_move_the_used_end() {
    let mut book = shrunk_sequence();
    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, at("B10")).text, "10");
    assert_eq!(book.used_end(SHEET), at("B10"));
    book.redo().unwrap();
    assert_eq!(book.cell(SHEET, at("B4")).text, "");
    assert_eq!(book.used_end(SHEET), at("B3"));
}

#[test]
fn a_blocked_spill_that_spills_again_shrinks_the_used_range() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, at("A1"), "10").unwrap();
    book.set_input(SHEET, at("B1"), "=SEQUENCE(A1)").unwrap();
    book.set_input(SHEET, at("B5"), "x").unwrap();
    assert_eq!(book.cell(SHEET, at("B1")).text, "#SPILL!");
    assert_eq!(book.used_end(SHEET), at("B5"));
    book.clear(SHEET, Range::new(at("B5"), at("B5"))).unwrap();
    assert_eq!(book.cell(SHEET, at("B10")).text, "10");
    book.set_input(SHEET, at("A1"), "3").unwrap();
    assert_eq!(book.used_end(SHEET), at("B3"));
}

#[test]
fn deleting_a_whole_spill_leaves_only_its_anchor_cleared() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, at("B1"), "=SEQUENCE(10)").unwrap();
    book.clear(SHEET, Range::new(at("B1"), at("B10"))).unwrap();
    assert_eq!(book.cell(SHEET, at("B10")).text, "");
    assert_eq!(book.used_end(SHEET), at("B1"));
    book.undo().unwrap();
    assert_eq!(book.cell(SHEET, at("B10")).text, "10");
    assert_eq!(book.used_end(SHEET), at("B10"));
}
