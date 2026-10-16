#![allow(clippy::unwrap_used)]

use crate::{mock_time, test::util::new_empty_model};

// 14:44 20 Mar 2023 Berlin
const TIMESTAMP_2023: i64 = 1679319865208;

#[test]
fn arguments() {
    mock_time::set_mock_time(TIMESTAMP_2023);
    let mut model = new_empty_model();

    model._set("A1", "=NOW(1, 1)");
    model._set("A2", "=NOW(\"Europe/Berlin\")");
    model._set("A3", "=NOW(\"faketimezone\")");
    model.evaluate();

    assert_eq!(
        model._get_text("A1"),
        "#ERROR!",
        "Wrong number of arguments"
    );
    assert_eq!(model._get_text("A2"), *"3/20/2023, 2:44 PM");
    assert_eq!(
        model._get_text("A3"),
        "#VALUE!",
        "Invalid timezone: faketimezone"
    );
}

#[test]
fn returns_date_time() {
    mock_time::set_mock_time(TIMESTAMP_2023);
    let mut model = new_empty_model();
    model._set("A1", "=NOW()");
    model.evaluate();
    let text = model._get_text("A1");
    assert_eq!(text, *"3/20/2023, 1:44 PM");
}

// Excel reads the clock once per recalculation, so two calls always agree.
#[test]
fn one_clock_reading_per_evaluation() {
    mock_time::set_mock_time(TIMESTAMP_2023);
    let mut model = new_empty_model();
    model._set("A1", "=NOW()-NOW()");
    model._set("A2", "=TODAY()-INT(NOW())");
    model._set(
        "A3",
        "=TODAY(\"Europe/Berlin\")-INT(NOW(\"Europe/Berlin\"))",
    );
    model._set("A4", "=NOW()");
    model.evaluate();
    assert_eq!(model._get_text("A1"), *"0");
    assert_eq!(model._get_text("A2"), *"0");
    assert_eq!(model._get_text("A3"), *"0");
    assert_eq!(model._get_text("A4"), *"3/20/2023, 1:44 PM");

    mock_time::set_mock_time(TIMESTAMP_2023 + 3_600_000);
    model.evaluate();
    assert_eq!(model._get_text("A1"), *"0");
    assert_eq!(model._get_text("A4"), *"3/20/2023, 2:44 PM");
}

#[test]
fn incremental_evaluation_takes_a_new_clock_reading() {
    mock_time::set_mock_time(TIMESTAMP_2023);
    let mut model = new_empty_model();
    model._set("A1", "=NOW()");
    model._set("A2", "=NOW()-A1");
    model._set("B1", "1");
    model.evaluate_indexed();
    assert_eq!(model._get_text("A1"), *"3/20/2023, 1:44 PM");

    mock_time::set_mock_time(TIMESTAMP_2023 + 3_600_000);
    model.evaluate_incremental(&[(0, 1, 2)]);
    assert_eq!(model._get_text("A1"), *"3/20/2023, 2:44 PM");
    assert_eq!(model._get_text("A2"), *"0");
}
