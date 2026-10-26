#![allow(clippy::unwrap_used)]

use crate::constants::LAST_ROW;
use crate::types::Cell;
use crate::{expressions::types::Area, test::user_model::util::new_empty_user_model, UserModel};

fn area(row: i32, column: i32, width: i32, height: i32) -> Area {
    Area {
        sheet: 0,
        row,
        column,
        width,
        height,
    }
}

fn cell(model: &UserModel, row: i32, column: i32) -> Option<Cell> {
    model
        .get_model()
        .workbook
        .worksheet(0)
        .unwrap()
        .sheet_data
        .get(row, column)
        .cloned()
}

fn last_row(model: &UserModel) -> i32 {
    model
        .get_model()
        .workbook
        .worksheet(0)
        .unwrap()
        .dimension()
        .max_row
}

fn assert_absent(model: &UserModel, rows: std::ops::RangeInclusive<i32>, column: i32) {
    for row in rows {
        assert_eq!(cell(model, row, column), None, "row {row}");
    }
}

fn sequence_of_ten() -> UserModel<'static> {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "10").unwrap();
    model.set_user_input(0, 1, 2, "=SEQUENCE(A1)").unwrap();
    assert_eq!(last_row(&model), 10);
    model
}

#[test]
fn a_shrinking_spill_removes_the_cells_it_leaves() {
    let mut model = sequence_of_ten();
    model.set_user_input(0, 1, 1, "3").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 3, 2), Ok("3".to_string()));
    assert_absent(&model, 4..=10, 2);
    assert_eq!(last_row(&model), 3);
}

#[test]
fn a_cell_with_its_own_style_stays_empty_after_the_spill_leaves() {
    let mut model = new_empty_user_model();
    model.update_range_style(&area(7, 2, 1, 1), "font.b", "true").unwrap();
    let bold = cell(&model, 7, 2).unwrap().get_style();
    model.set_user_input(0, 1, 1, "10").unwrap();
    model.set_user_input(0, 1, 2, "=SEQUENCE(A1)").unwrap();
    model.set_user_input(0, 1, 1, "3").unwrap();
    assert_eq!(cell(&model, 7, 2), Some(Cell::EmptyCell { s: bold }));
    assert_absent(&model, 4..=6, 2);
    assert_absent(&model, 8..=10, 2);
}

#[test]
fn a_spill_in_a_styled_column_removes_the_cells_it_leaves() {
    let mut model = new_empty_user_model();
    model
        .update_range_style(&area(1, 2, 1, LAST_ROW), "font.b", "true")
        .unwrap();
    model.set_user_input(0, 1, 1, "10").unwrap();
    model.set_user_input(0, 1, 2, "=SEQUENCE(A1)").unwrap();
    model.set_user_input(0, 1, 1, "3").unwrap();
    assert_absent(&model, 4..=10, 2);
}

#[test]
fn a_blocked_spill_removes_its_cells_and_spills_again_once_unblocked() {
    let mut model = sequence_of_ten();
    model.set_user_input(0, 5, 2, "x").unwrap();
    assert_eq!(
        model.get_formatted_cell_value(0, 1, 2),
        Ok("#SPILL!".to_string())
    );
    assert_absent(&model, 2..=4, 2);
    assert_absent(&model, 6..=10, 2);
    assert_eq!(last_row(&model), 5);

    model.range_clear_contents(&area(5, 2, 1, 1)).unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 10, 2), Ok("10".to_string()));
    model.set_user_input(0, 1, 1, "3").unwrap();
    assert_absent(&model, 4..=10, 2);
    assert_eq!(last_row(&model), 3);
}

#[test]
fn writing_over_the_anchor_removes_its_spill() {
    let mut model = sequence_of_ten();
    model.set_user_input(0, 1, 2, "x").unwrap();
    assert_absent(&model, 2..=10, 2);
    assert_eq!(last_row(&model), 1);
}

#[test]
fn clearing_the_anchor_removes_its_spill() {
    let mut model = sequence_of_ten();
    model.range_clear_contents(&area(1, 2, 1, 1)).unwrap();
    assert_absent(&model, 2..=10, 2);
    assert_eq!(last_row(&model), 1);
}

#[test]
fn undoing_the_anchor_removes_its_spill() {
    let mut model = sequence_of_ten();
    model.undo().unwrap();
    assert_absent(&model, 2..=10, 2);
    assert_eq!(last_row(&model), 1);
}

#[test]
fn deleting_a_column_the_spill_reads_removes_the_cells_it_leaves() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 2, "=SEQUENCE(COLUMNS(D1:H1))").unwrap();
    assert_eq!(last_row(&model), 5);
    model.delete_columns(0, 5, 1).unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 4, 2), Ok("4".to_string()));
    assert_absent(&model, 5..=5, 2);
    assert_eq!(last_row(&model), 4);
}

#[test]
fn undo_and_redo_of_the_shrink_restore_the_cells() {
    let mut model = sequence_of_ten();
    model.set_user_input(0, 1, 1, "3").unwrap();
    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 10, 2), Ok("10".to_string()));
    assert_eq!(last_row(&model), 10);
    model.redo().unwrap();
    assert_absent(&model, 4..=10, 2);
    assert_eq!(last_row(&model), 3);
}
