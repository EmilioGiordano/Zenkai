// Dates written day first ("25/10/2026", "8-10-26"), as Excel exports them in most locales
// outside the US. The engine reads slashed dates month first, so day-first fields are
// rewritten to ISO (2026-10-25) before import; anything else is left alone.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateOrder {
    DayFirst,
    MonthFirst,
}

/// The order the dates prove (a part above 12), or `None` when every date
/// reads both ways.
pub fn detect_date_order(rows: &[Vec<String>]) -> Option<DateOrder> {
    let mut found = None;
    for field in rows.iter().flatten() {
        let Some((first, second, _)) = parts(field) else {
            continue;
        };
        let order = match (first > 12, second > 12) {
            (true, false) => DateOrder::DayFirst,
            (false, true) => DateOrder::MonthFirst,
            _ => continue,
        };
        match found {
            None => found = Some(order),
            Some(seen) if seen != order => return None,
            Some(_) => {}
        }
    }
    found
}

/// Rewrites every valid day-first date to ISO; returns how many fields changed.
pub fn normalize_day_first(rows: &mut [Vec<String>]) -> usize {
    let mut changed = 0;
    for field in rows.iter_mut().flatten() {
        if let Some(rewritten) = day_first_field(field) {
            *field = rewritten;
            changed += 1;
        }
    }
    changed
}

// What `normalize_day_first` makes of one field, so a save can tell what the import turned a
// field of the file into.
pub fn day_first_field(field: &str) -> Option<String> {
    let (day, month, year) = parts(field)?;
    valid(day, month, year).then(|| format!("{year:04}-{month:02}-{day:02}"))
}

