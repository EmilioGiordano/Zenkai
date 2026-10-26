use chrono::Datelike;
use chrono::Duration;
use chrono::Months;
use chrono::NaiveDate;
use chrono::Weekday;

use crate::constants::EXCEL_DATE_BASE;
use crate::constants::MAXIMUM_DATE_SERIAL_NUMBER;
use crate::constants::MINIMUM_DATE_SERIAL_NUMBER;

pub const DATE_OUT_OF_RANGE_MESSAGE: &str = "Out of range parameters for date";

// Excel counts a fictitious 29 February 1900 as serial 60, so every earlier date sits one
// serial lower than the days elapsed since 1899-12-30.
fn convert_to_serial_number(date: NaiveDate) -> i32 {
    let days = date.num_days_from_ce() - EXCEL_DATE_BASE;
    if days < 61 {
        days - 1
    } else {
        days
    }
}

fn is_date_within_range(date: NaiveDate) -> bool {
    convert_to_serial_number(date) >= MINIMUM_DATE_SERIAL_NUMBER
        && convert_to_serial_number(date) <= MAXIMUM_DATE_SERIAL_NUMBER
}

pub fn from_excel_date(days: i64) -> Result<NaiveDate, String> {
    if days < MINIMUM_DATE_SERIAL_NUMBER as i64 {
        return Err(format!(
            "Excel date must be greater than {MINIMUM_DATE_SERIAL_NUMBER}"
        ));
    };
    if days > MAXIMUM_DATE_SERIAL_NUMBER as i64 {
        return Err(format!(
            "Excel date must be less than {MAXIMUM_DATE_SERIAL_NUMBER}"
        ));
    };
    #[allow(clippy::expect_used)]
    let dt = NaiveDate::from_ymd_opt(1900, 1, 1).expect("problem with chrono::NaiveDate");
    Ok(dt + Duration::days(days - 2))
}

// Maps a date serial number (date part only) to the Excel (year, month, day) it
// represents, replicating Excel's fictional 29 February 1900 (the "1900
// leap-year bug"). Returns `None` when the serial is outside
// `0..=MAXIMUM_DATE_SERIAL_NUMBER`, which Excel reports as `#NUM!`.
pub(crate) fn excel_serial_to_ymd(serial: i64) -> Option<(i32, u32, u32)> {
    if !(0..=MAXIMUM_DATE_SERIAL_NUMBER as i64).contains(&serial) {
        return None;
    }
    // Excel renders serial 0 as "0 January 1900"; DAYS360 reads it as day 0 of
    // January 1900 (not 31 December 1899), which is what makes it antisymmetric.
    if serial == 0 {
        return Some((1900, 1, 0));
    }
    // Excel's non-existent 29 February 1900 (serial 60).
    if serial == 60 {
        return Some((1900, 2, 29));
    }
    let date = if serial < 60 {
        // Below the fictional leap day Excel counts serial 1 as 1900-01-01, so
        // serial N is N days after 1899-12-31 (serial 0).
        let base = NaiveDate::from_ymd_opt(1899, 12, 31)?;
        base + Duration::days(serial)
    } else {
        // From 1900-03-01 (serial 61) onwards IronCalc's serials match Excel.
        from_excel_date(serial).ok()?
    };
    Some((date.year(), date.month(), date.day()))
}

// Excel names the weekday by the serial alone, so it calls 1900-01-01 (serial 1) a Sunday:
// the fictitious 29 February 1900 shifts every earlier weekday by one.
pub(crate) fn excel_serial_weekday(serial: i64) -> Option<Weekday> {
    if !(0..=MAXIMUM_DATE_SERIAL_NUMBER as i64).contains(&serial) {
        return None;
    }
    let base = NaiveDate::from_ymd_opt(1899, 12, 30)?;
    Some((base + Duration::days(serial)).weekday())
}

