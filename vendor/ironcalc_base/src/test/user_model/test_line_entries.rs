#![allow(clippy::unwrap_used)]

use crate::test::user_model::util::new_empty_user_model;

#[test]
fn many_row_heights_are_one_undo_step() {
    let mut model = new_empty_user_model();
    model.set_rows_height(0, 5, 5, 62.5).unwrap();
    let before = model.get_row_height(0, 5).unwrap();
    let heights: Vec<(i32, f64)> = (1..=2000).map(|row| (row, 31.25)).collect();
    model.set_row_heights(0, &heights).unwrap();
    assert_eq!(model.get_row_height(0, 1).unwrap(), 31.25);
    assert_eq!(model.get_row_height(0, 5).unwrap(), 31.25);
    assert_eq!(model.get_row_height(0, 2000).unwrap(), 31.25);
    assert_eq!(model.get_row_height(0, 2001).unwrap(), 25.0);

    model.undo().unwrap();
    assert_eq!(model.get_row_height(0, 1).unwrap(), 25.0);
    assert_eq!(model.get_row_height(0, 5).unwrap(), before);
    model.redo().unwrap();
    assert_eq!(model.get_row_height(0, 1000).unwrap(), 31.25);
    model.undo().unwrap();
    model.undo().unwrap();
    assert!(!model.can_undo());
}

#[test]
fn a_height_edit_keeps_the_rows_style_and_hidden_state() {
    let mut model = new_empty_user_model();
    model.set_rows_hidden(0, 3, 3, true).unwrap();
    model.set_row_heights(0, &[(3, 62.5)]).unwrap();
    assert_eq!(model.get_row_height(0, 3).unwrap(), 0.0);
    model.set_rows_hidden(0, 3, 3, false).unwrap();
    assert_eq!(model.get_row_height(0, 3).unwrap(), 62.5);
}

#[test]
fn hiding_a_range_of_rows_is_one_undo_step_and_showing_it_leaves_no_new_entries() {
    let mut model = new_empty_user_model();
    model.set_rows_height(0, 4, 4, 62.5).unwrap();
    model.set_rows_hidden_in_bulk(0, 2, 6, true).unwrap();
    for row in 2..=6 {
        assert!(model.model.is_row_hidden(0, row).unwrap(), "{row}");
    }
    assert!(!model.model.is_row_hidden(0, 7).unwrap());
    model.set_rows_hidden_in_bulk(0, 1, 100, false).unwrap();
    assert!(!model.model.is_row_hidden(0, 4).unwrap());
    assert_eq!(model.get_row_height(0, 4).unwrap(), 62.5);
    model.undo().unwrap();
    model.undo().unwrap();
    for row in 2..=6 {
        assert!(!model.model.is_row_hidden(0, row).unwrap(), "{row}");
    }
    assert_eq!(model.get_row_height(0, 4).unwrap(), 62.5);
    assert_eq!(model.get_row_height(0, 2).unwrap(), 25.0);
    model.undo().unwrap();
    assert_eq!(model.get_row_height(0, 4).unwrap(), 25.0);
}

#[test]
fn hiding_rows_in_bulk_recalculates_subtotal() {
    let mut model = new_empty_user_model();
    for row in 1..=3 {
        model.set_user_input(0, row, 1, &row.to_string()).unwrap();
    }
    model
        .set_user_input(0, 4, 1, "=SUBTOTAL(109,A1:A3)")
        .unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 4, 1).unwrap(), "6");
    model.set_rows_hidden_in_bulk(0, 2, 2, true).unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 4, 1).unwrap(), "4");
    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 4, 1).unwrap(), "6");
}

#[test]
fn a_run_of_columns_takes_a_width_and_keeps_style_and_hidden_state() {
    let mut model = new_empty_user_model();
    model.set_columns_width(0, 3, 3, 135.0).unwrap();
    model.set_columns_hidden(0, 4, 4, true).unwrap();
    model.set_column_widths(0, &[(2, 5, 180.0)]).unwrap();
    for column in [2, 3, 5] {
        assert_eq!(
            model.get_column_width(0, column).unwrap(),
            180.0,
            "{column}"
        );
    }
    assert!(model.model.is_column_hidden(0, 4).unwrap());
    assert_eq!(model.get_column_width(0, 6).unwrap(), 90.0);
    model.undo().unwrap();
    assert_eq!(model.get_column_width(0, 2).unwrap(), 90.0);
    assert_eq!(model.get_column_width(0, 3).unwrap(), 135.0);
    assert!(model.model.is_column_hidden(0, 4).unwrap());
    model.redo().unwrap();
    assert_eq!(model.get_column_width(0, 5).unwrap(), 180.0);
}

#[test]
fn a_run_inside_a_wide_entry_cuts_it_and_undo_gives_the_columns_back() {
    let mut model = new_empty_user_model();
    model.set_columns_width(0, 1, 20, 135.0).unwrap();
    model.set_column_widths(0, &[(5, 8, 270.0)]).unwrap();
    assert_eq!(model.get_column_width(0, 4).unwrap(), 135.0);
    assert_eq!(model.get_column_width(0, 6).unwrap(), 270.0);
    assert_eq!(model.get_column_width(0, 9).unwrap(), 135.0);
    model.undo().unwrap();
    for column in 1..=20 {
        assert_eq!(
            model.get_column_width(0, column).unwrap(),
            135.0,
            "{column}"
        );
    }
}

#[test]
fn a_whole_sheet_run_of_columns_leaves_a_few_entries() {
    let mut model = new_empty_user_model();
    model.set_column_widths(0, &[(1, 16384, 180.0)]).unwrap();
    assert_eq!(model.get_column_width(0, 1).unwrap(), 180.0);
    assert_eq!(model.get_column_width(0, 16384).unwrap(), 180.0);
    let columns = model.get_model().workbook.worksheet(0).unwrap().cols.len();
    assert_eq!(columns, 1);
}

#[test]
fn a_bad_size_is_refused() {
    let mut model = new_empty_user_model();
    assert!(model.set_row_heights(0, &[(1, f64::NAN)]).is_err());
    assert!(model.set_row_heights(0, &[(1, -1.0)]).is_err());
    assert!(model
        .set_column_widths(0, &[(1, 2, f64::INFINITY)])
        .is_err());
    assert!(!model.can_undo());
}

#[test]
fn the_edit_group_can_be_told_from_a_loose_edit() {
    let mut model = new_empty_user_model();
    assert!(!model.is_edit_group_open());
    model.begin_edit_group().unwrap();
    assert!(model.is_edit_group_open());
    model.end_edit_group().unwrap();
    assert!(!model.is_edit_group_open());
}

#[test]
fn bulk_diffs_travel_to_another_model() {
    let mut model = new_empty_user_model();
    model.set_row_heights(0, &[(2, 62.5), (3, 62.5)]).unwrap();
    model.set_column_widths(0, &[(2, 3, 180.0)]).unwrap();
    let queue = model.flush_send_queue();
    let mut other = new_empty_user_model();
    other.apply_external_diffs(&queue).unwrap();
    assert_eq!(other.get_row_height(0, 3).unwrap(), 62.5);
    assert_eq!(other.get_column_width(0, 3).unwrap(), 180.0);
}
