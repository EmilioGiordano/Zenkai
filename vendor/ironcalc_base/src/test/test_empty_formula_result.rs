#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

// A formula that evaluates to an empty cell holds 0, as in Excel, whichever cell happens
// to evaluate it first.

#[test]
fn read_after_the_formula() {
    let mut model = new_empty_model();
    model._set("B1", "=A1");
    model._set("C1", "=ISBLANK(B1)");
    model._set("D1", "=COUNTIF(B1:B1,\"<5\")");
    model.evaluate();
    assert_eq!(model._get_text("B1"), "0");
    assert_eq!(model._get_text("C1"), "FALSE");
    assert_eq!(model._get_text("D1"), "1");
}

#[test]
fn read_before_the_formula() {
    let mut model = new_empty_model();
    model._set("A1", "=ISBLANK(B1)");
    model._set("A2", "=COUNTIF(B1:B1,\"<5\")");
    model._set("B1", "=C1");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "FALSE");
    assert_eq!(model._get_text("A2"), "1");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "FALSE");
    assert_eq!(model._get_text("A2"), "1");
}

#[test]
fn array_whose_first_value_is_empty() {
    let mut model = new_empty_model();
    model._set("A1", "=COUNTIF(C1:C1,\"<5\")");
    model._set("B3", "5");
    model._set("C1", "=B2:B3");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "0");
    assert_eq!(model._get_text("C2"), "5");
    assert_eq!(model._get_text("A1"), "1");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "1");
}

#[test]
fn array_read_by_an_earlier_array() {
    let mut model = new_empty_model();
    model._set("A1", "=SEQUENCE(COUNTIF(C1:C1,\"<5\")+1)");
    model._set("B3", "5");
    model._set("C1", "=B2:B3");
    model.evaluate();
    assert_eq!(model._get_text("A2"), "2");
}