// "d/m/y" with one or two digits for d and m, two or four for y, and the same separator
// ('/', '-' or '.') twice. Two-digit years follow Excel: 00-29 are 2000s, 30-99 are 1900s.
fn parts(field: &str) -> Option<(u32, u32, u32)> {
    let separator = field.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let mut pieces = field.split(separator);
    let (a, b, c) = (pieces.next()?, pieces.next()?, pieces.next()?);
    if pieces.next().is_some() {
        return None;
    }
    let number = |text: &str, lengths: &[usize]| {
        (lengths.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    let first = number(a, &[1, 2])?;
    let second = number(b, &[1, 2])?;
    // "3.4.10" is more likely a version than a date: dots need a four-digit year.
    let year = number(c, if separator == '.' { &[4] } else { &[2, 4] })?;
    let year = match (c.len(), year) {
        (2, y) if y < 30 => 2000 + y,
        (2, y) => 1900 + y,
        (_, y) => y,
    };
    (first >= 1 && second >= 1).then_some((first, second, year))
}

// The serial number a text stands for when read as a date in a file with this order: year
// first (2026-10-08) or a number, a number and a year (10/8/2026) read in `order`. `None` for
// anything else, including a date with a time or with words.
pub fn date_serial(text: &str, order: DateOrder) -> Option<f64> {
    let separator = text.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let pieces: Vec<&str> = text.split(separator).collect();
    let year_first = pieces.first().is_some_and(|first| first.len() == 4);
    let (year, month, day) = if year_first {
        let [a, b, c] = pieces[..] else {
            return None;
        };
        let number = |piece: &str| {
            (piece.len() <= 2 && piece.bytes().all(|b| b.is_ascii_digit()))
                .then(|| piece.parse::<u32>().ok())
                .flatten()
        };
        let year = (a.bytes().all(|b| b.is_ascii_digit()))
            .then(|| a.parse::<u32>().ok())
            .flatten()?;
        (year, number(b)?, number(c)?)
    } else {
        let (first, second, year) = parts(text)?;
        match order {
            DateOrder::DayFirst => (year, second, first),
            DateOrder::MonthFirst => (year, first, second),
        }
    };
    if !valid(day, month, year) {
        return None;
    }
    let (y, m, d) = (i64::from(year), i64::from(month), i64::from(day));
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let serial = era * 146_097 + day_of_era - 719_468 + 25_569;
    (serial >= 61).then_some(serial as f64)
}

// The date an Excel serial number stands for, as yyyy-mm-dd. Serials below 61 are left out:
// Excel counts a 29 February 1900 that never existed.
pub fn iso_date(serial: f64) -> Option<String> {
    if serial.fract() != 0.0 || !(61.0..=2_958_465.0).contains(&serial) {
        return None;
    }
    let days_since_1970 = serial as i64 - 25_569;
    let z = days_since_1970 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

fn valid(day: u32, month: u32, year: u32) -> bool {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1900..=9999).contains(&year) && day <= days
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(fields: &[&str]) -> Vec<Vec<String>> {
        vec![fields.iter().map(ToString::to_string).collect()]
    }

    #[test]
    fn detects_order_only_when_a_part_proves_it() {
        assert_eq!(
            detect_date_order(&rows(&["08/10/2026", "25/10/2026"])),
            Some(DateOrder::DayFirst)
        );
        assert_eq!(
            detect_date_order(&rows(&["10/25/2026"])),
            Some(DateOrder::MonthFirst)
        );
        assert_eq!(detect_date_order(&rows(&["08/10/2026", "x"])), None);
        assert_eq!(
            detect_date_order(&rows(&["25/10/2026", "10/25/2026"])),
            None
        );
    }

    #[test]
    fn rewrites_valid_day_first_dates_to_iso() {
        let mut data = rows(&[
            "25/10/2026",
            "8-1-26",
            "1.2.1995",
            "29/02/2024",
            "31/04/2026",
            "1/2/3/4",
            "12,5",
            "Total",
        ]);
        assert_eq!(normalize_day_first(&mut data), 4);
        assert_eq!(
            data[0],
            [
                "2026-10-25",
                "2026-01-08",
                "1995-02-01",
                "2024-02-29",
                "31/04/2026",
                "1/2/3/4",
                "12,5",
                "Total"
            ]
        );
    }

    #[test]
    fn a_serial_number_is_the_date_it_counts_days_to() {
        assert_eq!(iso_date(46_303.0).as_deref(), Some("2026-10-08"));
        assert_eq!(iso_date(61.0).as_deref(), Some("1900-03-01"));
        assert_eq!(iso_date(25_569.0).as_deref(), Some("1970-01-01"));
        assert_eq!(iso_date(36_585.0).as_deref(), Some("2000-02-29"));
        assert_eq!(iso_date(2_958_465.0).as_deref(), Some("9999-12-31"));
        for serial in [60.0, 0.0, -1.0, 46_303.5, 2_958_466.0, f64::NAN] {
            assert_eq!(iso_date(serial), None, "{serial}");
        }
    }

    #[test]
    fn a_text_is_the_serial_of_the_date_it_reads_as_in_a_files_order() {
        let read = |text: &str, order| date_serial(text, order);
        assert_eq!(read("10/8/2026", DateOrder::MonthFirst), Some(46_303.0));
        assert_eq!(read("10/8/2026", DateOrder::DayFirst), Some(46_244.0));
        assert_eq!(read("2026-10-08", DateOrder::DayFirst), Some(46_303.0));
        assert_eq!(read("2026/10/8", DateOrder::MonthFirst), Some(46_303.0));
        assert_eq!(read("8.10.2026", DateOrder::DayFirst), Some(46_303.0));
        for text in ["8 October 2026", "2026-10-08 12:00", "13/13/2026", "x", ""] {
            assert_eq!(read(text, DateOrder::MonthFirst), None, "{text}");
        }
        for serial in [61.0, 25_569.0, 46_303.0, 2_958_465.0] {
            let iso = iso_date(serial).unwrap();
            assert_eq!(read(&iso, DateOrder::DayFirst), Some(serial), "{iso}");
        }
    }

    #[test]
    fn mixed_separators_and_lengths_are_not_dates() {
        for field in [
            "1/2-2026",
            "123/1/2026",
            "1/1/202",
            "0/1/2026",
            "a/b/c",
            "1/1",
        ] {
            assert_eq!(parts(field), None, "{field}");
        }
    }
}

#[cfg(test)]
mod no_panic {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

        #[test]
        fn date_rewrite_accepts_any_field(field in prop_oneof![any::<String>(), "[0-9/.-]{0,12}"]) {
            let mut rows = vec![vec![field]];
            let _ = super::detect_date_order(&rows);
            let _ = super::normalize_day_first(&mut rows);
        }
    }
}
