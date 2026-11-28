// Spec (#340): a sheet's AutoFilter hides the data rows its criteria reject, apart from rows
// hidden by hand, lists each column's values as Excel does, sorts only the visible rows,
// keeps edits off the rows it hides, and goes through undo, moves and open and save intact.
#![allow(clippy::unwrap_used)]

use std::cmp::Ordering;

use proptest::prelude::*;
use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{
    CellPos, ColIdx, ColumnCriteria, FilterList, MAX_FILTER_ITEMS, Range, RowIdx, SheetId,
    ShownValues,
};

#[path = "auto_filter/files.rs"]
mod files;

const SHEET: SheetId = SheetId(0);

fn pos(a1: &str) -> CellPos {
    CellPos::parse_a1(a1).unwrap()
}

fn range(a1: &str) -> Range {
    Range::parse_a1(a1).unwrap()
}

fn col(letter: char) -> ColIdx {
    ColIdx::clamped(i64::from(u8::try_from(letter).unwrap() - b'A'))
}

fn row(number: u32) -> RowIdx {
    RowIdx::clamped(i64::from(number) - 1)
}

fn rows<const N: usize>(values: &[[&str; N]]) -> Vec<Vec<String>> {
    values
        .iter()
        .map(|cells| cells.iter().map(|cell| cell.to_string()).collect())
        .collect()
}

fn shown(values: &[&str], blanks: bool) -> Option<ShownValues> {
    Some(ShownValues {
        values: values.iter().map(|value| value.to_string()).collect(),
        blanks,
    })
}

// 1-based, as the row headers show them.
fn hidden_rows(book: &Workbook) -> Vec<u32> {
    let mut hidden: Vec<u32> = book
        .sizes(SHEET)
        .rows
        .iter()
        .filter(|(_, height)| *height == 0.0)
        .map(|(row, _)| row.get() + 1)
        .collect();
    hidden.sort_unstable();
    hidden.dedup();
    hidden
}

fn listed(list: &FilterList) -> Vec<(String, u32, bool)> {
    list.items
        .iter()
        .map(|item| (item.text.clone(), item.count, item.shown))
        .collect()
}

fn people() -> Workbook {
    let mut book = Workbook::new_empty().unwrap();
    let table = rows(&[
        ["Name", "Qty"],
        ["x", "1"],
        ["y", "2"],
        ["x", "3"],
        ["z", "4"],
        ["Y", "5"],
        ["", "6"],
    ]);
    book.set_inputs(SHEET, pos("A1"), &table).unwrap();
    book.toggle_auto_filter(SHEET, range("A1:B7")).unwrap();
    book
}

#[test]
fn a_new_filter_hides_nothing_and_counts_every_record() {
    let book = people();
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!(view.range, range("A1:B7"));
    assert!(!view.is_filtering());
    assert!(view.filtered_out.is_empty());
    assert_eq!((view.found, view.total), (6, 6));
    assert!(hidden_rows(&book).is_empty());
}

#[test]
fn a_column_filter_hides_the_rows_it_rejects_ignoring_case() {
    let mut book = people();
    book.set_column_filter(SHEET, col('A'), shown(&["x"], false))
        .unwrap();
    assert_eq!(hidden_rows(&book), [3, 5, 6, 7]);
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!((view.found, view.total), (2, 6));
    assert_eq!(view.filtered_out, [(row(3), row(3)), (row(5), row(7))]);
    assert_eq!(
        view.criteria,
        [(
            col('A'),
            ColumnCriteria::Values(ShownValues {
                values: vec!["x".to_string()],
                blanks: false,
            })
        )]
    );

    book.set_column_filter(SHEET, col('A'), shown(&["y"], false))
        .unwrap();
    assert_eq!(hidden_rows(&book), [2, 4, 5, 7]);
    book.set_column_filter(SHEET, col('A'), shown(&[], true))
        .unwrap();
    assert_eq!(hidden_rows(&book), [2, 3, 4, 5, 6]);
}

