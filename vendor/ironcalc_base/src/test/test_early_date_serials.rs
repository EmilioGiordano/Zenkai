#![allow(clippy::unwrap_used)]

use crate::test::util::new_empty_model;

// Excel's 1900 date system: serial 1 is 1/1/1900 and serial 60 is the fictitious 2/29/1900.
fn excel_year_month_day(serial: u32) -> (u32, u32, u32) {
    match serial {
        0..=31 => (1900, 1, serial),
        32..=60 => (1900, 2, serial - 31),
        _ => (1900, 3, serial - 60),
    }
}

#[test]
fn day_month_year_of_early_serials() {
    let mut model = new_empty_model();
    for serial in 0..=70 {
        let row = serial + 1;
        model._set(&format!("A{row}"), &format!("=YEAR({serial})"));
        model._set(&format!("B{row}"), &format!("=MONTH({serial})"));
        model._set(&format!("C{row}"), &format!("=DAY({serial})"));
    }
    model.evaluate();
    for serial in 0..=70 {
        let row = serial + 1;
        let (year, month, day) = excel_year_month_day(serial);
        let actual = (
            model._get_text(&format!("A{row}")),
            model._get_text(&format!("B{row}")),
            model._get_text(&format!("C{row}")),
        );
        let expected = (year.to_string(), month.to_string(), day.to_string());
        assert_eq!(actual, expected, "serial {serial}");
    }
}

#[test]
fn day_month_year_of_modern_serials() {
    let mut model = new_empty_model();
    model._set("A1", "=YEAR(45351)&\"-\"&MONTH(45351)&\"-\"&DAY(45351)");
    model._set("A2", "=YEAR(36526)&\"-\"&MONTH(36526)&\"-\"&DAY(36526)");
    model._set(
        "A3",
        "=YEAR(2958465)&\"-\"&MONTH(2958465)&\"-\"&DAY(2958465)",
    );
    model._set("A4", "=DAY(-1)");
    model._set("A5", "=DAY(2958466)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "2024-2-29");
    assert_eq!(model._get_text("A2"), "2000-1-1");
    assert_eq!(model._get_text("A3"), "9999-12-31");
    assert_eq!(model._get_text("A4"), "#NUM!");
    assert_eq!(model._get_text("A5"), "#NUM!");
}

#[test]
fn weekday_of_early_serials() {
    let mut model = new_empty_model();
    model._set("A1", "=WEEKDAY(0)");
    model._set("A2", "=WEEKDAY(1)");
    model._set("A3", "=WEEKDAY(7)");
    model._set("A4", "=WEEKDAY(59)");
    model._set("A5", "=WEEKDAY(60)");
    model._set("A6", "=WEEKDAY(61)");
    model._set("A7", "=WEEKDAY(45292)");
    model._set("A8", "=WEEKDAY(0, 2)");
    model._set("A9", "=WEEKDAY(-1)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "7");
    assert_eq!(model._get_text("A2"), "1");
    assert_eq!(model._get_text("A3"), "7");
    assert_eq!(model._get_text("A4"), "3");
    assert_eq!(model._get_text("A5"), "4");
    assert_eq!(model._get_text("A6"), "5");
    assert_eq!(model._get_text("A7"), "2");
    assert_eq!(model._get_text("A8"), "6");
    assert_eq!(model._get_text("A9"), "#NUM!");
}

#[test]
fn text_of_early_serials() {
    let mut model = new_empty_model();
    model._set("A1", "=TEXT(7, \"m/d/yyyy\")");
    model._set("A2", "=TEXT(0, \"m/d/yyyy\")");
    model._set("A3", "=TEXT(60, \"dddd, mmmm d, yyyy\")");
    model._set("A4", "=TEXT(61, \"m/d/yyyy\")");
    model._set("A5", "=TEXT(45292, \"m/d/yyyy\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "1/7/1900");
    assert_eq!(model._get_text("A2"), "1/0/1900");
    assert_eq!(model._get_text("A3"), "Wednesday, February 29, 1900");
    assert_eq!(model._get_text("A4"), "3/1/1900");
    assert_eq!(model._get_text("A5"), "1/1/2024");
}
