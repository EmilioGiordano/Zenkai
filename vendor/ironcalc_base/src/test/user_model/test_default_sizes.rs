#![allow(clippy::unwrap_used)]

use crate::test::user_model::util::new_empty_user_model;
use crate::types::DefaultSizes;

fn sizes(row_height: f64, column_width: f64) -> DefaultSizes {
    DefaultSizes {
        row_height: Some(row_height),
        column_width: Some(column_width),
    }
}

#[test]
fn a_new_sheet_has_no_default_sizes() {
    let model = new_empty_user_model();
    assert_eq!(model.get_default_sizes(0), Ok(DefaultSizes::default()));
    assert_eq!(
        model.get_default_sizes(1),
        Err("Invalid sheet index".to_string())
    );
}

#[test]
fn setting_the_sizes_undoes_and_redoes_as_one_step() {
    let mut model = new_empty_user_model();
    model.new_sheet().unwrap();
    model.set_default_sizes(1, sizes(22.5, 20.0)).unwrap();
    assert_eq!(model.get_default_sizes(1), Ok(sizes(22.5, 20.0)));
    assert_eq!(model.get_default_sizes(0), Ok(DefaultSizes::default()));

    model.undo().unwrap();
    assert_eq!(model.get_default_sizes(1), Ok(DefaultSizes::default()));

    model.redo().unwrap();
    assert_eq!(model.get_default_sizes(1), Ok(sizes(22.5, 20.0)));
}

#[test]
fn the_same_sizes_leave_no_undo_step() {
    let mut model = new_empty_user_model();
    model.set_default_sizes(0, DefaultSizes::default()).unwrap();
    assert!(!model.can_undo());
    model.set_default_sizes(0, sizes(30.0, 12.0)).unwrap();
    model.set_default_sizes(0, sizes(30.0, 12.0)).unwrap();
    model.undo().unwrap();
    assert!(!model.can_undo());
}

#[test]
fn an_edit_group_joins_the_sizes_with_the_edits_around_them() {
    let mut model = new_empty_user_model();
    model.begin_edit_group().unwrap();
    model.set_rows_height(0, 3, 3, 40.0).unwrap();
    model.set_default_sizes(0, sizes(30.0, 12.0)).unwrap();
    model.end_edit_group().unwrap();

    model.undo().unwrap();
    assert_eq!(model.get_default_sizes(0), Ok(DefaultSizes::default()));
    assert!(!model.can_undo());
}

#[test]
fn deleting_a_sheet_and_undoing_it_keeps_the_sizes() {
    let mut model = new_empty_user_model();
    model.new_sheet().unwrap();
    model.set_default_sizes(1, sizes(18.0, 11.0)).unwrap();
    model.delete_sheet(1).unwrap();
    model.undo().unwrap();
    assert_eq!(model.get_default_sizes(1), Ok(sizes(18.0, 11.0)));
}

#[test]
fn external_diffs_carry_the_sizes() {
    let mut model = new_empty_user_model();
    model.set_default_sizes(0, sizes(21.0, 9.5)).unwrap();
    let send_queue = model.flush_send_queue();
    let mut other = new_empty_user_model();
    other.apply_external_diffs(&send_queue).unwrap();
    assert_eq!(other.get_default_sizes(0), Ok(sizes(21.0, 9.5)));
}