#[test]
fn criteria_on_two_columns_must_both_pass() {
    let mut book = people();
    book.set_column_filter(SHEET, col('A'), shown(&["x", "y"], false))
        .unwrap();
    book.set_column_filter(SHEET, col('B'), shown(&["1", "2", "5", "6"], false))
        .unwrap();
    assert_eq!(hidden_rows(&book), [4, 5, 7]);
    book.set_column_filter(SHEET, col('A'), None).unwrap();
    assert_eq!(hidden_rows(&book), [4, 5]);
}

#[test]
fn rows_hidden_by_hand_stay_hidden_whatever_the_filter_does() {
    let mut book = people();
    book.set_rows_hidden(SHEET, range("A4:A4"), true).unwrap();
    book.set_rows_hidden(SHEET, range("A9:A9"), true).unwrap();
    book.set_column_filter(SHEET, col('A'), shown(&["y"], false))
        .unwrap();
    assert_eq!(hidden_rows(&book), [2, 4, 5, 7, 9]);
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!(
        view.filtered_out,
        [(row(2), row(2)), (row(5), row(5)), (row(7), row(7))]
    );
    assert_eq!(view.found, 2);

    book.clear_filters(SHEET).unwrap();
    assert_eq!(hidden_rows(&book), [4, 9]);
    assert!(book.auto_filter(SHEET).is_some());

    book.set_column_filter(SHEET, col('A'), shown(&["z"], false))
        .unwrap();
    book.toggle_auto_filter(SHEET, range("A1:B7")).unwrap();
    assert!(book.auto_filter(SHEET).is_none());
    assert_eq!(hidden_rows(&book), [4, 9]);
}

#[test]
fn filtering_is_one_undo_step_and_redo_brings_it_back() {
    let mut book = people();
    book.set_column_filter(SHEET, col('A'), shown(&["x"], false))
        .unwrap();
    let filtered = book.auto_filter(SHEET);
    book.undo().unwrap();
    assert!(hidden_rows(&book).is_empty());
    assert!(!book.auto_filter(SHEET).unwrap().is_filtering());
    book.redo().unwrap();
    assert_eq!(hidden_rows(&book), [3, 5, 6, 7]);
    assert_eq!(book.auto_filter(SHEET), filtered);

    book.toggle_auto_filter(SHEET, range("A1:B7")).unwrap();
    assert!(hidden_rows(&book).is_empty());
    book.undo().unwrap();
    assert_eq!(book.auto_filter(SHEET), filtered);
    assert_eq!(hidden_rows(&book), [3, 5, 6, 7]);
}

#[test]
fn filter_commands_on_a_sheet_without_a_filter_are_refused() {
    let mut book = Workbook::new_empty().unwrap();
    assert!(book.auto_filter(SHEET).is_none());
    assert!(book.clear_filters(SHEET).is_err());
    assert!(book.filter_values(SHEET, col('A'), "").is_err());
    assert!(book.sort_filter(SHEET, col('A'), false).is_err());
    assert!(
        book.set_column_filter(SHEET, col('A'), shown(&["x"], false))
            .is_err()
    );
    let book = people();
    assert!(book.filter_values(SHEET, col('C'), "").is_err());
}

#[test]
fn the_list_merges_case_and_sorts_numbers_text_logicals_then_errors() {
    let mut book = Workbook::new_empty().unwrap();
    let column = rows(&[
        ["Head"],
        ["10"],
        ["b"],
        ["=1/0"],
        ["A"],
        ["TRUE"],
        ["2"],
        ["a"],
        [""],
        ["FALSE"],
    ]);
    book.set_inputs(SHEET, pos("A1"), &column).unwrap();
    book.toggle_auto_filter(SHEET, range("A1:A11")).unwrap();
    let list = book.filter_values(SHEET, col('A'), "").unwrap();
    assert_eq!(
        listed(&list),
        [
            ("2".to_string(), 1, true),
            ("10".to_string(), 1, true),
            ("A".to_string(), 2, true),
            ("b".to_string(), 1, true),
            ("FALSE".to_string(), 1, true),
            ("TRUE".to_string(), 1, true),
            ("#DIV/0!".to_string(), 1, true),
        ]
    );
    let blanks = list.blanks.unwrap();
    assert_eq!(
        (blanks.text.as_str(), blanks.count, blanks.shown),
        ("", 2, true)
    );
    assert!(!list.truncated && !list.custom);

    book.set_column_filter(SHEET, col('A'), shown(&["a", "10"], false))
        .unwrap();
    let list = book.filter_values(SHEET, col('A'), "").unwrap();
    let checked: Vec<&str> = list
        .items
        .iter()
        .filter(|item| item.shown)
        .map(|item| item.text.as_str())
        .collect();
    assert_eq!(checked, ["10", "A"]);
    assert!(!list.blanks.unwrap().shown);
}

