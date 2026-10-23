#![allow(clippy::unwrap_used)]

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

fn style_index(model: &UserModel, row: i32, column: i32) -> i32 {
    model
        .get_model()
        .get_cell_style_index(0, row, column)
        .unwrap()
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
