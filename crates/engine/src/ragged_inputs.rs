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
    assert_eq!(input(&book, "A2001"), "1999");
    assert_eq!(input(&book, "B2001"), "");
}

#[test]
fn ragged_rows_inside_an_edit_group_stay_one_undo_step() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, CellPos::default(), &[row(&["before"])])
        .unwrap();
    let origin = CellPos::parse_a1("C1").unwrap();
    book.as_one_undo_step(|book| {
        book.set_inputs(SHEET, origin, &[row(&["a", "b"]), row(&["c"])])?;
        book.set_inputs(SHEET, origin, &[row(&["d"]), row(&["e", "f"])])
    })
    .unwrap();
    assert_eq!(input(&book, "D2"), "f");
    book.undo().unwrap();
    assert_eq!(input(&book, "A1"), "before");
    assert_eq!(book.filled_cells(SHEET).len(), 1);
}

fn book_with_an_array_formula() -> Workbook {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("array.xlsx");
    let mut source = rust_xlsxwriter::Workbook::new();
    source
        .add_worksheet()
        .write_array_formula(0, 1, 1, 1, "{=A1:A2*2}")
        .unwrap();
    source.save(&path).unwrap();
    crate::file::open_xlsx(&path).unwrap().workbook
}

fn straddling_block() -> Vec<Vec<String>> {
    vec![row(&["x"]), row(&["y", "z"])]
}

#[test]
fn a_rejected_ragged_block_changes_nothing() {
    let mut book = book_with_an_array_formula();
    let before = book.filled_cells(SHEET);
    let result = book.set_inputs(SHEET, CellPos::default(), &straddling_block());
    assert!(result.is_err());
    assert_eq!(book.filled_cells(SHEET), before);
    assert_eq!(input(&book, "A1"), "");
    assert_eq!(input(&book, "A2"), "");
}

#[test]
fn a_rejected_ragged_block_keeps_the_redo_history() {
    let mut book = book_with_an_array_formula();
    book.set_input(SHEET, CellPos::parse_a1("D4").unwrap(), "7")
        .unwrap();
    book.undo().unwrap();
    assert!(
        book.set_inputs(SHEET, CellPos::default(), &straddling_block())
            .is_err()
    );
    book.redo().unwrap();
    assert_eq!(input(&book, "D4"), "7");
}

#[test]
fn a_rejected_ragged_block_inside_an_edit_group_is_not_half_written() {
    let mut book = book_with_an_array_formula();
    let before = book.filled_cells(SHEET);
    let grouped = book.as_one_undo_step(|book| {
        book.set_input(SHEET, CellPos::parse_a1("D4").unwrap(), "7")?;
        book.set_inputs(SHEET, CellPos::default(), &straddling_block())
    });
    assert!(grouped.is_err());
    assert_eq!(input(&book, "A1"), "");
    assert_eq!(input(&book, "A2"), "");
    assert_eq!(input(&book, "D4"), "7");
    book.undo().unwrap();
    assert_eq!(book.filled_cells(SHEET), before);
}

#[test]
fn short_rows_clear_the_cells_to_their_right() {
    let mut book = Workbook::new_empty().unwrap();
    let filled: Vec<Vec<String>> = (0..4)
        .map(|r| (0..4).map(|c| format!("r{r}c{c}")).collect())
        .collect();
    book.set_inputs(SHEET, CellPos::default(), &filled).unwrap();
    let before = book.filled_cells(SHEET);

    let ragged = vec![
        row(&["a", "b", "c"]),
        row(&["d"]),
        Vec::new(),
        row(&["e", "f"]),
    ];
    book.set_inputs(SHEET, CellPos::default(), &ragged).unwrap();
    for cleared in ["B2", "C2", "A3", "B3", "C3", "C4"] {
        assert_eq!(input(&book, cleared), "", "{cleared}");
    }
    assert_eq!(input(&book, "A2"), "d");
    assert_eq!(input(&book, "B4"), "f");
    assert_eq!(input(&book, "D1"), "r0c3");
    assert_eq!(input(&book, "A4"), "e");

    book.undo().unwrap();
    assert_eq!(book.filled_cells(SHEET), before);
    assert_eq!(input(&book, "C3"), "r2c2");
    assert_eq!(input(&book, "B2"), "r1c1");
}

#[test]
fn alternating_widths_store_only_the_fields_in_the_file() {
    const WIDTH: usize = 500;
    const PAIRS: usize = 2_000;
    let mut rows = Vec::new();
    for n in 0..PAIRS {
        rows.push(vec!["w".to_string(); WIDTH]);
        rows.push(vec![n.to_string()]);
    }
    let fields: usize = rows.iter().map(Vec::len).sum();

    let started = std::time::Instant::now();
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, CellPos::default(), &rows).unwrap();

    assert!(started.elapsed() < std::time::Duration::from_secs(60));
    assert_eq!(stored_cells(&book), fields);
    assert_eq!(input(&book, "A4000"), "1999");
}
