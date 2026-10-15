#![allow(clippy::unwrap_used)]

use crate::test::user_model::util::new_empty_user_model;

#[test]
fn the_next_undo_and_redo_say_which_sheets_get_rows_or_columns_moved() {
    let mut model = new_empty_user_model();
    assert_eq!(model.sheets_moved_by_undo(), Some(vec![]));
    assert_eq!(model.sheets_moved_by_redo(), Some(vec![]));
    model.new_sheet().unwrap();
    assert_eq!(model.sheets_moved_by_undo(), None);
    model.set_user_input(1, 1, 1, "1").unwrap();
    assert_eq!(model.sheets_moved_by_undo(), Some(vec![]));
    model.insert_rows(1, 3, 1).unwrap();
    assert_eq!(model.sheets_moved_by_undo(), Some(vec![1]));
    model.undo().unwrap();
    assert_eq!(model.sheets_moved_by_undo(), Some(vec![]));
    assert_eq!(model.sheets_moved_by_redo(), Some(vec![1]));
    model.redo().unwrap();
    model.delete_columns(0, 2, 1).unwrap();
    assert_eq!(model.sheets_moved_by_undo(), Some(vec![0]));
    model.undo().unwrap();
    model.undo().unwrap();
    model.undo().unwrap();
    assert_eq!(model.sheets_moved_by_undo(), None);
    assert_eq!(model.sheets_moved_by_redo(), Some(vec![]));
}
