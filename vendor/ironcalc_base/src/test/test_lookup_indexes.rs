#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;
use crate::{Model, UserModel};

const TABLE_ROWS: i32 = 60;
const LOOKUPS: i32 = 80;

// Values typed into the searched vectors: duplicates, case and Unicode case pairs, numbers
// around the magnitudes where equality stops being exact, booleans, errors and blanks.
const TABLE_VALUES: [&str; 34] = [
    "P0098",
    "p0098",
    "C00477",
    "c00477",
    "Straße",
    "STRASSE",
    "ss",
    "ΟΔΟΣ",
    "οδος",
    "İstanbul",
    "i̇stanbul",
    "a*b",
    "a?b",
    "a~b",
    "1",
    "0.9999999999999999",
    "1.9999999999999998",
    "2",
    "0",
    "-0",
    "1E-17",
    "0.3",
    "0.30000000000000004",
    "123456789012345678",
    "123456789012345680",
    "-7.25",
    "1048576",
    "TRUE",
    "FALSE",
    "#N/A",
    "#DIV/0!",
    "",
    "",
    "P0099",
];

// Lookup values: everything above plus texts with wildcards and values found nowhere.
const LOOKUP_VALUES: [&str; 14] = [
    "a*",
    "?0098",
    "a~*b",
    "a~?b",
    "*",
    "missing",
    "3",
    "1E-300",
    "0.1",
    "-7.25000000000001",
    "strasse",
    "ς",
    "TRUE",
    "",
];

struct Random(u64);

impl Random {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound
    }

    fn pick<'a>(&mut self, values: &[&'a str]) -> &'a str {
        values[self.below(values.len())]
    }
}

fn lookup_value(random: &mut Random) -> &'static str {
    if random.below(3) == 0 {
        random.pick(&LOOKUP_VALUES)
    } else {
        random.pick(&TABLE_VALUES)
    }
}

// Column A and row 1 of Sheet2 hold the searched values, column B and row 2 what is
// returned; D holds lookup values and E to K one lookup function each.
fn fill(model: &mut Model, random: &mut Random) {
    model.new_sheet();
    for row in 1..=TABLE_ROWS {
        let value = random.pick(&TABLE_VALUES);
        if !value.is_empty() {
            model._set(&format!("A{row}"), value);
            model.set_user_input(1, 1, row, value.to_string()).unwrap();
        }
        model._set(&format!("B{row}"), &format!("row {row}"));
        model
            .set_user_input(1, 2, row, format!("column {row}"))
            .unwrap();
    }
    for row in 1..=LOOKUPS {
        let value = lookup_value(random);
        if !value.is_empty() {
            model._set(&format!("D{row}"), value);
        }
        set_lookups(model, row);
    }
}

fn set_lookups(model: &mut Model, row: i32) {
    let last = TABLE_ROWS;
    let formulas = [
        format!("=VLOOKUP(D{row},$A$1:$B${last},2,FALSE)"),
        format!("=MATCH(D{row},$A$1:$A${last},0)"),
        format!("=XLOOKUP(D{row},$A$1:$A${last},$B$1:$B${last},\"none\")"),
        format!("=XLOOKUP(D{row},$A$1:$A${last},$B$1:$B${last},\"none\",0,-1)"),
        format!("=MATCH(D{row},A:A,0)"),
        format!("=HLOOKUP(D{row},Sheet2!$A$1:$BH$2,2,FALSE)"),
        format!("=MATCH(D{row},Sheet2!$A$1:$BH$1,0)"),
    ];
    for (column, formula) in ["E", "F", "G", "H", "I", "J", "K"].iter().zip(formulas) {
        model._set(&format!("{column}{row}"), &formula);
    }
}

fn results(model: &Model) -> Vec<String> {
    (1..=LOOKUPS)
        .flat_map(|row| {
            ["E", "F", "G", "H", "I", "J", "K"]
                .map(|column| model._get_text(&format!("{column}{row}")))
        })
        .collect()
}

#[test]
fn indexed_lookups_match_cell_by_cell_search() {
    for seed in 0..12 {
        let mut indexed = new_empty_model();
        let mut searched = new_empty_model();
        searched.lookup_indexes.set_enabled(false);
        fill(&mut indexed, &mut Random(seed));
        fill(&mut searched, &mut Random(seed));
        indexed.evaluate();
        searched.evaluate();
        assert_eq!(results(&indexed), results(&searched), "seed {seed}");

        let mut random = Random(seed + 1000);
        for _ in 0..5 {
            let row = random.below(TABLE_ROWS as usize) as i32 + 1;
            let value = random.pick(&TABLE_VALUES);
            for model in [&mut indexed, &mut searched] {
                model._set(&format!("A{row}"), value);
                model.evaluate();
            }
            assert_eq!(
                results(&indexed),
                results(&searched),
                "seed {seed} edit A{row}"
            );
        }
    }
}

#[test]
fn an_edit_between_evaluations_is_seen() {
    let mut model = new_empty_model();
    for row in 1..=10 {
        model._set(&format!("A{row}"), &format!("K{row}"));
        model._set(&format!("B{row}"), &row.to_string());
    }
    for row in 1..=5 {
        model._set(&format!("C{row}"), "=VLOOKUP(\"k7\",$A$1:$B$10,2,FALSE)");
    }
    model.evaluate();
    assert_eq!(model._get_text("C5"), "7");
    model._set("A3", "k7");
    model.evaluate();
    assert_eq!(model._get_text("C5"), "3");
    model._set("A3", "x");
    model._set("A7", "x");
    model.evaluate();
    assert_eq!(model._get_text("C5"), "#N/A");
}

