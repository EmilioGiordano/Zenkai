#![allow(clippy::unwrap_used)]

use crate::{Recalculation, UserModel};

const ROWS: i32 = 3000;
const REGIONS: [&str; 3] = ["North", "south", "EAST"];

// Rows of (region, amount) in A and B; C doubles B through a formula, so the summed
// range holds formulas that an edit recalculates.
fn model() -> UserModel<'static> {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.pause_evaluation();
    for row in 1..=ROWS {
        let region = REGIONS[(row % 3) as usize];
        model.set_user_input(0, row, 1, region).unwrap();
        model
            .set_user_input(0, row, 2, &(row % 7).to_string())
            .unwrap();
        model
            .set_user_input(0, row, 3, &format!("=B{row}*2"))
            .unwrap();
    }
    for (index, region) in (1..).zip(REGIONS) {
        let last = ROWS;
        model
            .set_user_input(
                0,
                index,
                5,
                &format!("=SUMIF($A$1:$A${last},\"{region}\",$C$1:$C${last})"),
            )
            .unwrap();
        model
            .set_user_input(
                0,
                index,
                6,
                &format!("=COUNTIF($A$1:$A${last},\"{region}\")"),
            )
            .unwrap();
        model
            .set_user_input(
                0,
                index,
                7,
                &format!(
                    "=SUMIFS($C$1:$C${last},$A$1:$A${last},\"{region}\",$B$1:$B${last},\">2\")"
                ),
            )
            .unwrap();
    }
    model.resume_evaluation();
    model.evaluate();
    model
}

fn expected(model: &UserModel, region: &str) -> (f64, f64, f64) {
    let mut sum = 0.0;
    let mut count = 0.0;
    let mut sum_over_two = 0.0;
    for row in 1..=ROWS {
        let name = model.get_formatted_cell_value(0, row, 1).unwrap();
        if name.to_lowercase() != region.to_lowercase() {
            continue;
        }
        let amount: f64 = model
            .get_formatted_cell_value(0, row, 2)
            .unwrap()
            .parse()
            .unwrap();
        sum += amount * 2.0;
        count += 1.0;
        if amount > 2.0 {
            sum_over_two += amount * 2.0;
        }
    }
    (sum, count, sum_over_two)
}

fn check(model: &UserModel) {
    for (index, region) in (1..).zip(REGIONS) {
        let read = |column| -> f64 {
            model
                .get_formatted_cell_value(0, index, column)
                .unwrap()
                .parse()
                .unwrap()
        };
        assert_eq!(
            (read(5), read(6), read(7)),
            expected(model, region),
            "{region}"
        );
    }
}

#[test]
fn calls_sharing_a_range_match_a_cell_by_cell_count() {
    let mut model = model();
    check(&model);
    model.set_user_input(0, 10, 2, "6").unwrap();
    assert_eq!(
        model.get_model().last_recalculation(),
        Recalculation::Incremental
    );
    check(&model);
    model.set_user_input(0, 11, 1, "north").unwrap();
    check(&model);
    model.undo().unwrap();
    model.undo().unwrap();
    check(&model);
}

#[test]
fn a_spill_into_a_shared_range_is_seen() {
    let mut model = model();
    model.set_user_input(0, 1, 9, "=SEQUENCE(3)").unwrap();
    model
        .set_user_input(0, 5, 10, "=SUMIF($I$1:$I$3000,\">0\",$I$1:$I$3000)")
        .unwrap();
    model
        .set_user_input(0, 6, 10, "=SUMIF($I$1:$I$3000,\">0\",$I$1:$I$3000)")
        .unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 5, 10).unwrap(), "6");
    assert_eq!(model.get_formatted_cell_value(0, 6, 10).unwrap(), "6");
    model.set_user_input(0, 1, 9, "=SEQUENCE(1)").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 5, 10).unwrap(), "1");
    assert_eq!(model.get_formatted_cell_value(0, 6, 10).unwrap(), "1");
    model.set_user_input(0, 1, 9, "=1/0").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 5, 10).unwrap(), "0");
    assert_eq!(model.get_formatted_cell_value(0, 6, 10).unwrap(), "0");
}

#[test]
fn a_criteria_range_inside_a_spill_whose_anchor_is_outside() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 2, "=SEQUENCE(2000)").unwrap();
    model
        .set_user_input(0, 1, 4, "=COUNTIF($B$5:$B$1504,\">1000\")")
        .unwrap();
    model
        .set_user_input(0, 2, 4, "=SUMIF($B$5:$B$1504,\">1000\")")
        .unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 4).unwrap(), "504");
    assert_eq!(model.get_formatted_cell_value(0, 2, 4).unwrap(), "631260");
    model.set_user_input(0, 1, 2, "=SEQUENCE(1000)").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 4).unwrap(), "0");
    assert_eq!(model.get_formatted_cell_value(0, 2, 4).unwrap(), "0");
    model.set_user_input(0, 1, 2, "=SEQUENCE(1200)*2").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 4).unwrap(), "700");
}
