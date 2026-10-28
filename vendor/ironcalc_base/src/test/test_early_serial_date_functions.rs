use crate::formatter::dates::{convert_to_serial_number, from_excel_date};
use crate::test::util::new_empty_model;

// Expected values come from Excel's calendar: serial 1 is 1/1/1900, 31 is 1/31/1900, 60 the
// fictitious 2/29/1900, 61 is 3/1/1900 and 91 is 3/31/1900. 45292 is 1/1/2024 and 45351 is
// 2/29/2024. Weekdays follow WEEKDAY: serial 1 is a Sunday, serial 0 a Saturday.
fn assert_formulas(rows: &[(&str, &str)]) {
    let mut model = new_empty_model();
    model._set("Z1", "60");
    model._set("Z2", "2");
    for (index, (formula, _)) in rows.iter().enumerate() {
        model._set(&format!("A{}", index + 1), formula);
    }
    model.evaluate();
    for (index, (formula, expected)) in rows.iter().enumerate() {
        assert_eq!(
            model._get_text(&format!("A{}", index + 1)),
            *expected,
            "{formula}"
        );
    }
}

#[test]
fn eomonth_of_early_serials() {
    assert_formulas(&[
        ("=EOMONTH(0,0)", "31"),
        ("=EOMONTH(1,0)", "31"),
        ("=EOMONTH(31,0)", "31"),
        ("=EOMONTH(32,0)", "60"),
        ("=EOMONTH(59,0)", "60"),
        ("=EOMONTH(60,0)", "60"),
        ("=EOMONTH(61,0)", "91"),
        ("=EOMONTH(1,1)", "60"),
        ("=EOMONTH(61,-1)", "60"),
        ("=EOMONTH(45292,1)", "45351"),
        ("=EOMONTH(45351,0)", "45351"),
        ("=EOMONTH(1,-2)", "#NUM!"),
    ]);
}

#[test]
fn edate_of_early_serials() {
    assert_formulas(&[
        ("=EDATE(1,1)", "32"),
        ("=EDATE(31,1)", "60"),
        ("=EDATE(59,1)", "88"),
        ("=EDATE(60,1)", "89"),
        ("=EDATE(61,-1)", "32"),
        ("=EDATE(91,-1)", "60"),
        ("=EDATE(1,0)", "1"),
        ("=EDATE(0,0)", "0"),
        ("=EDATE(45351,12)", "45716"),
        ("=EDATE(1,-1)", "#NUM!"),
    ]);
}

#[test]
fn weeknum_and_isoweeknum_of_early_serials() {
    assert_formulas(&[
        ("=WEEKNUM(0)", "0"),
        ("=WEEKNUM(1)", "1"),
        ("=WEEKNUM(7)", "1"),
        ("=WEEKNUM(8)", "2"),
        ("=WEEKNUM(59)", "9"),
        ("=WEEKNUM(60)", "9"),
        ("=WEEKNUM(61)", "9"),
        ("=WEEKNUM(2,2)", "2"),
        ("=WEEKNUM(45292)", "1"),
        ("=WEEKNUM(45351)", "9"),
        ("=WEEKNUM(45351,21)", "9"),
        ("=ISOWEEKNUM(1)", "52"),
        ("=ISOWEEKNUM(2)", "1"),
        ("=ISOWEEKNUM(59)", "9"),
        ("=ISOWEEKNUM(60)", "9"),
        ("=ISOWEEKNUM(61)", "9"),
        ("=ISOWEEKNUM(45292)", "1"),
        ("=ISOWEEKNUM(45351)", "9"),
    ]);
}

#[test]
fn networkdays_and_workday_of_early_serials() {
    assert_formulas(&[
        ("=NETWORKDAYS(1,7)", "5"),
        ("=NETWORKDAYS(0,1)", "0"),
        ("=NETWORKDAYS(59,61)", "3"),
        ("=NETWORKDAYS(59,61,Z1)", "2"),
        ("=NETWORKDAYS(1,7,Z2)", "4"),
        ("=NETWORKDAYS.INTL(1,7,11)", "6"),
        ("=NETWORKDAYS.INTL(59,61,1,Z1)", "2"),
        ("=WORKDAY(1,1)", "2"),
        ("=WORKDAY(1,1,Z2)", "3"),
        ("=WORKDAY(0,1)", "2"),
        ("=WORKDAY(59,1)", "60"),
        ("=WORKDAY(59,1,Z1)", "61"),
        ("=WORKDAY(61,-1,Z1)", "59"),
        ("=WORKDAY.INTL(1,1,11)", "2"),
        ("=WORKDAY.INTL(59,1,1,Z1)", "61"),
        ("=WORKDAY(1,-1)", "#NUM!"),
    ]);
}

