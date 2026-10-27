use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, ValueKind};

const SHEET: SheetId = SheetId(0);

fn pos(row: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(0))
}

fn column(cells: &[&str]) -> Vec<Vec<String>> {
    cells.iter().map(|cell| vec![cell.to_string()]).collect()
}

#[test]
fn text_starting_with_a_hash_is_text_and_real_errors_are_errors() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(
        SHEET,
        pos(0),
        &column(&["#tag", "=\"#x\"", "=\"#N/A\"", "=1/0", "=NA()", "#DIV/0!"]),
    )
    .unwrap();
    let kinds: Vec<ValueKind> = (0..6).map(|row| book.cell(SHEET, pos(row)).kind).collect();
    assert_eq!(
        kinds,
        [
            ValueKind::Text,
            ValueKind::Text,
            ValueKind::Text,
            ValueKind::Error,
            ValueKind::Error,
            ValueKind::Error,
        ]
    );
}

#[test]
fn sort_puts_hash_text_with_text_before_booleans_and_errors() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(
        SHEET,
        pos(0),
        &column(&["=1/0", "TRUE", "#tag", "=NA()", "apple", "=\"#x\""]),
    )
    .unwrap();
    book.sort(SHEET, Range::new(pos(0), pos(5)), ColIdx::clamped(0), false)
        .unwrap();
    let inputs: Vec<String> = (0..6).map(|row| book.input(SHEET, pos(row))).collect();
    assert_eq!(
        inputs,
        ["#tag", "=\"#x\"", "apple", "TRUE", "=1/0", "=NA()"]
    );
}
