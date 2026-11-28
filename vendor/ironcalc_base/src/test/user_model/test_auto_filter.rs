#![allow(clippy::unwrap_used)]

use crate::auto_filter::LineMove;
use crate::test::util::new_empty_model;
use crate::types::{AutoFilter, FilterColumn, FilterCriteria, SortCondition, SortState};
use crate::UserModel;

fn filter() -> AutoFilter {
    AutoFilter {
        first_row: 2,
        first_column: 2,
        last_row: 10,
        last_column: 4,
        columns: vec![FilterColumn {
            column: 1,
            criteria: FilterCriteria::Values {
                values: vec!["a".to_string()],
                blank: false,
            },
            hidden_button: false,
            show_button: true,
        }],
        filtered_rows: vec![4, 7],
        sort_state: Some(SortState {
            first_row: 3,
            first_column: 2,
            last_row: 10,
            last_column: 4,
            conditions: vec![SortCondition {
                first_row: 3,
                first_column: 3,
                last_row: 10,
                last_column: 3,
                descending: true,
                attributes: String::new(),
            }],
            attributes: String::new(),
        }),
    }
}

fn filter_of(model: &UserModel) -> Option<AutoFilter> {
    model.get_auto_filter(0).unwrap().cloned()
}

#[test]
fn setting_a_filter_is_undone_and_redone() {
    let mut model = UserModel::from_model(new_empty_model());
    model.set_auto_filter(0, Some(filter())).unwrap();
    assert_eq!(filter_of(&model), Some(filter()));
    model.undo().unwrap();
    assert_eq!(filter_of(&model), None);
    model.redo().unwrap();
    assert_eq!(filter_of(&model), Some(filter()));
}

#[test]
fn an_invalid_filter_is_refused() {
    let mut model = UserModel::from_model(new_empty_model());
    let mut outside = filter();
    outside.columns[0].column = 3;
    assert!(model.set_auto_filter(0, Some(outside)).is_err());
    let mut header = filter();
    header.filtered_rows = vec![2];
    assert!(model.set_auto_filter(0, Some(header)).is_err());
    assert_eq!(filter_of(&model), None);
}

#[test]
fn a_sort_outside_the_sheet_makes_a_filter_invalid() {
    let mut model = UserModel::from_model(new_empty_model());
    let mut sorted_nowhere = filter();
    if let Some(state) = &mut sorted_nowhere.sort_state {
        state.conditions[0].last_row = i32::MAX;
    }
    assert!(model.set_auto_filter(0, Some(sorted_nowhere)).is_err());
}

#[test]
fn a_decoded_workbook_with_an_invalid_filter_is_refused_not_a_panic() {
    let mut model = new_empty_model();
    let mut outside = filter();
    outside.last_row = i32::MAX;
    model.workbook.worksheets[0].auto_filter = Some(outside);
    let bytes = model.to_bytes();
    assert!(UserModel::from_bytes(&bytes, "en").is_err());
    let mut model = new_empty_model();
    model.workbook.worksheets[0].auto_filter = Some(filter());
    assert!(UserModel::from_bytes(&model.to_bytes(), "en").is_ok());
}

#[test]
fn scattered_rows_are_hidden_as_one_step_and_undo_leaves_no_entry() {
    let mut model = UserModel::from_model(new_empty_model());
    model.set_rows_height(0, 5, 5, 30.0).unwrap();
    let rows_before = model.get_model().workbook.worksheets[0].rows.clone();
    model
        .set_scattered_rows_hidden(0, &[(3, true), (5, true), (8, true), (9, false)])
        .unwrap();
    for (row, hidden) in [(3, true), (4, false), (5, true), (8, true), (9, false)] {
        assert_eq!(model.get_model().is_row_hidden(0, row), Ok(hidden), "{row}");
    }
    model.undo().unwrap();
    assert_eq!(model.get_model().workbook.worksheets[0].rows, rows_before);
    model.redo().unwrap();
    assert_eq!(model.get_model().is_row_hidden(0, 8), Ok(true));
    model.set_scattered_rows_hidden(0, &[(8, false)]).unwrap();
    model.undo().unwrap();
    assert_eq!(model.get_model().is_row_hidden(0, 8), Ok(true));
    assert!(model.set_scattered_rows_hidden(0, &[(0, true)]).is_err());
}

