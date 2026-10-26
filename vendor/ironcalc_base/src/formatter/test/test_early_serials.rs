#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

use crate::{formatter::format::format_number, locale::get_locale};

// Excel's 1900 date system: serial 1 is 1/1/1900 and serial 60 is the fictitious 2/29/1900.
fn excel_label(serial: u32) -> String {
    match serial {
        0..=31 => format!("1/{serial}/1900"),
        32..=60 => format!("2/{}/1900", serial - 31),
        _ => format!("3/{}/1900", serial - 60),
    }
}

#[test]
fn serials_up_to_seventy_format_as_excel() {
    let locale = get_locale("en").unwrap();
    for serial in 0..=70 {
        let formatted = format_number(serial as f64, "m/d/yyyy", locale);
        assert_eq!(formatted.text, excel_label(serial), "serial {serial}");
    }
}

#[test]
fn modern_serials_format_as_excel() {
    let locale = get_locale("en").unwrap();
    let cases = [
        (36526.0, "1/1/2000"),
        (43831.0, "1/1/2020"),
        (45292.0, "1/1/2024"),
        (45351.0, "2/29/2024"),
        (2958465.0, "12/31/9999"),
    ];
    for (serial, expected) in cases {
        assert_eq!(format_number(serial, "m/d/yyyy", locale).text, expected);
    }
}

#[test]
fn early_serials_name_days_months_and_years_as_excel() {
    let locale = get_locale("en").unwrap();
    let cases = [
        (0.0, "dddd d mmmm yy", "Saturday 0 January 00"),
        (1.0, "dddd d mmmm yy", "Sunday 1 January 00"),
        (7.0, "ddd dd mmm yyyy", "Sat 07 Jan 1900"),
        (59.0, "dddd d mmmm yyyy", "Tuesday 28 February 1900"),
        (60.0, "dddd d mmmm yyyy", "Wednesday 29 February 1900"),
        (61.0, "dddd d mmmm yyyy", "Thursday 1 March 1900"),
        (45292.0, "dddd d mmmm yy", "Monday 1 January 24"),
        (7.5, "m/d/yyyy h:mm", "1/7/1900 12:00"),
    ];
    for (serial, format, expected) in cases {
        assert_eq!(format_number(serial, format, locale).text, expected);
    }
}

#[test]
fn negative_serials_are_not_dates() {
    let locale = get_locale("en").unwrap();
    assert_eq!(format_number(-1.0, "m/d/yyyy", locale).text, "#VALUE!");
    assert_eq!(format_number(-0.5, "m/d/yyyy", locale).text, "#VALUE!");
}
