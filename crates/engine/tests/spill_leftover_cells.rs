// The cells a shrinking dynamic array leaves are gone from the used range and the saved
// file, as in Excel.
#![allow(clippy::unwrap_used)]

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

#[test]
fn a_shrunk_spill_ends_the_used_range_at_its_last_cell() {
    let book = shrunk_sequence();
    assert_eq!(book.cell(SHEET, at("B3")).text, "3");
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
