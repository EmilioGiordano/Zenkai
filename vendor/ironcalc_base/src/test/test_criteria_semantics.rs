#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;
use crate::Model;

// A1:A10 hold text in several cases, a wildcard character as text, a number and text that
// looks like it, a boolean and a blank; B1:B10 hold 1, 2, 4, ... 512.
fn model() -> Model<'static> {
    let mut model = new_empty_model();
    let values = [
        "apple", "Apple", "APPLE", "banana", "b*n", "5", "'5", "TRUE", "", "ÉCLAIR",
    ];
    for (row, value) in (1..).zip(values) {
        if !value.is_empty() {
            model._set(&format!("A{row}"), value);
        }
        model._set(&format!("B{row}"), &(1 << (row - 1)).to_string());
    }
    model
}

fn count(criterion: &str) -> String {
    let mut model = model();
    model._set("C1", &format!("=COUNTIF(A1:A10,{criterion})"));
    model.evaluate();
    model._get_text("C1")
}

fn sum(criterion: &str) -> String {
    let mut model = model();
    model._set("C1", &format!("=SUMIF(A1:A10,{criterion},B1:B10)"));
    model.evaluate();
    model._get_text("C1")
}

#[test]
fn text_matches_ignoring_case() {
    assert_eq!(count("\"apple\""), "3");
    assert_eq!(count("\"APPLE\""), "3");
    assert_eq!(sum("\"Apple\""), "7");
    assert_eq!(count("\"éclair\""), "1");
}

#[test]
fn wildcards() {
    assert_eq!(count("\"a*\""), "3");
    assert_eq!(count("\"?????\""), "3");
    assert_eq!(count("\"b*\""), "2");
    assert_eq!(count("\"b~*n\""), "1");
    assert_eq!(count("\"*\""), "7");
    assert_eq!(count("\"<>a*\""), "7");
    assert_eq!(count("\"*CLAIR\""), "1");
}

#[test]
fn comparison_operators_on_text() {
    assert_eq!(count("\">b\""), "3");
    assert_eq!(count("\"<b\""), "4");
    assert_eq!(count("\"<=b\""), "4");
    assert_eq!(count("\">=banana\""), "2");
    assert_eq!(count("\"<=banana\""), "6");
    assert_eq!(count("\"<>apple\""), "7");
}

#[test]
fn numbers_and_text_that_looks_like_numbers() {
    assert_eq!(count("5"), "2");
    assert_eq!(count("\"5\""), "2");
    assert_eq!(count("\">4\""), "1");
    assert_eq!(sum("\">=5\""), "32");
}

#[test]
fn booleans() {
    assert_eq!(count("TRUE"), "1");
    assert_eq!(count("\"TRUE\""), "1");
}

#[test]
fn empty_cells() {
    assert_eq!(count("\"\""), "1");
    assert_eq!(count("\"<>\""), "9");
    assert_eq!(sum("\"\""), "256");
}
