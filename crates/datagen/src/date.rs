use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

const FIRST_YEAR: i64 = 1900;
const LAST_YEAR: i64 = 9999;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Date {
    days_since_epoch: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("\"{0}\" is not a date written as yyyy-mm-dd between 1900-01-01 and 9999-12-31")]
pub struct InvalidDate(pub String);

impl Date {
    pub fn from_ymd(year: i64, month: u32, day: u32) -> Option<Date> {
        let in_range = (FIRST_YEAR..=LAST_YEAR).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month);
        in_range.then(|| Date {
            days_since_epoch: days_from_civil(year, month, day),
        })
    }

    pub fn parse(text: &str) -> Result<Date, InvalidDate> {
        let invalid = || InvalidDate(text.to_string());
        let mut parts = text.split('-');
        let (Some(year), Some(month), Some(day), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let all_digits = |part: &str, len: usize| {
            part.len() == len && part.bytes().all(|byte| byte.is_ascii_digit())
        };
        if !(all_digits(year, 4) && all_digits(month, 2) && all_digits(day, 2)) {
            return Err(invalid());
        }
        let number = |part: &str| part.parse::<u32>().map_err(|_| invalid());
        Date::from_ymd(i64::from(number(year)?), number(month)?, number(day)?).ok_or_else(invalid)
    }

    // No range check: only for literal dates known to be valid.
    pub(crate) const fn known(year: i64, month: u32, day: u32) -> Date {
        Date {
            days_since_epoch: days_from_civil(year, month, day),
        }
    }

    pub(crate) fn days_until(self, later: Date) -> i64 {
        later.days_since_epoch - self.days_since_epoch
    }

    pub(crate) fn plus_days(self, days: i64) -> Date {
        Date {
            days_since_epoch: self.days_since_epoch + days,
        }
    }
}

impl fmt::Display for Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days(self.days_since_epoch);
        write!(formatter, "{year:04}-{month:02}-{day:02}")
    }
}

impl TryFrom<String> for Date {
    type Error = InvalidDate;

    fn try_from(text: String) -> Result<Date, InvalidDate> {
        Date::parse(&text)
    }
}

impl From<Date> for String {
    fn from(date: Date) -> String {
        date.to_string()
    }
}

impl JsonSchema for Date {
    fn schema_name() -> Cow<'static, str> {
        "Date".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "date",
            "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$",
            "description": "Calendar date written as yyyy-mm-dd, from 1900-01-01 to 9999-12-31."
        })
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's days_from_civil and civil_from_days, proleptic Gregorian calendar.
const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = ((month + 9) % 12) as i64;
    let day_of_year = (153 * shifted_month + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_iso_dates() {
        for text in ["1900-01-01", "2020-02-29", "2024-12-31", "9999-12-31"] {
            assert_eq!(Date::parse(text).unwrap().to_string(), text);
        }
    }

    #[test]
    fn rejects_malformed_or_impossible_dates() {
        for text in [
            "2021-02-29",
            "2020-13-01",
            "2020-1-01",
            "20-01-01",
            "1899-12-31",
            "2020/01/01",
            "2020-01-01-01",
            "",
        ] {
            assert_eq!(Date::parse(text), Err(InvalidDate(text.to_string())));
        }
    }

    #[test]
    fn day_arithmetic_crosses_months_and_years() {
        let start = Date::parse("2023-12-30").unwrap();
        assert_eq!(start.plus_days(3).to_string(), "2024-01-02");
        assert_eq!(start.days_until(Date::parse("2024-03-01").unwrap()), 62);
    }
}
