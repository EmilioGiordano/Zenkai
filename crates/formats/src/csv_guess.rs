use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::csv_format::{Delimiter, ParsedCsv};
use crate::dates::{DateOrder, day_first_field, detect_date_order, normalize_day_first};
use crate::numbers::{
    decimal_comma_field, detect_decimal_comma, normalize_decimal_comma, number_value, same_value,
};

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

// What the import hands the engine for one field of the file, as `apply` does for every field.
pub fn import_input(field: &str, decimal_mark: DecimalMark, date_order: DateOrder) -> Cow<'_, str> {
    let mut text = Cow::Borrowed(field);
    if date_order == DateOrder::DayFirst
        && let Some(rewritten) = day_first_field(&text)
    {
        text = Cow::Owned(rewritten);
    }
    if decimal_mark == DecimalMark::Comma
        && let Some(rewritten) = decimal_comma_field(&text)
    {
        text = Cow::Owned(rewritten);
    }
    text
}

// The engine writes a formula back in capitals and without spaces, except inside quotes.
fn normalized_formula(text: &str) -> Option<String> {
    if !text.starts_with('=') {
        return None;
    }
    let mut normalized = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(open) => {
                normalized.push(c);
                if c == open {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                normalized.push(c);
            }
            None if c.is_whitespace() => {}
            None => normalized.extend(c.to_uppercase()),
        }
    }
    Some(normalized)
}

// Whether the engine's input for a cell (and its stored number, when it is one) is what
// importing the file's field gives.
pub fn stands_for(
    input: &str,
    field: &str,
    decimal_mark: DecimalMark,
    date_order: DateOrder,
    stored: Option<f64>,
) -> bool {
    same_value(
        input,
        &import_input(field, decimal_mark, date_order),
        stored,
    ) || normalized_formula(field).is_some_and(|formula| normalized_formula(input) == Some(formula))
}

// A number as a reader of the file takes it back as the same one: no grouping, which it could
// not tell from decimal commas; in a decimal-comma file a plain decimal, keeping only a percent
// sign. Text that is not a number stays as it is.
pub fn number_text(source: &str, decimal_mark: DecimalMark) -> String {
    let (body, percent) = match source.strip_suffix('%') {
        Some(body) => (body, "%"),
        None => (source, ""),
    };
    let Some(value) = number_value(body).filter(|value| value.is_finite()) else {
        return source.to_string();
    };
    match decimal_mark {
        DecimalMark::Dot => format!("{}{percent}", body.replace(',', "")),
        DecimalMark::Comma => format!("{}{percent}", value.to_string().replace('.', ",")),
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
    fn a_number_is_written_without_grouping_and_with_the_files_decimal_mark() {
        assert_eq!(number_text("1,234.50", DecimalMark::Dot), "1234.50");
        assert_eq!(number_text("-0.25%", DecimalMark::Comma), "-0,25%");
        assert_eq!(number_text("v1.2.3", DecimalMark::Comma), "v1.2.3");
        assert_eq!(number_text("1,23", DecimalMark::Dot), "1,23");
    }

    #[test]
    fn a_decimal_comma_number_is_a_plain_decimal_the_import_reads_as_the_same_number() {
        let cases = [
            ("$1,234.50", 1234.5),
            ("1.5E+20", 1.5e20),
            ("-12%", -0.12),
            ("1,234", 1234.0),
            ("0.0000001", 1e-7),
        ];
        for (shown, value) in cases {
            let written = number_text(shown, DecimalMark::Comma);
            assert!(!written.contains(['$', 'E', 'e']), "{written}");
            let read = import_input(&written, DecimalMark::Comma, DateOrder::MonthFirst);
            let read = number_value(&read).unwrap();
            assert!(
                (read - value).abs() <= 1e-12 * value.abs(),
                "{shown} {written}"
            );
        }
    }

    #[test]
    fn a_formula_the_engine_writes_back_normalized_is_the_field_it_came_from() {
        let stands = |input: &str, field: &str| {
            stands_for(input, field, DecimalMark::Dot, DateOrder::MonthFirst, None)
        };
        assert!(stands("=SUM(A1:A2)", "=sum(a1:a2)"));
        assert!(stands("=1+1", "=1 + 1"));
        assert!(stands("=\"a b\"&\"c\"", "=\"a b\" & \"c\""));
        assert!(!stands("=\"A B\"", "=\"a b\""));
        assert!(!stands("=1+2", "=1 + 1"));
        assert!(!stands("2", "=1 + 1"));
    }

    #[test]
    fn a_comma_file_reads_dots_and_month_first_dates() {
        let parsed = parse_csv(b"d,n\n08/10/2026,1.5\n", None).unwrap();
        let guessed = guess(&parsed);
        assert_eq!(guessed.decimal_mark, DecimalMark::Dot);
        assert_eq!(guessed.date_order, DateOrder::MonthFirst);
    }
}
