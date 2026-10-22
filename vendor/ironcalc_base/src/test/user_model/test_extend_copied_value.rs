#![allow(clippy::unwrap_used)]

use crate::expressions::types::CellReferenceIndex;
use crate::test::user_model::util::new_empty_user_model;

const INPUTS: [&str; 16] = [
    "=B1*2",
    "=$A1+B$2",
    "=SUM(A1:B2)",
    "-A1+2",
    "+B1",
    "1",
    "-3.5",
    "text",
    "'text",
    "1/2/2024",
    "5%",
    "TRUE",
    "#N/A",
    "=",
    "",
    "007",
];

// Extending the cell that holds these would give its parsed value (`45293`, `0.05`, `7`,
// `text`); Excel fills what was typed, so these come back unchanged.
const FILLED_AS_TYPED: [&str; 4] = ["'text", "1/2/2024", "5%", "007"];

#[test]
fn extending_a_typed_value_matches_extending_the_cell_that_holds_it() {
    let mut differing = Vec::new();
    for input in INPUTS {
        let mut model = new_empty_user_model();
        model.set_user_input(0, 1, 1, input).unwrap();
        let source = CellReferenceIndex {
            sheet: 0,
            row: 1,
            column: 1,
        };
        let target = CellReferenceIndex {
            sheet: 0,
            row: 4,
            column: 3,
        };
        let from_cell = model.get_model().extend_to(0, 1, 1, 4, 3).unwrap();
        let from_text = model.extend_copied_value(input, &source, &target).unwrap();
        let expected = if FILLED_AS_TYPED.contains(&input) {
            input
        } else {
            &from_cell
        };
        if from_text != expected {
            differing.push((input, from_text, expected.to_string()));
        }
    }
    assert!(differing.is_empty(), "{differing:?}");
}

#[test]
fn extending_a_formula_above_or_left_of_the_sheet_gives_a_ref_error() {
    let mut model = new_empty_user_model();
    let source = CellReferenceIndex {
        sheet: 0,
        row: 4,
        column: 3,
    };
    let target = CellReferenceIndex {
        sheet: 0,
        row: 1,
        column: 1,
    };
    let result = model.extend_copied_value("=A4", &source, &target).unwrap();
    assert_eq!(result, "=#REF!");
}