#[test]
fn the_list_has_no_blanks_entry_when_no_row_is_empty() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_inputs(SHEET, pos("A1"), &rows(&[["Head"], ["1"], ["2"]]))
        .unwrap();
    book.toggle_auto_filter(SHEET, range("A1:A3")).unwrap();
    assert!(
        book.filter_values(SHEET, col('A'), "")
            .unwrap()
            .blanks
            .is_none()
    );
}

#[test]
fn the_list_shows_only_rows_the_other_columns_let_through() {
    let mut book = people();
    book.set_column_filter(SHEET, col('A'), shown(&["x"], false))
        .unwrap();
    let qty = book.filter_values(SHEET, col('B'), "").unwrap();
    assert_eq!(
        listed(&qty),
        [("1".to_string(), 1, true), ("3".to_string(), 1, true)]
    );
    let names = book.filter_values(SHEET, col('A'), "").unwrap();
    assert_eq!(
        listed(&names),
        [
            ("x".to_string(), 2, true),
            ("y".to_string(), 2, false),
            ("z".to_string(), 1, false),
        ]
    );
    assert!(!names.blanks.unwrap().shown);
}

#[test]
fn a_search_lists_matching_values_ignoring_case_and_no_blanks() {
    let mut book = Workbook::new_empty().unwrap();
    let column = rows(&[["Fruit"], ["apple"], ["Grape"], ["banana"], ["APPLE"], [""]]);
    book.set_inputs(SHEET, pos("A1"), &column).unwrap();
    book.toggle_auto_filter(SHEET, range("A1:A6")).unwrap();
    let list = book.filter_values(SHEET, col('A'), "aP").unwrap();
    assert_eq!(
        listed(&list),
        [
            ("apple".to_string(), 2, true),
            ("Grape".to_string(), 1, true)
        ]
    );
    assert!(list.blanks.is_none());
}

#[test]
fn the_list_stops_at_the_cap_and_says_so() {
    let mut book = Workbook::new_empty().unwrap();
    let count = MAX_FILTER_ITEMS + 1;
    let mut column = vec![vec!["Head".to_string()]];
    column.extend((1..=count).rev().map(|n| vec![n.to_string()]));
    book.set_inputs(SHEET, pos("A1"), &column).unwrap();
    let last = u32::try_from(count + 1).unwrap();
    let filter_range = Range::new(pos("A1"), CellPos::new(row(last), col('A')));
    book.toggle_auto_filter(SHEET, filter_range).unwrap();
    let list = book.filter_values(SHEET, col('A'), "").unwrap();
    assert!(list.truncated);
    assert_eq!(list.items.len(), MAX_FILTER_ITEMS);
    assert_eq!(list.items[0].text, "1");
    assert_eq!(
        list.items[MAX_FILTER_ITEMS - 1].text,
        MAX_FILTER_ITEMS.to_string()
    );

    let exactly = book.filter_values(SHEET, col('A'), "1000").unwrap();
    assert!(!exactly.truncated);
    assert_eq!(listed(&exactly)[0], ("1000".to_string(), 1, true));
}

