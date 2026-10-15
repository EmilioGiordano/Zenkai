#![allow(clippy::unwrap_used)]

use crate::incremental::{circular_cells_fit, MAX_CIRCULAR_CELLS};
use crate::{Recalculation, UserModel};

const ROWS: i32 = 12;
const COLUMNS: i32 = 8;

fn texts(model: &UserModel) -> Vec<String> {
    let mut texts = Vec::new();
    for row in 1..=ROWS {
        for column in 1..=COLUMNS {
            texts.push(model.get_formatted_cell_value(0, row, column).unwrap());
        }
    }
    texts
}

fn assert_matches_full_evaluation(model: &UserModel) {
    let mut fresh = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
    fresh.evaluate();
    assert_eq!(fresh.get_model().last_recalculation(), Recalculation::Full);
    assert_eq!(texts(model), texts(&fresh));
}

fn mode(model: &UserModel) -> Recalculation {
    model.get_model().last_recalculation()
}

// C1 and C2 read each other; D1 and E1 read the cycle; A1 and A2 have nothing to do with it.
fn model_with_a_cycle() -> UserModel<'static> {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model.set_user_input(0, 2, 1, "=A1*2").unwrap();
    model.set_user_input(0, 1, 4, "=C1*2").unwrap();
    model.set_user_input(0, 1, 5, "=IFERROR(C2,5)").unwrap();
    model.set_user_input(0, 1, 3, "=C2+1").unwrap();
    model.set_user_input(0, 2, 3, "=C1+1").unwrap();
    assert!(model.get_model().circular_hits > 0);
    assert_eq!(mode(&model), Recalculation::Full);
    model
}

#[test]
fn edits_away_from_a_cycle_are_incremental() {
    let mut model = model_with_a_cycle();
    model.set_user_input(0, 1, 1, "5").unwrap();
    assert_eq!(mode(&model), Recalculation::Incremental);
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "10");
    assert_eq!(model.get_formatted_cell_value(0, 1, 3).unwrap(), "#CIRC!");
    assert_matches_full_evaluation(&model);
}

#[test]
fn edits_that_change_or_read_a_cycle_are_evaluated_in_full() {
    let mut model = model_with_a_cycle();
    // Reads the cycle, and sorts before the cell the cycle was found at.
    model.set_user_input(0, 1, 2, "=C2").unwrap();
    assert_eq!(mode(&model), Recalculation::Full);
    assert_matches_full_evaluation(&model);

    model.set_user_input(0, 1, 4, "7").unwrap();
    assert_eq!(mode(&model), Recalculation::Full);
    assert_matches_full_evaluation(&model);

    model.set_user_input(0, 2, 3, "3").unwrap();
    assert_eq!(mode(&model), Recalculation::Full);
    assert_eq!(model.get_formatted_cell_value(0, 1, 3).unwrap(), "4");
    assert_matches_full_evaluation(&model);

    model.set_user_input(0, 1, 1, "6").unwrap();
    assert_eq!(mode(&model), Recalculation::Incremental);
    assert_matches_full_evaluation(&model);
}

#[test]
fn a_formula_reading_at_run_time_beside_a_cycle_forces_a_full_evaluation() {
    let mut model = model_with_a_cycle();
    model.set_user_input(0, 5, 6, "=INDIRECT(\"C1\")").unwrap();
    model.set_user_input(0, 1, 1, "2").unwrap();
    assert_eq!(mode(&model), Recalculation::Full);
    assert_matches_full_evaluation(&model);
}

#[test]
fn volatile_formulas_that_read_nothing_do_not_force_a_full_evaluation() {
    let mut model = model_with_a_cycle();
    model.set_user_input(0, 5, 6, "=TODAY()-TODAY()").unwrap();
    model.set_user_input(0, 6, 6, "=RAND()*0").unwrap();
    model.set_user_input(0, 1, 1, "2").unwrap();
    assert_eq!(mode(&model), Recalculation::Incremental);
    assert_matches_full_evaluation(&model);
    model.set_user_input(0, 1, 1, "3").unwrap();
    assert_eq!(mode(&model), Recalculation::Incremental);
    assert_matches_full_evaluation(&model);
}

#[test]
fn a_volatile_formula_reading_a_cycle_is_evaluated_in_full() {
    let mut model = model_with_a_cycle();
    model.set_user_input(0, 5, 6, "=NOW()*0+C1").unwrap();
    model.set_user_input(0, 1, 1, "2").unwrap();
    assert_eq!(mode(&model), Recalculation::Full);
    assert_matches_full_evaluation(&model);
}

#[test]
fn the_cycle_cells_kept_have_a_limit() {
    assert!(circular_cells_fit(0));
    assert!(circular_cells_fit(MAX_CIRCULAR_CELLS));
    assert!(!circular_cells_fit(MAX_CIRCULAR_CELLS + 1));
}

struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }

    fn cell(&mut self) -> String {
        let column = char::from(b'A' + u8::try_from(self.below(6)).unwrap());
        format!("{column}{}", 1 + self.below(8))
    }

    fn input(&mut self) -> String {
        match self.below(10) {
            0..=2 => self.below(10).to_string(),
            3 => "x".to_string(),
            4 => String::new(),
            5 => format!("={}+{}", self.cell(), self.cell()),
            6 => format!("=IFERROR({},5)", self.cell()),
            7 => format!("=SUM({}:{})", self.cell(), self.cell()),
            8 => format!("=SEQUENCE(MAX(1,MIN(3,{})))", self.cell()),
            _ => format!("={}*2", self.cell()),
        }
    }
}

#[test]
fn random_edits_with_cycles_and_spills_match_a_full_evaluation() {
    for seed in 0..40u64 {
        let mut random = Random(seed);
        let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
        // Plain cells that nothing reads, as in a workbook of mostly data.
        for row in 1..=ROWS {
            model.set_user_input(0, row, 8, &row.to_string()).unwrap();
        }
        for _ in 0..30 {
            let row = 1 + i32::try_from(random.below(8)).unwrap();
            let column = 1 + i32::try_from(random.below(6)).unwrap();
            let input = random.input();
            // A write over part of an array is refused; the model is left as it was.
            let _refused = model.set_user_input(0, row, column, &input);
            if random.below(6) == 0 {
                let _nothing_to_undo = model.undo();
            }
            assert_matches_full_evaluation(&model);
        }
    }
}
