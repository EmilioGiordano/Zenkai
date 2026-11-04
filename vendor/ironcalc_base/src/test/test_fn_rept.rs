#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

#[test]
fn simple_cases() {
    let mut model = new_empty_model();
    model._set("A1", "Well");

    model._set("B1", "=REPT(A1, 3)");

    model.evaluate();

    assert_eq!(model._get_text("B1"), *"WellWellWell");
}

#[test]
fn wrong_number_of_arguments() {
    let mut model = new_empty_model();
    model._set("A1", "Well");

    model._set("B1", "=REPT(A1)");
    model._set("B2", "=REPT(A1,3,1)");

    model.evaluate();

    assert_eq!(model._get_text("B1"), *"#ERROR!");
    assert_eq!(model._get_text("B2"), *"#ERROR!");
}

#[test]
fn a_long_text_repeated_past_the_limit_is_an_error_not_an_overflow() {
    let mut model = new_empty_model();
    model._set("A1", "=REPT(\"x\", 32767)");
    model._set("A2", "=A1&A1&A1");
    // 98,301 characters 30,000 times overflows an i32 to a negative length.
    model._set("B1", "=REPT(A2, 30000)");

    model.evaluate();

    assert_eq!(model._get_text("A2").len(), 98_301);
    assert_eq!(model._get_text("B1"), *"#VALUE!");
}
