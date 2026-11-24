#![allow(clippy::unwrap_used)]

use crate::{
    formatter::format::format_number,
    locale::{get_locale, Locale},
};

fn get_default_locale() -> &'static Locale {
    get_locale("en").unwrap()
}

#[test]
fn simple_test() {
    let locale = get_default_locale();
    let format = "h:mm AM/PM";
    let value = 16.001_423_611_111_11; // =1/86400 => 12:02 AM
    let formatted = format_number(value, format, locale);
    assert_eq!(formatted.text, "12:02 AM");
}

#[test]
fn padded_vs_unpadded() {
    let locale = get_default_locale();
    let padded_format = "hh:mm:ss AM/PM";
    let unpadded_format = "h:m:s AM/PM";
    let value = 0.25351851851851853; // => 6:05:04 AM (21904/(24*60*60)) where 21904 = 6 * 3600 + 5*60 + 4
    let formatted = format_number(value, padded_format, locale);
    assert_eq!(formatted.text, "06:05:04 AM");

    let formatted = format_number(value, unpadded_format, locale);
    assert_eq!(formatted.text, "6:5:4 AM");
}

#[test]
fn elapsed_hour() {
    let locale = get_default_locale();
    let format = "[hh]:mm:ss";
    // 1.5 days = 36 hours
    let value = 1.5;
    let formatted = format_number(value, format, locale);
    assert_eq!(formatted.text, "36:00:00");
}

#[test]
fn elapsed_minute() {
    let locale = get_default_locale();
    let format = "[mm]:ss";
    // 2 days = 2880 minutes
    let value = 2.0;
    let formatted = format_number(value, format, locale);
    assert_eq!(formatted.text, "2880:00");
}

#[test]
fn elapsed_second() {
    let locale = get_default_locale();
    let format = "[ss]";
    // 0.5 days = 43200 seconds
    let value = 0.5;
    let formatted = format_number(value, format, locale);
    assert_eq!(formatted.text, "43200");
}

#[test]
fn elapsed_hour_padded() {
    let locale = get_default_locale();
    let format = "[hh]:mm:ss";
    // 0.1 days = 2.4 hours
    let value = 0.1;
    let formatted = format_number(value, format, locale);
    assert_eq!(formatted.text, "02:24:00");
}

#[test]
fn fractional_seconds() {
    let locale = get_default_locale();
    let fmt = r"yyyy\-mm\-dd\ hh:mm:ss.000";
    assert_eq!(
        format_number(45331.417007951401, fmt, locale).text,
        "2024-02-09 10:00:29.487"
    );
    assert_eq!(
        format_number(45860.0, fmt, locale).text,
        "2025-07-22 00:00:00.000"
    );

    assert_eq!(
        format_number(45331.417007951401, "hh:mm:ss.0", locale).text,
        "10:00:29.5"
    );
    assert_eq!(
        format_number(45331.417007951401, "hh:mm:ss.00", locale).text,
        "10:00:29.49"
    );
    assert_eq!(
        format_number(5.05 / 86_400.0, "ss.00", locale).text,
        "05.05"
    );
    assert_eq!(format_number(5.0 / 86_400.0, "s.0", locale).text, "5.0");
}

#[test]
fn fractional_seconds_carry() {
    let locale = get_default_locale();
    assert_eq!(
        format_number(5.9996 / 86_400.0, "hh:mm:ss.000", locale).text,
        "00:00:06.000"
    );
    assert_eq!(
        format_number(59.9996 / 86_400.0, "hh:mm:ss.000", locale).text,
        "00:01:00.000"
    );
    assert_eq!(
        format_number(3599.9996 / 86_400.0, "hh:mm:ss.000", locale).text,
        "01:00:00.000"
    );
    assert_eq!(
        format_number(
            45331.0 + 86_399.9996 / 86_400.0,
            "yyyy-mm-dd hh:mm:ss.000",
            locale
        )
        .text,
        "2024-02-10 00:00:00.000"
    );
}

#[test]
fn fractional_seconds_elapsed_and_unchanged() {
    let locale = get_default_locale();
    assert_eq!(
        format_number(1.5 + 12.345 / 86_400.0, "[h]:mm:ss.000", locale).text,
        "36:00:12.345"
    );
    assert_eq!(
        format_number((36.0 * 3600.0 - 0.0004) / 86_400.0, "[h]:mm:ss.000", locale).text,
        "36:00:00.000"
    );
    assert_eq!(
        format_number(1.234 / 86_400.0, "[s].000", locale).text,
        "1.234"
    );
    assert_eq!(
        format_number(45331.417007951401, "yyyy-mm-dd hh:mm:ss", locale).text,
        "2024-02-09 10:00:29"
    );
}

#[test]
fn fractional_seconds_locale_and_errors() {
    let fr = get_locale("fr").unwrap();
    assert_eq!(
        format_number(45331.417007951401, "hh:mm:ss.000", fr).text,
        "10:00:29,487"
    );

    let en = get_default_locale();
    assert!(format_number(1.0, "hh:mm.00", en).error.is_some());
    assert!(format_number(1.0, "ss.0000", en).error.is_some());
    assert!(format_number(1.0, "ss.0#", en).error.is_some());
    assert!(format_number(1.0, "yyyy-mm-dd 0.00", en).error.is_some());
}
