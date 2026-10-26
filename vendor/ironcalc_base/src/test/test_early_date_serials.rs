#![allow(clippy::unwrap_used)]

use crate::{cell::CellValue, test::util::new_empty_model};

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

#[test]
fn date_of_early_days() {
    let mut model = new_empty_model();
    let cases = [
        ("DATE(1900, 1, 1)", "1"),
        ("DATE(1900, 1, 7)", "7"),
        ("DATE(1900, 1, 31)", "31"),
        ("DATE(1900, 2, 1)", "32"),
        ("DATE(1900, 2, 28)", "59"),
        ("DATE(1900, 2, 29)", "60"),
        ("DATE(1900, 3, 0)", "60"),
        ("DATE(1900, 3, 1)", "61"),
        ("DATE(1900, 1, 0)", "0"),
        ("DATE(1900, 1, -1)", "#NUM!"),
        ("DATE(1900, 1, 60)", "60"),
        ("DATE(1900, 1, 61)", "61"),
        ("DATE(1900, 2, 30)", "61"),
        ("DATE(1900, 4, -30)", "61"),
        ("DATE(1900, 4, -31)", "60"),
        ("DATE(1900, 4, -32)", "59"),
        ("DATE(2024, 1, 1)", "45292"),
        ("DATE(2024, 2, 30)", "45352"),
        ("DATE(9999, 12, 31)", "2958465"),
        ("DATE(9999, 12, 32)", "#NUM!"),
    ];
    for (row, (date, _)) in cases.iter().enumerate() {
        model._set(&format!("A{}", row + 1), &format!("=TEXT({date}, \"0\")"));
    }
    model.evaluate();
    for (row, (date, expected)) in cases.iter().enumerate() {
        assert_eq!(
            model._get_text(&format!("A{}", row + 1)),
            *expected,
            "{date}"
        );
    }
}

#[test]
fn datevalue_of_early_days() {
    let mut model = new_empty_model();
    model._set("A1", "=DATEVALUE(\"1/1/1900\")");
    model._set("A2", "=DATEVALUE(\"2/28/1900\")");
    model._set("A3", "=DATEVALUE(\"2/29/1900\")");
    model._set("A4", "=DATEVALUE(\"3/1/1900\")");
    model._set("A5", "=DATEVALUE(\"1/1/2024\")");
    model._set("A6", "=DATEVALUE(\"2/30/1900\")");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "1");
    assert_eq!(model._get_text("A2"), "59");
    assert_eq!(model._get_text("A3"), "60");
    assert_eq!(model._get_text("A4"), "61");
    assert_eq!(model._get_text("A5"), "45292");
    assert_eq!(model._get_text("A6"), "#VALUE!");
}

#[test]
fn typed_early_dates_store_excel_serials() {
    let mut model = new_empty_model();
    model._set("A1", "1/7/1900");
    model._set("A2", "2/28/1900");
    model._set("A3", "2/29/1900");
    model._set("A4", "3/1/1900");
    model._set("A5", "1/1/2024");
    model.evaluate();
    let expected = [
        ("A1", 7.0, "1/7/1900"),
        ("A2", 59.0, "2/28/1900"),
        ("A3", 60.0, "2/29/1900"),
        ("A4", 61.0, "3/1/1900"),
        ("A5", 45292.0, "1/1/2024"),
    ];
    for (cell, serial, text) in expected {
        assert_eq!(
            model.get_cell_value_by_ref(&format!("Sheet1!{cell}")),
            Ok(CellValue::Number(serial)),
            "{cell}"
        );
        assert_eq!(model._get_text(cell), text, "{cell}");
    }
}

#[test]
fn typed_dates_before_1900_stay_text() {
    let mut model = new_empty_model();
    model._set("A1", "1899-12-31");
    model._set("A2", "12/31/1899");
    model._set("A3", "=DATEVALUE(\"12/31/1899\")");
    model._set("A4", "12/31/9999");
    model.evaluate();
    assert_eq!(
        model.get_cell_value_by_ref("Sheet1!A1"),
        Ok(CellValue::String("1899-12-31".to_string()))
    );
    assert_eq!(
        model.get_cell_value_by_ref("Sheet1!A2"),
        Ok(CellValue::String("12/31/1899".to_string()))
    );
    assert_eq!(model._get_text("A3"), "#VALUE!");
    assert_eq!(
        model.get_cell_value_by_ref("Sheet1!A4"),
        Ok(CellValue::Number(2958465.0))
    );
}

#[test]
fn eomonth_reaches_the_last_and_first_months() {
    let mut model = new_empty_model();
    model._set("A1", "=EOMONTH(2958465, 0)");
    model._set("A2", "=EOMONTH(2958435, 0)");
    model._set("A3", "=EOMONTH(61, -2)");
    model.evaluate();
    assert_eq!(
        model.get_cell_value_by_ref("Sheet1!A1"),
        Ok(CellValue::Number(2958465.0))
    );
    assert_eq!(
        model.get_cell_value_by_ref("Sheet1!A2"),
        Ok(CellValue::Number(2958465.0))
    );
    assert_eq!(
        model.get_cell_value_by_ref("Sheet1!A3"),
        Ok(CellValue::Number(31.0))
    );
}
