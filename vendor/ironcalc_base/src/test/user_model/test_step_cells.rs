#![allow(clippy::unwrap_used)]

use crate::{test::user_model::util::new_empty_user_model, StepCells};

fn areas(cells: &StepCells) -> Vec<(u32, i32, i32, i32, i32)> {
    cells
        .areas
        .iter()
        .map(|area| (area.sheet, area.row, area.column, area.width, area.height))
        .collect()
}

#[test]
fn nothing_to_undo_or_redo_changes_no_cells() {
    let model = new_empty_user_model();
    let undo = model.undo_cells().unwrap();
    let redo = model.redo_cells().unwrap();
    assert!(undo.areas.is_empty() && undo.sheets.is_empty());
    assert!(redo.areas.is_empty() && redo.sheets.is_empty());
}

#[test]
fn a_cell_edit_reaches_its_cell_both_ways() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 3, 2, "x").unwrap();
    let undo = model.undo_cells().unwrap();
    assert_eq!(areas(&undo), vec![(0, 3, 2, 1, 1)]);
    assert!(undo.sheets.is_empty());

    model.undo().unwrap();
    assert!(model.undo_cells().unwrap().areas.is_empty());
    assert_eq!(areas(&model.redo_cells().unwrap()), vec![(0, 3, 2, 1, 1)]);
}

#[test]
fn a_cleared_range_reaches_its_area() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 4, 3, "2").unwrap();
    model
        .range_clear_contents(&crate::expressions::types::Area {
            sheet: 0,
            row: 1,
            column: 1,
            width: 3,
            height: 4,
        })
        .unwrap();
    assert_eq!(areas(&model.undo_cells().unwrap()), vec![(0, 1, 1, 3, 4)]);
}

#[test]
fn inserted_rows_reach_their_whole_sheet() {
    let mut model = new_empty_user_model();
    model.insert_rows(0, 2, 1).unwrap();
    let undo = model.undo_cells().unwrap();
    assert!(undo.areas.is_empty());
    assert_eq!(undo.sheets, vec![0]);
}

#[test]
fn a_new_sheet_reaches_the_whole_workbook() {
    let mut model = new_empty_user_model();
    model.new_sheet().unwrap();
    assert!(model.undo_cells().is_none());
    model.undo().unwrap();
    assert!(model.redo_cells().is_none());
}