#[test]
fn yearfrac_datedif_and_days_of_early_serials() {
    assert_formulas(&[
        ("=ROUND(YEARFRAC(1,61,0),6)", "0.166667"),
        ("=ROUND(YEARFRAC(1,60,2),6)", "0.163889"),
        ("=ROUND(YEARFRAC(1,60,3),6)", "0.161644"),
        ("=ROUND(YEARFRAC(0,61,2),6)", "0.169444"),
        ("=ROUND(YEARFRAC(45292,45351,1),6)", "0.161202"),
        ("=DATEDIF(1,61,\"D\")", "60"),
        ("=DATEDIF(1,61,\"M\")", "2"),
        ("=DATEDIF(1,60,\"M\")", "1"),
        ("=DATEDIF(31,60,\"M\")", "0"),
        ("=DATEDIF(1,61,\"Y\")", "0"),
        ("=DATEDIF(1,61,\"YD\")", "60"),
        ("=DATEDIF(60,61,\"MD\")", "1"),
        ("=DATEDIF(0,31,\"D\")", "31"),
        ("=DAYS(61,1)", "60"),
        ("=DAYS(60,0)", "60"),
    ]);
}

#[test]
fn financial_functions_read_early_serials_with_excel_labels() {
    // 30/360 from 1/1/1900 to 2/28/1900 is 57 days.
    assert_formulas(&[("=ROUND(ACCRINTM(1,59,0.1,1000,0),6)", "15.833333")]);
}

#[test]
fn from_excel_date_matches_convert_to_serial_number() {
    for serial in (1..=59).chain(61..=200).chain([45292, 2_958_465]) {
        let date = from_excel_date(serial).unwrap();
        assert_eq!(convert_to_serial_number(date) as i64, serial);
    }
    assert!(from_excel_date(0).is_err());
    assert!(from_excel_date(60).is_err());
}

#[test]
fn date_adds_1900_to_years_below_1900() {
    assert_formulas(&[
        ("=DATE(99,1,1)=DATE(1999,1,1)", "TRUE"),
        ("=DATE(0,1,1)=1", "TRUE"),
        ("=DATE(1899,12,31)=DATE(3799,12,31)", "TRUE"),
        ("=YEAR(DATE(1899,12,31))", "3799"),
        ("=DATE(1900,1,1)=1", "TRUE"),
        ("=DATE(1900,3,1)=61", "TRUE"),
        ("=DATE(10000,1,1)", "#NUM!"),
        ("=DATE(-1,1,1)", "#NUM!"),
    ]);
}

#[test]
fn month_arithmetic_rejects_huge_month_counts() {
    assert_formulas(&[
        ("=EDATE(1,1E19)", "#NUM!"),
        ("=EDATE(1,-1E19)", "#NUM!"),
        ("=EOMONTH(1,1E19)", "#NUM!"),
        ("=EOMONTH(1,-1E19)", "#NUM!"),
        ("=EDATE(45292,9223372036854775807)", "#NUM!"),
    ]);
}

#[test]
fn eomonth_past_the_last_date_is_an_error() {
    assert_formulas(&[
        ("=EOMONTH(2958465,0)", "2958465"),
        ("=EOMONTH(2958465,1)", "#NUM!"),
        ("=EOMONTH(2958000,16)", "#NUM!"),
    ]);
}

#[test]
fn days360_uses_the_excel_february_of_1900() {
    // 2/28/1900 is not a month end in Excel's calendar, 2/29/1900 is.
    assert_formulas(&[("=DAYS360(59,91)", "33"), ("=DAYS360(60,91)", "30")]);
}

// Derived from the serial weekday rules, to confirm in Excel.
#[test]
fn week_numbers_of_serial_zero_and_month_end_before_1900() {
    assert_formulas(&[
        ("=ISOWEEKNUM(0)", "52"),
        ("=WEEKNUM(0,21)", "52"),
        ("=EOMONTH(1,-1)", "0"),
    ]);
}
