#![allow(clippy::unwrap_used)]

use crate::cell::CellValue;
use crate::test::util::new_empty_model;
use crate::Model;

const ROWS: i32 = 1100;

const KEYS: [&str; 20] = [
    "apple",
    "APPLE",
    "Apple",
    "pear",
    "3",
    "3.0",
    "2",
    "2.5",
    "10",
    "-7",
    "TRUE",
    "",
    "=1/0",
    "1/1/2020",
    "=\"3\"",
    "=\"Apple\"",
    "1.9999999999999998",
    "2.0000000000000004",
    "=\"10\"",
    "0.5",
];
const AMOUNTS: [&str; 7] = [
    "0.1",
    "2",
    "-3",
    "679.8711",
    "1000000000000000.3",
    "x",
    "=1/0",
];
const SECOND: [&str; 6] = ["a", "b", "1", "2", "3", "A"];
const CRITERIA: [&str; 22] = [
    "\"apple\"",
    "\"=APPLE\"",
    "3",
    "\"3\"",
    "2",
    "2.5",
    "\"2.5\"",
    "\"1/1/2020\"",
    "DATE(2020,1,1)",
    "\"pear\"",
    "\">2\"",
    "\"a*\"",
    "\"\"",
    "TRUE",
    "\"#DIV/0!\"",
    "-7",
    "10",
    "\"10\"",
    "1.9999999999999998",
    "2.0000000000000004",
    "0.5",
    "\"<>apple\"",
];
const SECOND_CRITERIA: [&str; 5] = ["\"a\"", "1", "\">1\"", "\"b\"", "3"];

fn next(state: &mut u64) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 16) as usize
}

fn pick<'a>(state: &mut u64, from: &[&'a str]) -> &'a str {
    from[next(state) % from.len()]
}

fn data(state: &mut u64) -> Model<'static> {
    let mut model = new_empty_model();
    for row in 1..=ROWS {
        model._set(&format!("A{row}"), pick(state, &KEYS));
        model._set(&format!("B{row}"), pick(state, &AMOUNTS));
        model._set(&format!("C{row}"), pick(state, &SECOND));
    }
    model
}

fn formula(state: &mut u64) -> String {
    let first = pick(state, &CRITERIA);
    let second = pick(state, &SECOND_CRITERIA);
    let (a, b, c) = ("$A$1:$A$1100", "$B$1:$B$1100", "$C$1:$C$1100");
    match next(state) % 6 {
        0 => format!("=SUMIFS({b},{a},{first})"),
        1 => format!("=COUNTIFS({a},{first},{c},{second})"),
        2 => format!("=AVERAGEIFS({b},{a},{first},{c},{second})"),
        3 => format!("=SUMIF({a},{first},{b})"),
        4 => format!("=COUNTIF({a},{first})"),
        _ => format!("=SUMIFS({b},{c},{second},{a},{first})"),
    }
}

// Formulas that share their areas are answered through indexes from the second on; each
// alone in the workbook reads every cell. Both must agree on every value.
#[test]
fn indexed_criteria_give_the_results_of_a_full_read() {
    let mut state = 0x2545_F491_4F6C_DD1D;
    for _ in 0..6 {
        let mut model = data(&mut state);
        let bytes = model.to_bytes();
        let formulas: Vec<String> = (0..40).map(|_| formula(&mut state)).collect();
        for (row, formula) in (1..).zip(&formulas) {
            model._set(&format!("E{row}"), formula);
        }
        model.evaluate();
        for (row, formula) in (1..).zip(&formulas) {
            let mut alone = Model::from_bytes(&bytes, "en").unwrap();
            alone._set("E1", formula);
            alone.evaluate();
            let shared = model.get_cell_value_by_index(0, row, 5).unwrap();
            let expected = alone.get_cell_value_by_index(0, 1, 5).unwrap();
            match (&shared, &expected) {
                (CellValue::Number(a), CellValue::Number(b)) => {
                    assert_eq!(a.to_bits(), b.to_bits(), "{formula}")
                }
                _ => assert_eq!(shared, expected, "{formula}"),
            }
        }
    }
}
