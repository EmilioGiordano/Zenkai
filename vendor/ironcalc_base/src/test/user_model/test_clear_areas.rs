#![allow(clippy::unwrap_used)]

use crate::{expressions::types::Area, test::user_model::util::new_empty_user_model};

fn area(row: i32, column: i32, width: i32, height: i32) -> Area {
    Area {
        sheet: 0,
        row,
        column,
        width,
        height,
    }
}

#[test]
fn several_areas_are_one_undo_step() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 1, 2, "2").unwrap();
    model.set_user_input(0, 5, 4, "=A1+B1").unwrap();
    model.set_user_input(0, 9, 1, "keep").unwrap();
    model
        .range_clear_contents_of_areas(
            &area(1, 1, 4, 5),
            &[area(1, 1, 2, 1), area(5, 4, 1, 1)],
        )
        .unwrap();
    assert_eq!(model.get_cell_content(0, 1, 1), Ok("".to_string()));
    assert_eq!(model.get_cell_content(0, 1, 2), Ok("".to_string()));
    assert_eq!(model.get_cell_content(0, 5, 4), Ok("".to_string()));
    assert_eq!(model.get_cell_content(0, 9, 1), Ok("keep".to_string()));

    model.undo().unwrap();
    assert_eq!(model.get_cell_content(0, 1, 1), Ok("1".to_string()));
    assert_eq!(model.get_cell_content(0, 1, 2), Ok("2".to_string()));
    assert_eq!(model.get_cell_content(0, 5, 4), Ok("=A1+B1".to_string()));
    assert_eq!(model.get_formatted_cell_value(0, 5, 4), Ok("3".to_string()));

    model.redo().unwrap();
    assert_eq!(model.get_cell_content(0, 1, 1), Ok("".to_string()));
    assert_eq!(model.get_cell_content(0, 5, 4), Ok("".to_string()));
    model.undo().unwrap();
    assert_eq!(model.get_cell_content(0, 1, 2), Ok("2".to_string()));
    // The first undo restored everything, so a second one undoes the last input.
    model.undo().unwrap();
    assert_eq!(model.get_cell_content(0, 9, 1), Ok("".to_string()));
}

#[test]
fn spill_split_across_areas_clears_inside_bounds() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "=SEQUENCE(3)").unwrap();
    model.set_user_input(0, 2, 2, "x").unwrap();
    model.set_user_input(0, 3, 2, "y").unwrap();
    model
        .range_clear_contents_of_areas(
            &area(1, 1, 2, 3),
            &[area(1, 1, 1, 1), area(2, 1, 2, 2)],
        )
        .unwrap();
    for row in 1..=3 {
        assert_eq!(model.get_formatted_cell_value(0, row, 1), Ok("".to_string()));
    }
    model.undo().unwrap();
    assert_eq!(model.get_cell_content(0, 1, 1), Ok("=SEQUENCE(3)".to_string()));
    assert_eq!(model.get_formatted_cell_value(0, 3, 1), Ok("3".to_string()));
    assert_eq!(model.get_cell_content(0, 3, 2), Ok("y".to_string()));
}

#[test]
fn array_split_across_areas_clears_inside_bounds() {
    let mut model = new_empty_user_model();
    model
        .set_user_array_formula(0, 1, 1, 1, 3, "=SEQUENCE(3)")
        .unwrap();
    model.set_user_input(0, 2, 2, "x").unwrap();
    model
        .range_clear_contents_of_areas(&area(1, 1, 2, 3), &[area(1, 1, 1, 1), area(2, 1, 2, 2)])
        .unwrap();
    assert_eq!(model.get_cell_content(0, 1, 1), Ok("".to_string()));
    assert_eq!(model.get_formatted_cell_value(0, 3, 1), Ok("".to_string()));
    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 3, 1), Ok("3".to_string()));
    assert_eq!(model.get_cell_content(0, 2, 2), Ok("x".to_string()));
}

#[test]
fn array_reaching_outside_bounds_changes_nothing() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 3, "kept").unwrap();
    model
        .set_user_array_formula(0, 1, 1, 1, 3, "=SEQUENCE(3)")
        .unwrap();
    let result = model.range_clear_contents_of_areas(
        &area(1, 1, 3, 2),
        &[area(1, 3, 1, 1), area(1, 1, 1, 2)],
    );
    assert!(result.is_err());
    assert_eq!(model.get_cell_content(0, 1, 3), Ok("kept".to_string()));
    assert_eq!(model.get_formatted_cell_value(0, 3, 1), Ok("3".to_string()));
}

#[test]
fn area_outside_bounds_is_rejected() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 4, 4, "1").unwrap();
    let result = model.range_clear_contents_of_areas(&area(1, 1, 2, 2), &[area(4, 4, 1, 1)]);
    assert!(result.is_err());
    assert_eq!(model.get_cell_content(0, 4, 4), Ok("1".to_string()));
}