fn passes(value: &str, shown_values: &[&str], blanks: bool) -> bool {
    if value.is_empty() {
        return blanks;
    }
    shown_values
        .iter()
        .any(|shown| shown.eq_ignore_ascii_case(value))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn clearing_or_undoing_a_filter_gives_back_every_rows_hidden_state(
        values in prop::collection::vec(
            prop::sample::select(vec!["a", "A", "b", "1", "TRUE", ""]),
            1..16,
        ),
        by_hand in prop::collection::vec(any::<bool>(), 20),
        shown_values in prop::collection::vec(
            prop::sample::select(vec!["a", "b", "1", "true", "c"]),
            0..4,
        ),
        blanks in any::<bool>(),
    ) {
        let mut book = Workbook::new_empty().unwrap();
        let mut column = vec![vec!["Head".to_string()]];
        column.extend(values.iter().map(|value| vec![value.to_string()]));
        book.set_inputs(SHEET, pos("A1"), &column).unwrap();
        let last = u32::try_from(values.len() + 1).unwrap();
        for (number, hide) in (2..).zip(&by_hand) {
            if *hide {
                let line = Range::single(CellPos::new(row(number), col('A')));
                book.set_rows_hidden(SHEET, line, true).unwrap();
            }
        }
        let before = hidden_rows(&book);
        book.toggle_auto_filter(SHEET, Range::new(pos("A1"), CellPos::new(row(last), col('A'))))
            .unwrap();
        prop_assert_eq!(hidden_rows(&book), before.clone());

        book.set_column_filter(SHEET, col('A'), shown(&shown_values, blanks)).unwrap();
        let mut expected: Vec<u32> = before.clone();
        expected.extend(
            (2..)
                .zip(&values)
                .filter(|(_, value)| !passes(value, &shown_values, blanks))
                .map(|(number, _)| number),
        );
        expected.sort_unstable();
        expected.dedup();
        let filtered = hidden_rows(&book);
        prop_assert_eq!(&filtered, &expected);
        let view = book.auto_filter(SHEET).unwrap();
        let hidden_data = filtered.iter().filter(|number| (2..=last).contains(*number)).count();
        prop_assert_eq!(view.found + u32::try_from(hidden_data).unwrap(), view.total);

        book.clear_filters(SHEET).unwrap();
        prop_assert_eq!(hidden_rows(&book), before.clone());
        book.undo().unwrap();
        prop_assert_eq!(hidden_rows(&book), filtered.clone());
        book.undo().unwrap();
        prop_assert_eq!(hidden_rows(&book), before.clone());
        book.redo().unwrap();
        prop_assert_eq!(hidden_rows(&book), filtered);
        book.toggle_auto_filter(SHEET, range("A1:A1")).unwrap();
        prop_assert!(book.auto_filter(SHEET).is_none());
        prop_assert_eq!(hidden_rows(&book), before);
    }
}

// Excel's ascending order for the keys below; blanks last both ways.
fn excel_order(a: &str, b: &str, descending: bool) -> Ordering {
    let key = |text: &str| -> Option<(u8, f64, String)> {
        if text.is_empty() {
            None
        } else if let Ok(number) = text.parse::<f64>() {
            Some((0, number, String::new()))
        } else {
            Some((1, 0.0, text.to_lowercase()))
        }
    };
    match (key(a), key(b)) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let ascending =
                x.0.cmp(&y.0)
                    .then(x.1.total_cmp(&y.1))
                    .then_with(|| x.2.cmp(&y.2));
            if descending {
                ascending.reverse()
            } else {
                ascending
            }
        }
    }
}

