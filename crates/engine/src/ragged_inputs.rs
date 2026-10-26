// Rows of different lengths are written field by field: a short row leaves the cells past
// its end alone, so the work of an import follows the fields in the file, not the area
// its widest row spans.
#![allow(clippy::unwrap_used)]

use zenkai_types::{CellPos, SheetId};

use crate::workbook::{Engine, Workbook};

const SHEET: SheetId = SheetId(0);

fn stored_cells(book: &Workbook) -> usize {
    let sheet_data = &book
        .model
        .get_model()
        .workbook
        .worksheet(SHEET.0)
        .unwrap()
        .sheet_data;
    sheet_data.rows().map(|row| sheet_data.row_len(row)).sum()
}

fn row(fields: &[&str]) -> Vec<String> {
    fields.iter().map(ToString::to_string).collect()
}

fn input(book: &Workbook, a1: &str) -> String {
    book.input(SHEET, CellPos::parse_a1(a1).unwrap())
}

#[test]
fn ragged_rows_round_trip() {
    let mut book = Workbook::new_empty().unwrap();
    let rows = vec![
        row(&["a", "b", "c", "d"]),
        row(&["e"]),
        row(&["", "f", ""]),
        row(&["g", "h"]),
    ];
    book.set_inputs(SHEET, CellPos::default(), &rows).unwrap();
    for (r, fields) in rows.iter().enumerate() {
        for (c, text) in fields.iter().enumerate() {
            let a1 = CellPos::new(
                zenkai_types::RowIdx::new(r as u32).unwrap(),
                zenkai_types::ColIdx::new(c as u16).unwrap(),
            );
            assert_eq!(book.input(SHEET, a1), *text, "{a1}");
        }
    }
    for empty in ["B2", "C2", "D2", "D3", "C4", "D4"] {
        assert_eq!(input(&book, empty), "", "{empty}");
    }
    let filled: Vec<String> = book
        .filled_cells(SHEET)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(filled, ["A1", "B1", "C1", "D1", "A2", "B3", "A4", "B4"]);
    assert_eq!(book.used_end(SHEET).to_string(), "D4");

    book.undo().unwrap();
    assert!(book.filled_cells(SHEET).is_empty());
    book.redo().unwrap();
    assert_eq!(input(&book, "B3"), "f");
    assert_eq!(input(&book, "D1"), "d");
}

#[test]
fn a_wide_first_row_does_not_widen_the_rows_after_it() {
    const WIDTH: usize = 1_000;
    const LINES: usize = 2_000;
    let mut rows = vec![vec!["x".to_string(); WIDTH]];
    rows.extend((0..LINES).map(|n| vec![n.to_string()]));
    let fields: usize = rows.iter().map(Vec::len).sum();

    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, CellPos::default(), &rows).unwrap();

    assert_eq!(stored_cells(&book), fields);
    assert_eq!(input(&book, "A2002"), "1999");
    assert_eq!(input(&book, "B2002"), "");
}
