use serde::{Deserialize, Serialize};

use crate::csv_format::{Delimiter, ParsedCsv};
use crate::dates::{DateOrder, detect_date_order, normalize_day_first};
use crate::numbers::{detect_decimal_comma, normalize_decimal_comma};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecimalMark {
    Dot,
    Comma,
}

// How the numbers and dates of a file are read on import: guessed from the file, and a user
// may switch either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guess {
    pub decimal_mark: DecimalMark,
    pub date_order: DateOrder,
    pub columns: usize,
}

// Dates go first: a day-first date written with dots must not be read as a grouped number.
pub fn apply(decimal_mark: DecimalMark, date_order: DateOrder, rows: &mut [Vec<String>]) {
    if date_order == DateOrder::DayFirst {
        normalize_day_first(rows);
    }
    if decimal_mark == DecimalMark::Comma {
        normalize_decimal_comma(rows);
    }
}

// Scans every row, so callers run it off the UI thread.
pub fn guess(parsed: &ParsedCsv) -> Guess {
    let decimal_mark = if parsed.delimiter != Delimiter::Comma && detect_decimal_comma(&parsed.rows)
    {
        DecimalMark::Comma
    } else {
        DecimalMark::Dot
    };
    // Dates that read both ways follow the number style: a decimal comma means a locale
    // that writes the day first.
    let date_order = detect_date_order(&parsed.rows).unwrap_or(match decimal_mark {
        DecimalMark::Comma => DateOrder::DayFirst,
        DecimalMark::Dot => DateOrder::MonthFirst,
    });
    Guess {
        decimal_mark,
        date_order,
        columns: parsed.rows.iter().map(Vec::len).max().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv_format::parse_csv;

    #[test]
    fn a_semicolon_file_with_decimal_commas_reads_numbers_and_dates_that_way() {
        let parsed = parse_csv(b"d;n\n08/10/2026;1,5\n", None).unwrap();
        let guessed = guess(&parsed);
        assert_eq!(guessed.decimal_mark, DecimalMark::Comma);
        assert_eq!(guessed.date_order, DateOrder::DayFirst);
        assert_eq!(guessed.columns, 2);
        let mut rows = parsed.rows;
        apply(guessed.decimal_mark, guessed.date_order, &mut rows);
        assert_eq!(rows[1], ["2026-10-08", "1.5"]);
    }

    #[test]
    fn a_comma_file_reads_dots_and_month_first_dates() {
        let parsed = parse_csv(b"d,n\n08/10/2026,1.5\n", None).unwrap();
        let guessed = guess(&parsed);
        assert_eq!(guessed.decimal_mark, DecimalMark::Dot);
        assert_eq!(guessed.date_order, DateOrder::MonthFirst);
    }
}
