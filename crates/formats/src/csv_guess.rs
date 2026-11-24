use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::csv_format::{Delimiter, ParsedCsv};
use crate::dates::{DateOrder, detect_date_order, normalize_day_first};
use crate::numbers::{detect_decimal_comma, normalize_decimal_comma, number_value};

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

// The engine reads a leading '+' or '-' as a formula unless it parses as a number;
// an apostrophe forces non-numeric signed fields to stay text on import.
pub fn import_input(field: &str) -> Cow<'_, str> {
    let signed_text =
        field.starts_with(['+', '-']) && number_value(field).is_none_or(|value| !value.is_finite());
    if signed_text {
        Cow::Owned(format!("'{field}"))
    } else {
        Cow::Borrowed(field)
    }
}

pub fn prepare_import_inputs(rows: &mut [Vec<String>]) {
    for field in rows.iter_mut().flatten() {
        let rewritten = match import_input(field) {
            Cow::Owned(text) => Some(text),
            Cow::Borrowed(_) => None,
        };
        if let Some(text) = rewritten {
            *field = text;
        }
    }
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
    use crate::numbers::decimal_comma_field;

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
            let read = decimal_comma_field(&written).unwrap_or_else(|| written.clone());
            let read = number_value(&read).unwrap();
            assert!(
                (read - value).abs() <= 1e-12 * value.abs(),
                "{shown} {written}"
            );
        }
    }

    #[test]
    fn a_comma_file_reads_dots_and_month_first_dates() {
        let parsed = parse_csv(b"d,n\n08/10/2026,1.5\n", None).unwrap();
        let guessed = guess(&parsed);
        assert_eq!(guessed.decimal_mark, DecimalMark::Dot);
        assert_eq!(guessed.date_order, DateOrder::MonthFirst);
    }

    #[test]
    fn signed_non_numbers_become_text_and_numbers_and_formulas_stay() {
        for field in [
            "+34 600",
            "-abc",
            "+x",
            "-",
            "+",
            "-3,5",
            "-1+2",
            "+SUM(A1:A3)",
        ] {
            assert_eq!(import_input(field).into_owned(), format!("'{field}"));
        }
        for field in ["+5", "-3.5", "-12%", "-1,234.5", "-$5", "-\u{20ac}5"] {
            assert_eq!(import_input(field).as_ref(), field, "{field}");
        }
        for field in ["=1+1", "hello", ""] {
            assert_eq!(import_input(field).as_ref(), field, "{field}");
        }
    }

    #[test]
    fn a_signed_decimal_comma_number_stays_a_number_after_apply() {
        let mut rows = vec![vec![
            "-3,5".to_string(),
            "-1.234,5".to_string(),
            "-12,5%".to_string(),
            "+34 600".to_string(),
            "-".to_string(),
            "+5".to_string(),
        ]];
        apply(DecimalMark::Comma, DateOrder::DayFirst, &mut rows);
        let inputs: Vec<String> = rows[0]
            .iter()
            .map(|field| import_input(field).into_owned())
            .collect();
        assert_eq!(
            inputs,
            ["-3.5", "-1234.5", "-12.5%", "'+34 600", "'-", "+5"]
        );
    }

    #[test]
    fn prepare_import_inputs_marks_only_signed_non_numbers_in_place() {
        let mut rows = vec![
            vec![
                "+34 600".to_string(),
                "-abc".to_string(),
                "-".to_string(),
                "-3.5".to_string(),
            ],
            vec!["+5".to_string(), "=1+1".to_string(), "plain".to_string()],
        ];
        prepare_import_inputs(&mut rows);
        assert_eq!(rows[0], ["'+34 600", "'-abc", "'-", "-3.5"]);
        assert_eq!(rows[1], ["+5", "=1+1", "plain"]);
    }
}