#[test]
fn rows_and_columns_move_the_filter_and_undo_brings_it_back() {
    let mut model = UserModel::from_model(new_empty_model());
    model.set_auto_filter(0, Some(filter())).unwrap();

    model.insert_rows(0, 5, 2).unwrap();
    let moved = filter_of(&model).unwrap();
    assert_eq!((moved.first_row, moved.last_row), (2, 12));
    assert_eq!(moved.filtered_rows, [4, 9]);
    model.undo().unwrap();
    assert_eq!(filter_of(&model), Some(filter()));
    model.redo().unwrap();
    model.undo().unwrap();

    model.delete_rows(0, 1, 1).unwrap();
    let moved = filter_of(&model).unwrap();
    assert_eq!((moved.first_row, moved.last_row), (1, 9));
    assert_eq!(moved.filtered_rows, [3, 6]);
    model.undo().unwrap();
    assert_eq!(filter_of(&model), Some(filter()));

    model.delete_rows(0, 2, 9).unwrap();
    assert_eq!(filter_of(&model), None);
    model.undo().unwrap();
    assert_eq!(filter_of(&model), Some(filter()));

    model.delete_columns(0, 3, 1).unwrap();
    let moved = filter_of(&model).unwrap();
    assert_eq!((moved.first_column, moved.last_column), (2, 3));
    assert!(moved.columns.is_empty());
    assert_eq!(moved.sort_state, None);
    model.undo().unwrap();
    assert_eq!(filter_of(&model), Some(filter()));

    model.insert_columns(0, 3, 1).unwrap();
    let moved = filter_of(&model).unwrap();
    assert_eq!((moved.first_column, moved.last_column), (2, 5));
    assert_eq!(moved.columns[0].column, 2);
    assert_eq!(moved.sort_state.unwrap().conditions[0].first_column, 4);
    model.undo().unwrap();
    assert_eq!(filter_of(&model), Some(filter()));
}

#[test]
fn lines_past_the_filter_leave_it_alone() {
    let mut model = UserModel::from_model(new_empty_model());
    model.set_auto_filter(0, Some(filter())).unwrap();
    model.insert_rows(0, 11, 3).unwrap();
    model.delete_columns(0, 5, 2).unwrap();
    assert_eq!(filter_of(&model), Some(filter()));
}

#[test]
fn deleting_a_sheet_and_undoing_brings_its_filter_back() {
    let mut model = UserModel::from_model(new_empty_model());
    model.new_sheet().unwrap();
    model.set_auto_filter(1, Some(filter())).unwrap();
    model.delete_sheet(1).unwrap();
    model.undo().unwrap();
    assert_eq!(model.get_auto_filter(1).unwrap(), Some(&filter()));
}

#[test]
fn a_move_keeps_every_index_in_order() {
    let insert = LineMove::Insert { at: 3, count: 2 };
    let delete = LineMove::Delete { at: 3, count: 2 };
    let base = filter();
    assert!(base.with_rows_moved(insert).unwrap().is_valid());
    assert!(base.with_rows_moved(delete).unwrap().is_valid());
    assert!(base.with_columns_moved(insert).unwrap().is_valid());
    assert!(base.with_columns_moved(delete).unwrap().is_valid());
    // Rows pushed past the last row of the sheet leave the range.
    let pushed = base
        .with_rows_moved(LineMove::Insert {
            at: 1,
            count: 1_048_570,
        })
        .unwrap();
    assert_eq!(pushed.last_row, 1_048_576);
    assert!(pushed.is_valid());
}

#[test]
fn hidden_rows_inside_a_filter_with_criteria_count_as_filtered() {
    let mut model = new_empty_model();
    let sheet = &mut model.workbook.worksheets[0];
    sheet.set_row_hidden(1, true).unwrap();
    sheet.set_row_hidden(6, true).unwrap();
    sheet.set_row_hidden(12, true).unwrap();
    let mut unfiltered = filter();
    unfiltered.filtered_rows = vec![];
    sheet.auto_filter = Some(unfiltered.clone());
    sheet.mark_filtered_rows();
    assert_eq!(sheet.auto_filter.as_ref().unwrap().filtered_rows, [6]);
    unfiltered.columns.clear();
    sheet.auto_filter = Some(unfiltered);
    sheet.mark_filtered_rows();
    assert!(sheet.auto_filter.as_ref().unwrap().filtered_rows.is_empty());
}
