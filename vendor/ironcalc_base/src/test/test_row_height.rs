#![allow(clippy::unwrap_used)]

use crate::constants::LAST_COLUMN;
use crate::expressions::types::Area;
use crate::types::Row;
use crate::UserModel;

fn rows(model: &UserModel) -> Vec<Row> {
    model
        .get_model()
        .workbook
        .worksheet(0)
        .unwrap()
        .rows
        .clone()
}

#[test]
fn undo_height_of_a_row_without_an_entry_leaves_no_entry() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_rows_height(0, 3, 3, 40.0).unwrap();
    let taller = rows(&model);
    assert_eq!(taller.len(), 1);
    model.undo().unwrap();
    assert!(rows(&model).is_empty());
    model.redo().unwrap();
    assert_eq!(rows(&model), taller);
}

#[test]
fn undo_height_restores_the_height_and_flag_of_the_entry() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_rows_height(0, 3, 3, 40.0).unwrap();
    let before = rows(&model);
    model.set_rows_height(0, 3, 3, 60.0).unwrap();
    model.undo().unwrap();
    assert_eq!(rows(&model), before);
}

#[test]
fn undo_height_of_a_hidden_row_keeps_it_hidden_and_at_its_height() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_rows_height(0, 3, 3, 40.0).unwrap();
    model.set_rows_hidden(0, 3, 3, true).unwrap();
    let hidden = rows(&model);
    model.set_rows_height(0, 3, 3, 60.0).unwrap();
    model.undo().unwrap();
    assert_eq!(rows(&model), hidden);
    model.set_rows_hidden(0, 3, 3, false).unwrap();
    assert_eq!(model.get_row_height(0, 3), Ok(40.0));
}

#[test]
fn undo_height_of_a_row_hidden_without_an_entry_keeps_the_hidden_entry() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_rows_hidden(0, 3, 3, true).unwrap();
    let hidden = rows(&model);
    model.set_rows_height(0, 3, 3, 60.0).unwrap();
    model.undo().unwrap();
    assert_eq!(rows(&model), hidden);
}

#[test]
fn undo_height_of_a_styled_row_keeps_its_style() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    let row = Area {
        sheet: 0,
        row: 3,
        column: 1,
        width: LAST_COLUMN,
        height: 1,
    };
    model.update_range_style(&row, "font.b", "true").unwrap();
    let styled = rows(&model);
    assert_eq!(styled.len(), 1);
    model.set_rows_height(0, 3, 3, 60.0).unwrap();
    model.undo().unwrap();
    assert_eq!(rows(&model), styled);
}
