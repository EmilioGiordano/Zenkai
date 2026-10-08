#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;
use crate::{Model, Recalculation, UserModel};

fn set(model: &mut Model, cell: &str, value: &str) -> (u32, i32, i32) {
    model._set(cell, value);
    let r = model._parse_reference(cell);
    (r.sheet, r.row, r.column)
}

#[test]
fn only_dependents_are_recalculated() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("A2", "=A1*2");
    model._set("A3", "=A2+1");
    model._set("B1", "=SUM(A1:A3)");
    model._set("C1", "=10");
    model.evaluate_indexed();
    let edited = set(&mut model, "A1", "5");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model.last_recalculation(), Recalculation::Incremental);
    assert_eq!(model._get_text("A2"), "10");
    assert_eq!(model._get_text("A3"), "11");
    assert_eq!(model._get_text("B1"), "26");
}

#[test]
fn edited_formulas_and_volatile_ones() {
    let mut model = new_empty_model();
    model._set("A1", "1");
    model._set("B1", "2");
    model._set("C1", "=A1");
    model._set("D1", "=INDIRECT(\"B1\")*10");
    model.evaluate_indexed();
    let edited = set(&mut model, "C1", "=B1+100");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model._get_text("C1"), "102");
    let edited = set(&mut model, "B1", "3");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model._get_text("C1"), "103");
    assert_eq!(
        model._get_text("D1"),
        "30",
        "INDIRECT is always recalculated"
    );
    let edited = set(&mut model, "A1", "50");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model._get_text("C1"), "103", "A1 no longer feeds C1");
}

#[test]
fn criteria_ranges_are_read_in_the_shape_of_the_sum_range() {
    let mut model = new_empty_model();
    model._set("A3", "x");
    model._set("B3", "7");
    model._set("C1", "=SUMIF(A1:A2,\"x\",B1:B3)");
    model.evaluate_indexed();
    assert_eq!(model._get_text("C1"), "7");
    let edited = set(&mut model, "A3", "y");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model._get_text("C1"), "0");
}

#[test]
fn wide_and_tall_ranges() {
    let mut model = new_empty_model();
    model._set("A1", "=SUM(B2:CZ3)");
    model._set("A2", "=SUM(B:B)");
    model._set("A3", "=SUM($B$2:B3)");
    model.evaluate_indexed();
    let edited = set(&mut model, "CY3", "4");
    model.evaluate_incremental(&[edited]);
    let edited = set(&mut model, "B3", "5");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model.last_recalculation(), Recalculation::Incremental);
    assert_eq!(model._get_text("A1"), "9");
    assert_eq!(model._get_text("A2"), "5");
    assert_eq!(model._get_text("A3"), "5");
}

#[test]
fn circular_reference_made_by_an_edit() {
    let mut model = new_empty_model();
    model._set("A1", "=B1+1");
    model._set("B1", "1");
    model.evaluate_indexed();
    let edited = set(&mut model, "B1", "=A1");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model.last_recalculation(), Recalculation::Full);
    assert_eq!(model._get_text("A1"), "#CIRC!");
    let edited = set(&mut model, "B1", "4");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model._get_text("A1"), "5");
    let edited = set(&mut model, "B1", "6");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model.last_recalculation(), Recalculation::Incremental);
    assert_eq!(model._get_text("A1"), "7");
}

#[test]
fn spills_fall_back_to_a_full_evaluation() {
    let mut model = new_empty_model();
    model._set("A1", "3");
    model._set("B1", "=SEQUENCE(A1)");
    model._set("C1", "=SUM(B1:B5)");
    model.evaluate_indexed();
    assert_eq!(model._get_text("C1"), "6");
    let edited = set(&mut model, "A1", "4");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model.last_recalculation(), Recalculation::Full);
    assert_eq!(model._get_text("C1"), "10");
    let edited = set(&mut model, "B3", "x");
    model.evaluate_incremental(&[edited]);
    assert_eq!(model._get_text("B1"), "#SPILL!");
    let edited = set(&mut model, "D1", "1");
    model.evaluate_incremental(&[edited]);
    assert_eq!(
        model.last_recalculation(),
        Recalculation::Full,
        "a blocked spill may unblock"
    );
}

#[test]
fn user_model_edits_undo_and_redo_are_incremental() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 2, 1, "=A1*2").unwrap();
    model.set_user_input(0, 1, 1, "4").unwrap();
    assert_eq!(
        model.get_model().last_recalculation(),
        Recalculation::Incremental
    );
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "8");
    model.undo().unwrap();
    assert_eq!(
        model.get_model().last_recalculation(),
        Recalculation::Incremental
    );
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "2");
    model.redo().unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "8");
}

#[test]
fn structural_changes_evaluate_the_whole_workbook() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.new_sheet().unwrap();
    model.set_user_input(1, 1, 1, "=Sheet1!A1+1").unwrap();
    assert_eq!(model.get_model().last_recalculation(), Recalculation::Full);
    model.delete_sheet(0).unwrap();
    model.set_user_input(0, 2, 1, "2").unwrap();
    assert_eq!(model.get_model().last_recalculation(), Recalculation::Full);
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "#REF!");
}

#[test]
fn paused_edits_are_recalculated_together() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 2, "=A1+A2").unwrap();
    model.set_user_input(0, 3, 3, "0").unwrap();
    model.pause_evaluation();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 2, 1, "2").unwrap();
    model.resume_evaluation();
    model.evaluate();
    assert_eq!(
        model.get_model().last_recalculation(),
        Recalculation::Incremental
    );
    assert_eq!(model.get_formatted_cell_value(0, 1, 2).unwrap(), "3");
}
