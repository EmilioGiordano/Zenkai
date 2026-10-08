use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

const SHEET: SheetId = SheetId(0);

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn rows(cells: &[&[&str]]) -> Vec<Vec<String>> {
    cells
        .iter()
        .map(|row| row.iter().map(ToString::to_string).collect())
        .collect()
}

fn row_inputs(book: &Workbook, row: i64, cols: i64) -> Vec<String> {
    (0..cols)
        .map(|col| book.input(SHEET, pos(row, col)))
        .collect()
}

fn column_inputs(book: &Workbook, col: i64, count: i64) -> Vec<String> {
    (0..count)
        .map(|row| book.input(SHEET, pos(row, col)))
        .collect()
}

#[test]
fn extend_right_continues_a_number_trend_across_a_row() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(0, 0), &rows(&[&["1", "3"]]))
        .unwrap();
    let source = Range::new(pos(0, 0), pos(0, 1));
    book.extend(SHEET, source, Range::new(pos(0, 0), pos(0, 4)))
        .unwrap();
    assert_eq!(row_inputs(&book, 0, 5), ["1", "3", "5", "7", "9"]);
}

#[test]
fn extend_right_undoes_in_one_step() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(0, 0), &rows(&[&["1", "3"]]))
        .unwrap();
    let source = Range::new(pos(0, 0), pos(0, 1));
    book.extend(SHEET, source, Range::new(pos(0, 0), pos(0, 4)))
        .unwrap();
    book.undo().unwrap();
    assert_eq!(row_inputs(&book, 0, 5), ["1", "3", "", "", ""]);
}

#[test]
fn extend_right_counts_up_numbered_text() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(0, 0), &rows(&[&["Item 1", "Item 2"]]))
        .unwrap();
    let source = Range::new(pos(0, 0), pos(0, 1));
    book.extend(SHEET, source, Range::new(pos(0, 0), pos(0, 3)))
        .unwrap();
    assert_eq!(
        row_inputs(&book, 0, 4),
        ["Item 1", "Item 2", "Item 3", "Item 4"]
    );
}

#[test]
fn extend_down_keeps_zero_padding_of_numbered_text() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(0, 0), &rows(&[&["Q08"], &["Q09"]]))
        .unwrap();
    let source = Range::new(pos(0, 0), pos(1, 0));
    book.extend(SHEET, source, Range::new(pos(0, 0), pos(3, 0)))
        .unwrap();
    assert_eq!(column_inputs(&book, 0, 4), ["Q08", "Q09", "Q10", "Q11"]);
}

#[test]
fn extend_repeats_plain_text_with_different_prefixes() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(0, 0), &rows(&[&["a1", "b2"]]))
        .unwrap();
    let source = Range::new(pos(0, 0), pos(0, 1));
    book.extend(SHEET, source, Range::new(pos(0, 0), pos(0, 3)))
        .unwrap();
    assert_eq!(row_inputs(&book, 0, 4), ["a1", "b2", "a1", "b2"]);
}

#[test]
fn extend_to_a_target_that_is_not_past_the_source_changes_nothing() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(1, 1), &rows(&[&["1", "2"]]))
        .unwrap();
    let source = Range::new(pos(1, 1), pos(1, 2));
    book.extend(SHEET, source, Range::new(pos(0, 0), pos(1, 2)))
        .unwrap();
    assert_eq!(row_inputs(&book, 0, 3), ["", "", ""]);
}

#[test]
fn sort_descending_puts_errors_bools_text_numbers_in_reverse_and_blanks_last() {
    let mut book = Workbook::new_empty().unwrap();
    let input = rows(&[&["b"], &["3"], &[""], &["=1/0"], &["=1=1"], &["1"], &["a"]]);
    book.set_inputs(SHEET, pos(0, 0), &input).unwrap();
    let range = Range::new(pos(0, 0), pos(6, 0));
    book.sort(SHEET, range, ColIdx::clamped(0), true).unwrap();
    assert_eq!(
        column_inputs(&book, 0, 7),
        ["=1/0", "=1=1", "b", "a", "3", "1", ""]
    );
}

#[test]
fn sort_ascending_keeps_blanks_last_and_errors_after_bools() {
    let mut book = Workbook::new_empty().unwrap();
    let input = rows(&[&[""], &["=1/0"], &["=1=1"], &["b"], &["2"]]);
    book.set_inputs(SHEET, pos(0, 0), &input).unwrap();
    let range = Range::new(pos(0, 0), pos(4, 0));
    book.sort(SHEET, range, ColIdx::clamped(0), false).unwrap();
    assert_eq!(column_inputs(&book, 0, 5), ["2", "b", "=1=1", "=1/0", ""]);
}

#[test]
fn sort_descending_keeps_equal_keys_in_their_original_order() {
    let mut book = Workbook::new_empty().unwrap();
    let input = rows(&[&["1", "first"], &["2", "x"], &["1", "second"]]);
    book.set_inputs(SHEET, pos(0, 0), &input).unwrap();
    let range = Range::new(pos(0, 0), pos(2, 1));
    book.sort(SHEET, range, ColIdx::clamped(0), true).unwrap();
    assert_eq!(column_inputs(&book, 1, 3), ["x", "first", "second"]);
}

#[test]
fn sort_by_a_column_of_blanks_leaves_the_range_untouched() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos(0, 0), &rows(&[&["", "z"], &["", "a"]]))
        .unwrap();
    let range = Range::new(pos(0, 0), pos(1, 1));
    book.sort(SHEET, range, ColIdx::clamped(0), true).unwrap();
    assert_eq!(column_inputs(&book, 1, 2), ["z", "a"]);
}