#[test]
fn a_formula_in_the_searched_vector_is_read_as_it_evaluates() {
    let mut model = new_empty_model();
    for row in 1..=10 {
        model._set(&format!("A{row}"), &format!("K{row}"));
        model._set(&format!("B{row}"), &row.to_string());
    }
    model._set("A4", "=D1");
    model._set("D1", "=\"k\"&D2");
    model._set("D2", "=3*3");
    for row in 1..=5 {
        model._set(&format!("C{row}"), "=VLOOKUP(\"k9\",$A$1:$B$10,2,FALSE)");
    }
    model.evaluate();
    assert_eq!(model._get_text("C5"), "4");
}

#[test]
fn a_spill_into_the_searched_vector_is_seen() {
    let mut indexed = new_empty_model();
    let mut searched = new_empty_model();
    searched.lookup_indexes.set_enabled(false);
    for model in [&mut indexed, &mut searched] {
        for row in 1..=4 {
            model._set(&format!("A{row}"), &format!("K{row}"));
        }
        // Spills 2, 3 and 4 into A6:A8, which were blank when the rows above searched them.
        model._set("A5", "=SEQUENCE(4)");
        for row in 1..=12 {
            model._set(
                &format!("B{row}"),
                "=XLOOKUP(3,$A$6:$A$12,$A$6:$A$12,\"none\")",
            );
            model._set(&format!("C{row}"), "=MATCH(FALSE,$A$6:$A$12,0)");
            model._set(&format!("D{row}"), "=VLOOKUP(FALSE,$A$6:$A$12,1,FALSE)");
        }
        model.evaluate();
    }
    for row in 1..=12 {
        for column in ["B", "C", "D"] {
            let cell = format!("{column}{row}");
            assert_eq!(
                indexed._get_text(&cell),
                searched._get_text(&cell),
                "{cell}"
            );
        }
    }
    assert_eq!(indexed._get_text("B12"), "3");
}

// `compare_values` reads a blank as FALSE and as "", so VLOOKUP and XLOOKUP find them in a
// blank; MATCH does not. The cell by cell search does the same; kept until it changes.
#[test]
fn blanks_equal_false_and_empty_text_as_in_the_cell_by_cell_search() {
    let mut model = new_empty_model();
    model._set("A1", "x");
    model._set("A3", "FALSE");
    for row in 1..=3 {
        model._set(&format!("B{row}"), "=VLOOKUP(FALSE,$A$1:$A$4,1,FALSE)");
        model._set(&format!("C{row}"), "=MATCH(FALSE,$A$1:$A$4,0)");
        model._set(
            &format!("D{row}"),
            "=XLOOKUP(\"\",$A$1:$A$4,$A$1:$A$4,\"none\")",
        );
        model._set(
            &format!("E{row}"),
            "=XLOOKUP(FALSE,$A$1:$A$4,$A$1:$A$4,\"none\",0,-1)",
        );
    }
    model.evaluate();
    assert_eq!(model._get_text("B3"), "0");
    assert_eq!(model._get_text("C3"), "3");
    assert_eq!(model._get_text("D3"), "0");
    assert_eq!(model._get_text("E3"), "0");
}

fn user_model() -> UserModel<'static> {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    for row in 1..=TABLE_ROWS {
        model
            .set_user_input(0, row, 1, &format!("Code{}", row % 17))
            .unwrap();
        model.set_user_input(0, row, 2, &row.to_string()).unwrap();
    }
    for row in 1..=20 {
        let code = format!("CODE{}", row % 19);
        model.set_user_input(0, row, 4, &code).unwrap();
        model
            .set_user_input(0, row, 5, &format!("=VLOOKUP(D{row},$A$1:$B$70,2,FALSE)"))
            .unwrap();
        model
            .set_user_input(0, row, 6, &format!("=MATCH(D{row},$A$1:$A$70,0)"))
            .unwrap();
    }
    model
}

fn user_results(model: &UserModel) -> Vec<String> {
    (1..=20)
        .flat_map(|row| {
            [5, 6].map(|column| model.get_formatted_cell_value(0, row, column).unwrap())
        })
        .collect()
}

#[test]
fn edits_undo_redo_and_row_moves_are_seen() {
    let mut indexed = user_model();
    let mut searched = user_model();
    searched.model.lookup_indexes.set_enabled(false);
    let steps: [&dyn Fn(&mut UserModel); 7] = [
        &|model| model.set_user_input(0, 3, 1, "code5").unwrap(),
        &|model| model.undo().unwrap(),
        &|model| model.redo().unwrap(),
        &|model| model.insert_rows(0, 2, 3).unwrap(),
        &|model| model.delete_rows(0, 1, 4).unwrap(),
        &|model| model.undo().unwrap(),
        &|model| model.set_user_input(0, 10, 1, "CODE18").unwrap(),
    ];
    for (index, step) in steps.iter().enumerate() {
        step(&mut indexed);
        step(&mut searched);
        assert_eq!(
            user_results(&indexed),
            user_results(&searched),
            "step {index}"
        );
    }
}
