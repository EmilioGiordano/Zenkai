#![allow(clippy::unwrap_used)]

use crate::expressions::types::Area;
use crate::test::user_model::util::new_empty_user_model;

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
fn fill_style_copies_the_whole_style_and_undo_redo_restore_it() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "3/15/2024").unwrap();
    model
        .update_range_style(&area(1, 1, 1, 1), "font.b", "true")
        .unwrap();
    model
        .update_range_style(&area(1, 1, 1, 1), "fill.bg_color", "#FF0000")
        .unwrap();
    model.set_user_input(0, 3, 1, "x").unwrap();
    let source = model.get_cell_style(0, 1, 1).unwrap();
    let old = model.get_cell_style(0, 3, 1).unwrap();

    model.fill_style((1, 1), &area(2, 1, 1, 4)).unwrap();
    for row in 2..=5 {
        assert_eq!(model.get_cell_style(0, row, 1).unwrap(), source, "{row}");
    }
    model.undo().unwrap();
    assert_eq!(model.get_cell_style(0, 3, 1).unwrap(), old);
    assert!(model
        .model
        .workbook
        .worksheet(0)
        .unwrap()
        .cell(2, 1)
        .is_none());
    assert!(model
        .model
        .workbook
        .worksheet(0)
        .unwrap()
        .cell(5, 1)
        .is_none());
    model.redo().unwrap();
    assert_eq!(model.get_cell_style(0, 5, 1).unwrap(), source);
}

#[test]
fn fill_style_of_an_empty_cell_passes_its_column_style_on() {
    let mut model = new_empty_user_model();
    model
        .update_range_style(&area(1, 1, 1, crate::constants::LAST_ROW), "font.i", "true")
        .unwrap();
    model.set_user_input(0, 1, 2, "kept").unwrap();
    model.fill_style((1, 1), &area(1, 2, 1, 2)).unwrap();
    assert!(model.get_cell_style(0, 2, 2).unwrap().font.i);
    assert_eq!(model.get_formatted_cell_value(0, 1, 2).unwrap(), "kept");
}

#[test]
fn fill_style_rejects_an_area_outside_the_sheet() {
    let mut model = new_empty_user_model();
    assert!(model.fill_style((1, 1), &area(0, 1, 1, 1)).is_err());
    assert!(model
        .fill_style((1, 1), &area(crate::constants::LAST_ROW, 1, 1, 2))
        .is_err());
    assert!(!model.can_undo());
}

#[test]
fn undo_and_redo_report_the_filled_area() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.fill_style((1, 1), &area(2, 1, 2, 3)).unwrap();
    let reported = |cells: crate::StepCells| {
        cells
            .areas
            .iter()
            .map(|a| (a.sheet, a.row, a.column, a.width, a.height))
            .collect::<Vec<_>>()
    };
    assert_eq!(reported(model.undo_cells().unwrap()), vec![(0, 2, 1, 2, 3)]);
    model.undo().unwrap();
    assert_eq!(reported(model.redo_cells().unwrap()), vec![(0, 2, 1, 2, 3)]);
}

#[test]
fn two_fills_then_undo_twice_restore_the_state_before_both() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "3/15/2024").unwrap();
    model.set_user_input(0, 3, 1, "x").unwrap();
    let before = model.get_cell_style(0, 3, 1).unwrap();
    model.fill_style((1, 1), &area(2, 1, 1, 3)).unwrap();
    model.set_user_input(0, 3, 1, "typed").unwrap();
    model
        .update_range_style(&area(4, 1, 1, 1), "font.b", "true")
        .unwrap();
    model.undo().unwrap();
    model.undo().unwrap();
    model.undo().unwrap();
    assert_eq!(model.get_cell_style(0, 3, 1).unwrap(), before);
    assert!(model.model.workbook.worksheet(0).unwrap().cell(2, 1).is_none());
    assert!(model.model.workbook.worksheet(0).unwrap().cell(4, 1).is_none());
}

#[test]
fn fills_on_the_undo_and_redo_stacks_survive_the_history_round_trip() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "3/15/2024").unwrap();
    model
        .update_range_style(&area(1, 1, 1, 1), "font.b", "true")
        .unwrap();
    let source = model.get_cell_style(0, 1, 1).unwrap();
    model.fill_style((1, 1), &area(2, 1, 1, 2)).unwrap();
    model.fill_style((1, 1), &area(5, 1, 1, 2)).unwrap();
    model.undo().unwrap();
    let bytes = model.to_bytes_with_history().unwrap();
    let mut restored = crate::UserModel::from_bytes_with_history(&bytes, "en").unwrap();
    assert_eq!(restored.get_cell_style(0, 3, 1).unwrap(), source);
    assert!(restored.model.workbook.worksheet(0).unwrap().cell(5, 1).is_none());
    restored.redo().unwrap();
    assert_eq!(restored.get_cell_style(0, 6, 1).unwrap(), source);
    restored.undo().unwrap();
    restored.undo().unwrap();
    assert!(restored.model.workbook.worksheet(0).unwrap().cell(2, 1).is_none());
    restored.redo().unwrap();
    assert_eq!(restored.get_cell_style(0, 2, 1).unwrap(), source);
}
