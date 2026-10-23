#![allow(clippy::unwrap_used)]

use crate::constants::LAST_COLUMN;
use crate::expressions::types::Area;
use crate::test::user_model::util::new_empty_user_model;
use crate::UserModel;

const PERCENT: &str = "#,##0%";

fn area(row: i32, column: i32, width: i32, height: i32) -> Area {
    Area {
        sheet: 0,
        row,
        column,
        width,
        height,
    }
}

fn has_cell(model: &UserModel, row: i32, column: i32) -> bool {
    model
        .get_model()
        .workbook
        .worksheet(0)
        .unwrap()
        .cell(row, column)
        .is_some()
}

fn style_index(model: &UserModel, row: i32, column: i32) -> i32 {
    model
        .get_model()
        .get_cell_style_index(0, row, column)
        .unwrap()
}

fn num_fmt(model: &UserModel, row: i32, column: i32) -> String {
    model.get_cell_style(0, row, column).unwrap().num_fmt
}

#[test]
fn undo_typed_input_into_empty_cell_drops_its_format() {
    for input in ["5%", "2024-06-01", "$5"] {
        let mut model = new_empty_user_model();
        model.set_user_input(0, 1, 1, input).unwrap();
        let format = num_fmt(&model, 1, 1);
        assert_ne!(format, "general", "{input}");

        model.undo().unwrap();
        assert!(!has_cell(&model, 1, 1), "{input}");
        assert_eq!(num_fmt(&model, 1, 1), "general", "{input}");

        model.redo().unwrap();
        assert_eq!(num_fmt(&model, 1, 1), format, "{input}");
        model.undo().unwrap();

        model.set_user_input(0, 1, 1, "=ROW()*2").unwrap();
        assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "2");
    }
}

#[test]
fn undo_typed_input_into_styled_blank_keeps_its_style() {
    let mut model = new_empty_user_model();
    model
        .update_range_style(&area(1, 1, 1, 1), "font.b", "true")
        .unwrap();
    let bold = style_index(&model, 1, 1);

    model.set_user_input(0, 1, 1, "5%").unwrap();
    assert_eq!(num_fmt(&model, 1, 1), PERCENT);

    model.undo().unwrap();
    assert_eq!(style_index(&model, 1, 1), bold);
    assert_eq!(model.get_cell_content(0, 1, 1).unwrap(), "");

    model.redo().unwrap();
    let style = model.get_cell_style(0, 1, 1).unwrap();
    assert_eq!(style.num_fmt, PERCENT);
    assert!(style.font.b);
}

#[test]
fn undo_typed_input_inherits_the_row_style_again() {
    let mut model = new_empty_user_model();
    model
        .update_range_style(&area(1, 1, LAST_COLUMN, 1), "font.b", "true")
        .unwrap();
    let row_style = style_index(&model, 1, 3);
    assert!(!has_cell(&model, 1, 3));

    model.set_user_input(0, 1, 3, "5%").unwrap();
    model.undo().unwrap();

    assert!(!has_cell(&model, 1, 3));
    assert_eq!(style_index(&model, 1, 3), row_style);
}

#[test]
fn undo_paste_csv_into_empty_cells_drops_their_formats() {
    let mut model = new_empty_user_model();
    model.set_selected_cell(1, 1).unwrap();
    model
        .paste_csv_string(&area(1, 1, 1, 1), "5%\t2024-06-01\n$5\t7")
        .unwrap();
    assert_eq!(num_fmt(&model, 1, 1), PERCENT);

    model.undo().unwrap();
    for (row, column) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
        assert!(!has_cell(&model, row, column), "{row} {column}");
    }

    model.redo().unwrap();
    assert_eq!(num_fmt(&model, 1, 1), PERCENT);
    assert_eq!(num_fmt(&model, 1, 2), "yyyy-mm-dd");
}

#[test]
fn undo_clipboard_paste_into_empty_cell_drops_its_format() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "5%").unwrap();
    model.set_selected_range(1, 1, 1, 1).unwrap();
    let clipboard = model.copy_to_clipboard().unwrap();
    model.set_selected_cell(3, 3).unwrap();
    model
        .paste_from_clipboard(0, (1, 1, 1, 1), &clipboard.data, false)
        .unwrap();
    assert_eq!(num_fmt(&model, 3, 3), PERCENT);

    model.undo().unwrap();
    assert!(!has_cell(&model, 3, 3));

    model.redo().unwrap();
    assert_eq!(num_fmt(&model, 3, 3), PERCENT);
}

#[test]
fn undo_autofill_into_empty_cells_drops_their_formats() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "5%").unwrap();
    model.auto_fill_rows(&area(1, 1, 1, 1), 3).unwrap();
    assert_eq!(num_fmt(&model, 3, 1), PERCENT);

    model.undo().unwrap();
    assert_eq!(num_fmt(&model, 2, 1), "general");
    assert_eq!(num_fmt(&model, 3, 1), "general");
    assert_eq!(model.get_cell_content(0, 3, 1).unwrap(), "");
}

#[test]
fn undo_typed_input_into_spill_cell_keeps_its_style() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 1, "=SEQUENCE(3)").unwrap();
    model
        .update_range_style(&area(2, 1, 1, 1), "font.b", "true")
        .unwrap();
    let bold = style_index(&model, 2, 1);

    model.set_user_input(0, 2, 1, "5%").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "#SPILL!");

    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "2");
    assert_eq!(style_index(&model, 2, 1), bold);

    model.redo().unwrap();
    let style = model.get_cell_style(0, 2, 1).unwrap();
    assert_eq!(style.num_fmt, PERCENT);
    assert!(style.font.b);
}

#[test]
fn undo_array_formula_over_spill_cell_keeps_its_style() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 1, 3, "5%").unwrap();
    model.set_user_input(0, 1, 1, "=SEQUENCE(3)").unwrap();
    model
        .update_range_style(&area(2, 1, 1, 1), "font.b", "true")
        .unwrap();
    let bold = style_index(&model, 2, 1);

    model
        .set_user_array_formula(0, 2, 1, 2, 1, "=C1+1")
        .unwrap();
    let format = num_fmt(&model, 2, 1);
    assert_ne!(format, "general");
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "#SPILL!");

    model.undo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "2");
    assert_eq!(style_index(&model, 2, 1), bold);

    model.redo().unwrap();
    let style = model.get_cell_style(0, 2, 1).unwrap();
    assert_eq!(style.num_fmt, format);
    assert!(style.font.b);
}
