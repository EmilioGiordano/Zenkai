#![allow(clippy::unwrap_used)]

use crate::cell::CellValue;
use crate::{Recalculation, UserModel};

const ROWS: i32 = 3000;

// Region in A, a decimal amount in B, a formula doubling it in C, and in column E totals
// over the 3000 rows: criteria from a cell, a literal, a wildcard, two criteria, counts.
fn model() -> UserModel<'static> {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.pause_evaluation();
    for row in 1..=ROWS {
        let region = ["North", "south", "EAST"][(row % 3) as usize];
        model.set_user_input(0, row, 1, region).unwrap();
        let amount = format!("{}.{}", row % 97, row % 13);
        model.set_user_input(0, row, 2, &amount).unwrap();
        model
            .set_user_input(0, row, 3, &format!("=B{row}*1.21"))
            .unwrap();
    }
    model.set_user_input(0, 1, 4, "north").unwrap();
    let totals = [
        "=SUMIF($A$1:$A$3000,D1,$C$1:$C$3000)",
        "=SUMIF($A$1:$A$3000,\"south\",$B$1:$B$3000)",
        "=SUMIFS($C$1:$C$3000,$A$1:$A$3000,\"e*\",$B$1:$B$3000,\">20\")",
        "=COUNTIF($A$1:$A$3000,D1)",
        "=COUNTIFS($A$1:$A$3000,\"<>south\",$B$1:$B$3000,\"<=50.5\")",
        "=SUMIF($B$1:$B$3000,\">10\")",
        "=IF($D$2,SUMIF($A$1:$A$3000,\"EAST\",$C$1:$C$3000),0)",
    ];
    for (row, formula) in (1..).zip(totals) {
        model.set_user_input(0, row, 5, formula).unwrap();
    }
    model.set_user_input(0, 2, 4, "TRUE").unwrap();
    model.resume_evaluation();
    model.evaluate();
    model
}

fn totals(model: &UserModel) -> Vec<CellValue> {
    (1..=7)
        .map(|row| {
            model
                .get_model()
                .get_cell_value_by_index(0, row, 5)
                .unwrap()
        })
        .collect()
}

// Bit for bit, as every SUMIF rounds its exact sum once.
fn check(model: &UserModel) {
    let mut fresh = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
    fresh.evaluate();
    assert_eq!(totals(model), totals(&fresh));
}

fn edit(model: &mut UserModel, row: i32, column: i32, value: &str) {
    model.set_user_input(0, row, column, value).unwrap();
    assert_eq!(
        model.get_model().last_recalculation(),
        Recalculation::Incremental
    );
    check(model);
}

#[test]
fn edits_update_kept_totals_by_delta() {
    let mut model = model();
    check(&model);
    let before = model.get_model().criteria_deltas();
    edit(&mut model, 10, 2, "61.37");
    edit(&mut model, 11, 1, "east");
    edit(&mut model, 12, 1, "North");
    edit(&mut model, 13, 3, "7.5");
    edit(&mut model, 14, 2, "text");
    edit(&mut model, 15, 2, "");
    assert!(model.get_model().criteria_deltas() >= before + 6 * 4);
    model.undo().unwrap();
    model.undo().unwrap();
    check(&model);
    model.redo().unwrap();
    check(&model);
}

#[test]
fn a_criterion_from_a_cell_keys_a_new_total() {
    let mut model = model();
    edit(&mut model, 1, 4, "south");
    edit(&mut model, 20, 1, "SOUTH");
    edit(&mut model, 1, 4, "north");
    edit(&mut model, 21, 1, "north");
}

// The SUMIF is not evaluated while D2 is FALSE, so its kept total misses the edit of C30.
#[test]
fn a_total_its_formula_stopped_reading_is_dropped() {
    let mut model = model();
    edit(&mut model, 2, 4, "FALSE");
    edit(&mut model, 30, 3, "1000");
    edit(&mut model, 2, 4, "TRUE");
}

#[test]
fn an_error_in_the_sum_range_reads_every_cell_again() {
    let mut model = model();
    edit(&mut model, 33, 3, "=1/0");
    assert_eq!(model.get_formatted_cell_value(0, 1, 5).unwrap(), "#DIV/0!");
    edit(&mut model, 33, 3, "2");
    edit(&mut model, 36, 3, "=NA()");
    edit(&mut model, 36, 3, "3");
}

#[test]
fn many_changed_cells_read_every_cell_again() {
    let mut model = model();
    let before = model.get_model().criteria_deltas();
    model
        .range_clear_contents(&crate::expressions::types::Area {
            sheet: 0,
            row: 1,
            column: 2,
            width: 1,
            height: ROWS / 4,
        })
        .unwrap();
    assert_eq!(model.get_model().criteria_deltas(), before);
    check(&model);
}

#[test]
fn whole_column_ranges_follow_the_used_area() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.pause_evaluation();
    for row in 1..=2000 {
        model
            .set_user_input(0, row, 1, if row % 2 == 0 { "x" } else { "y" })
            .unwrap();
        model.set_user_input(0, row, 2, "0.1").unwrap();
    }
    model
        .set_user_input(0, 1, 4, "=COUNTIF(A:A,\"x\")")
        .unwrap();
    model
        .set_user_input(0, 2, 4, "=SUMIF(A:A,\"x\",B:B)")
        .unwrap();
    model.set_user_input(0, 3, 4, "=COUNTIF(A:A,\"\")").unwrap();
    model.resume_evaluation();
    model.evaluate();
    for (row, value) in [(10, "y"), (2500, "x"), (2600, "0.3"), (2500, "")] {
        model.set_user_input(0, row, 1, value).unwrap();
        model.set_user_input(0, row, 2, "0.7").unwrap();
        let mut fresh = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
        fresh.evaluate();
        for row in 1..=3 {
            assert_eq!(
                model.get_model().get_cell_value_by_index(0, row, 4),
                fresh.get_model().get_cell_value_by_index(0, row, 4)
            );
        }
    }
}

#[test]
fn ranges_of_different_shapes_are_read_every_time() {
    let mut model = model();
    model
        .set_user_input(0, 8, 5, "=SUMIF($A$1:$A$3000,\"north\",$C$1:$C$10)")
        .unwrap();
    let before = model.get_model().criteria_deltas();
    model.set_user_input(0, 2000, 3, "5").unwrap();
    // The three totals over C1:C3000; the one over C1:C10 is read again.
    assert_eq!(model.get_model().criteria_deltas(), before + 3);
    let mut fresh = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
    fresh.evaluate();
    assert_eq!(
        model.get_model().get_cell_value_by_index(0, 8, 5),
        fresh.get_model().get_cell_value_by_index(0, 8, 5)
    );
}
