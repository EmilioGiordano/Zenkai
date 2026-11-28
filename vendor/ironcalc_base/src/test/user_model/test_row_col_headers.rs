#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;
use crate::UserModel;

#[test]
fn headers_are_shown_by_default_and_toggle_with_undo() {
    let mut model = UserModel::from_model(new_empty_model());
    model.new_sheet().unwrap();

    assert_eq!(model.get_show_row_col_headers(0), Ok(true));
    assert_eq!(model.get_show_row_col_headers(1), Ok(true));
    assert_eq!(
        model.get_show_row_col_headers(2),
        Err("Invalid sheet index".to_string())
    );

    model.set_show_row_col_headers(1, false).unwrap();
    assert_eq!(model.get_show_row_col_headers(1), Ok(false));
    assert_eq!(model.get_show_row_col_headers(0), Ok(true));
    assert_eq!(model.get_show_grid_lines(1), Ok(true));

    model.undo().unwrap();
    assert_eq!(model.get_show_row_col_headers(1), Ok(true));

    model.redo().unwrap();
    let send_queue = model.flush_send_queue();
    let mut model2 = UserModel::from_model(new_empty_model());
    model2.apply_external_diffs(&send_queue).unwrap();
    assert_eq!(model2.get_show_row_col_headers(1), Ok(false));
    assert_eq!(model2.get_show_row_col_headers(0), Ok(true));
}

#[test]
fn deleting_a_sheet_and_undoing_brings_its_headers_back() {
    let mut model = UserModel::from_model(new_empty_model());
    model.new_sheet().unwrap();
    model.set_show_row_col_headers(1, false).unwrap();
    model.delete_sheet(1).unwrap();
    model.undo().unwrap();
    assert_eq!(model.get_show_row_col_headers(1), Ok(false));
}