pub fn date_to_serial_number(day: u32, month: u32, year: i32) -> Result<i32, String> {
    // Excel's 1900 leap-year bug: it accepts 29 February 1900 as serial 60.
    if (year, month, day) == (1900, 2, 29) {
        return Ok(60);
    }
    match NaiveDate::from_ymd_opt(year, month, day) {
        Some(native_date) => Ok(convert_to_serial_number(native_date)),
        None => Err(DATE_OUT_OF_RANGE_MESSAGE.to_string()),
    }
}

pub fn permissive_date_to_serial_number(day: i32, month: i32, year: i32) -> Result<i32, String> {
    // Excel parses `DATE` very permissively. It allows not just for valid date values, but it
    // allows for invalid dates as well. If you for example enter `DATE(1900, 1, 32)` it will
    // return the date `1900-02-01`. Despite giving a day that is out of range it will just
    // wrap the month and year around.
    //
    // This function applies that same logic to dates. And does it in the most compatible way as
    // possible.

    let Some(mut date) = NaiveDate::from_ymd_opt(year, 1, 1) else {
        return Err(DATE_OUT_OF_RANGE_MESSAGE.to_string());
    };

    // One thing to note for example is that even if you started with a year out of range
    // but tried to increment the months so that it wraps around into within range, excel
    // would still return an error.
    //
    // I.E. DATE(0,13,-1) will return an error, despite it being equivalent to DATE(1,1,0) which
    // is within range.
    //
    // As a result, we have to run range checks as we parse the date from the biggest unit to the
    // smallest unit.
    if !is_date_within_range(date) {
        return Err(DATE_OUT_OF_RANGE_MESSAGE.to_string());
    }

    date = {
        let month_diff = month - 1;
        let abs_month = month_diff.unsigned_abs();
        if month_diff <= 0 {
            date = date - Months::new(abs_month);
        } else {
            date = date + Months::new(abs_month);
        }
        if !is_date_within_range(date) {
            return Err(DATE_OUT_OF_RANGE_MESSAGE.to_string());
        }
        date
    };

    // Excel adds the days to the serial of the first of the month, so DATE(1900, 3, 0) is the
    // fictitious 29 February 1900 (60) and DATE(1900, 1, 0) is serial 0.
    let serial = convert_to_serial_number(date) as i64 + day as i64 - 1;
    if !(0..=MAXIMUM_DATE_SERIAL_NUMBER as i64).contains(&serial) {
        return Err(DATE_OUT_OF_RANGE_MESSAGE.to_string());
    }
    Ok(serial as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_date_to_serial_number() {
        assert_eq!(
            permissive_date_to_serial_number(42, 42, 2002),
            date_to_serial_number(12, 7, 2005)
        );
        assert_eq!(
            permissive_date_to_serial_number(1, 42, 2002),
            date_to_serial_number(1, 6, 2005)
        );
        assert_eq!(
            permissive_date_to_serial_number(1, 15, 2000),
            date_to_serial_number(1, 3, 2001)
        );
        assert_eq!(
            permissive_date_to_serial_number(1, 49, 2000),
            date_to_serial_number(1, 1, 2004)
        );
        assert_eq!(
            permissive_date_to_serial_number(1, 49, 2000),
            date_to_serial_number(1, 1, 2004)
        );
        assert_eq!(
            permissive_date_to_serial_number(31, 49, 2000),
            date_to_serial_number(31, 1, 2004)
        );
        assert_eq!(
            permissive_date_to_serial_number(256, 49, 2000),
            date_to_serial_number(12, 9, 2004)
        );
        assert_eq!(
            permissive_date_to_serial_number(256, 1, 2004),
            date_to_serial_number(12, 9, 2004)
        );
    }

    #[test]
    fn test_max_and_min_dates() {
        assert_eq!(
            permissive_date_to_serial_number(31, 12, 9999),
            Ok(MAXIMUM_DATE_SERIAL_NUMBER),
        );
        assert_eq!(permissive_date_to_serial_number(0, 1, 1900), Ok(0));
        assert!(permissive_date_to_serial_number(-1, 1, 1900).is_err());
        assert!(permissive_date_to_serial_number(1, 13, 9999).is_err());
    }
}