fn table(book: &Workbook, last: u32) -> Vec<(String, String)> {
    (2..=last)
        .map(|number| {
            (
                book.input(SHEET, CellPos::new(row(number), col('A'))),
                book.input(SHEET, CellPos::new(row(number), col('B'))),
            )
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn sorting_a_filter_reorders_only_its_visible_rows_and_loses_nothing(
        keys in prop::collection::vec(
            prop_oneof![
                (-3i32..10).prop_map(|n| n.to_string()),
                "[a-c]|[A-C]",
                Just(String::new()),
            ],
            1..15,
        ),
        hide in prop::collection::vec(any::<bool>(), 15),
        descending in any::<bool>(),
    ) {
        let mut book = Workbook::new_empty().unwrap();
        let mut cells = vec![vec!["Key".to_string(), "Id".to_string()]];
        cells.extend(keys.iter().enumerate().map(|(at, key)| vec![key.clone(), format!("r{at}")]));
        book.set_inputs(SHEET, pos("A1"), &cells).unwrap();
        let last = u32::try_from(keys.len() + 1).unwrap();
        book.toggle_auto_filter(SHEET, Range::new(pos("A1"), CellPos::new(row(last), col('B'))))
            .unwrap();
        let kept: Vec<String> = (0..keys.len())
            .filter(|at| !hide[*at])
            .map(|at| format!("r{at}"))
            .collect();
        let kept: Vec<&str> = kept.iter().map(String::as_str).collect();
        book.set_column_filter(SHEET, col('B'), shown(&kept, false)).unwrap();
        let hidden = hidden_rows(&book);
        let before = table(&book, last);

        book.sort_filter(SHEET, col('A'), descending).unwrap();
        let after = table(&book, last);
        let mut was = before.clone();
        let mut now = after.clone();
        was.sort();
        now.sort();
        prop_assert_eq!(was, now);
        prop_assert_eq!(hidden_rows(&book), hidden.clone());
        let is_hidden = |at: usize| hidden.contains(&(u32::try_from(at).unwrap() + 2));
        let mut expected: Vec<(String, String)> = before
            .iter()
            .enumerate()
            .filter(|(at, _)| !is_hidden(*at))
            .map(|(_, line)| line.clone())
            .collect();
        expected.sort_by(|a, b| excel_order(&a.0, &b.0, descending));
        let visible: Vec<(String, String)> = after
            .iter()
            .enumerate()
            .filter(|(at, _)| !is_hidden(*at))
            .map(|(_, line)| line.clone())
            .collect();
        prop_assert_eq!(visible, expected);
        for (at, line) in after.iter().enumerate() {
            if is_hidden(at) {
                prop_assert_eq!(line, &before[at]);
            }
        }

        book.undo().unwrap();
        prop_assert_eq!(table(&book, last), before);
    }
}

#[test]
fn a_sort_from_the_filter_leaves_the_header_and_is_remembered() {
    let mut book = people();
    book.sort_filter(SHEET, col('B'), true).unwrap();
    assert_eq!(book.input(SHEET, pos("A1")), "Name");
    assert_eq!(book.input(SHEET, pos("B2")), "6");
    assert_eq!(book.input(SHEET, pos("B7")), "1");
    let saved = files::sheet_xml(&book.to_xlsx().unwrap());
    assert!(
        saved.contains(r#"<sortState ref="A2:B7"><sortCondition descending="1" ref="B2:B7"/>"#),
        "{saved}"
    );
}

fn filtered_people() -> Workbook {
    let mut book = people();
    book.set_column_filter(SHEET, col('A'), shown(&["x", "z"], false))
        .unwrap();
    assert_eq!(hidden_rows(&book), [3, 6, 7]);
    book
}

#[test]
fn delete_leaves_the_rows_a_filter_hides_untouched() {
    let mut book = filtered_people();
    book.clear(SHEET, range("B2:B7")).unwrap();
    let qty: Vec<String> = (2..=7)
        .map(|number| book.input(SHEET, CellPos::new(row(number), col('B'))))
        .collect();
    assert_eq!(qty, ["", "2", "", "", "5", "6"]);
    book.undo().unwrap();
    assert_eq!(book.input(SHEET, pos("B2")), "1");
}

#[test]
fn fill_down_and_typing_over_a_selection_skip_the_rows_a_filter_hides() {
    let mut book = filtered_people();
    book.fill(SHEET, range("B2:B7"), true).unwrap();
    let qty: Vec<String> = (2..=7)
        .map(|number| book.input(SHEET, CellPos::new(row(number), col('B'))))
        .collect();
    assert_eq!(qty, ["1", "2", "1", "1", "5", "6"]);

    book.fill_with(SHEET, pos("B2"), "=ROW()", range("B2:B7"))
        .unwrap();
    let qty: Vec<String> = (2..=7)
        .map(|number| book.input(SHEET, CellPos::new(row(number), col('B'))))
        .collect();
    assert_eq!(qty, ["=ROW()", "2", "=ROW()", "=ROW()", "5", "6"]);
    book.undo().unwrap();
    assert_eq!(book.input(SHEET, pos("B4")), "1");
}

#[test]
fn fill_right_skips_a_row_a_filter_hides() {
    let mut book = filtered_people();
    book.fill(SHEET, range("A2:B3"), false).unwrap();
    assert_eq!(book.input(SHEET, pos("B2")), "x");
    assert_eq!(book.input(SHEET, pos("B3")), "2");
}

#[test]
fn pasting_several_rows_over_rows_a_filter_hides_is_refused() {
    let mut book = filtered_people();
    let copied = book.copy(SHEET, range("D1:D2")).unwrap();
    assert!(matches!(
        book.paste(SHEET, pos("B2"), &copied, false),
        Err(EngineError::FilteredPaste)
    ));
    assert_eq!(book.input(SHEET, pos("B2")), "1");
    let one_row = book.copy(SHEET, range("D1:E1")).unwrap();
    book.paste(SHEET, pos("A2"), &one_row, false).unwrap();
    book.paste(SHEET, pos("B9"), &copied, false).unwrap();
}

#[test]
fn the_filter_follows_inserted_and_deleted_rows_and_columns() {
    let mut book = people();
    book.set_column_filter(SHEET, col('B'), shown(&["1", "2"], false))
        .unwrap();
    book.insert_rows(SHEET, row(1), 2).unwrap();
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!(view.range, range("A3:B9"));
    assert_eq!(hidden_rows(&book), [6, 7, 8, 9]);
    assert_eq!(view.filtered_out, [(row(6), row(9))]);

    book.insert_columns(SHEET, col('A'), 1).unwrap();
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!(view.range, range("B3:C9"));
    assert_eq!(view.criteria[0].0, col('C'));

    book.delete_rows(SHEET, row(6), 1).unwrap();
    assert_eq!(book.auto_filter(SHEET).unwrap().range, range("B3:C8"));
    book.delete_columns(SHEET, col('C'), 1).unwrap();
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!(view.range, range("B3:B8"));
    assert!(view.criteria.is_empty());

    for _ in 0..4 {
        book.undo().unwrap();
    }
    let view = book.auto_filter(SHEET).unwrap();
    assert_eq!(view.range, range("A1:B7"));
    assert_eq!(hidden_rows(&book), [4, 5, 6, 7]);
}

#[test]
fn deleting_the_whole_filter_range_removes_the_filter() {
    let mut book = people();
    book.delete_rows(SHEET, row(1), 7).unwrap();
    assert!(book.auto_filter(SHEET).is_none());
    book.undo().unwrap();
    assert_eq!(book.auto_filter(SHEET).unwrap().range, range("A1:B7"));
}

#[test]
fn subtotal_leaves_filtered_rows_out_and_rows_hidden_by_hand_only_from_101_on() {
    let mut book = people();
    book.set_input(SHEET, pos("D1"), "=SUBTOTAL(9,B2:B7)")
        .unwrap();
    book.set_input(SHEET, pos("E1"), "=SUBTOTAL(109,B2:B7)")
        .unwrap();
    book.set_rows_hidden(SHEET, range("A2:A2"), true).unwrap();
    assert_eq!(book.cell(SHEET, pos("D1")).text, "21");
    assert_eq!(book.cell(SHEET, pos("E1")).text, "20");
    book.set_column_filter(SHEET, col('A'), shown(&["x", "y"], false))
        .unwrap();
    // Excel: SUBTOTAL 1 to 11 still counts rows hidden by hand, never filtered ones.
    assert_eq!(book.cell(SHEET, pos("D1")).text, "11");
    assert_eq!(book.cell(SHEET, pos("E1")).text, "10");
}
