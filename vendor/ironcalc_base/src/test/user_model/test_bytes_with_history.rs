#![allow(clippy::unwrap_used)]

use crate::{test::util::new_empty_model, UserModel};

fn edited() -> UserModel<'static> {
    let mut model = UserModel::from_model(new_empty_model());
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 2, 1, "=A1*2").unwrap();
    model.set_user_input(0, 1, 1, "5").unwrap();
    model.undo().unwrap();
    model
}

#[test]
fn history_survives_the_round_trip() {
    let model = edited();
    let bytes = model.to_bytes_with_history().unwrap();
    let mut restored = UserModel::from_bytes_with_history(&bytes, "en").unwrap();
    restored.evaluate();
    assert_eq!(
        restored.get_formatted_cell_value(0, 2, 1),
        Ok("2".to_string())
    );
    assert!(restored.can_undo());
    assert!(restored.can_redo());
    restored.redo().unwrap();
    assert_eq!(
        restored.get_formatted_cell_value(0, 2, 1),
        Ok("10".to_string())
    );
    restored.undo().unwrap();
    restored.undo().unwrap();
    restored.undo().unwrap();
    assert_eq!(
        restored.get_formatted_cell_value(0, 1, 1),
        Ok("".to_string())
    );
    assert!(!restored.can_undo());
}

#[test]
fn plain_bytes_still_drop_the_history() {
    let model = edited();
    let restored = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
    assert!(!restored.can_undo());
    assert!(!restored.can_redo());
}

#[test]
fn bytes_that_are_not_a_model_are_an_error() {
    assert!(UserModel::from_bytes_with_history(b"not a model", "en").is_err());
    let bytes = edited().to_bytes_with_history().unwrap();
    assert!(UserModel::from_bytes_with_history(&bytes[..bytes.len() / 2], "en").is_err());
}

#[test]
fn a_model_in_the_middle_of_an_edit_is_not_dumped() {
    let mut model = edited();
    model.pause_evaluation();
    assert!(model.to_bytes_with_history().is_err());
    model.resume_evaluation();
    assert!(model.to_bytes_with_history().is_ok());
}

#[test]
fn the_pieces_given_to_a_sink_join_into_the_same_bytes() {
    let model = edited();
    let mut pieces = Vec::new();
    model
        .write_with_history(&mut |chunk| {
            pieces.push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
    // A length and a section for the workbook, the undo history and the redo history.
    assert_eq!(pieces.len(), 6);
    assert_eq!(pieces.concat(), model.to_bytes_with_history().unwrap());
}

#[test]
fn a_sink_that_fails_stops_the_dump() {
    let model = edited();
    let mut calls = 0;
    let result = model.write_with_history(&mut |_| {
        calls += 1;
        Err("disk full".to_string())
    });
    assert_eq!(result, Err("disk full".to_string()));
    assert_eq!(calls, 1);
}
