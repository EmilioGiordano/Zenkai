// Numbers written with a decimal comma ("1.234,56", "12,5%"), as Excel exports them in
// locales such as es-AR. The engine reads a dot as the decimal separator, so these fields
// are rewritten before import; anything that is not exactly such a number is left alone.

/// True when some field uses a comma for decimals and none uses a dot. Every row is
/// scanned, so a dot decimal anywhere vetoes the rewrite of the whole file.
/// Only a file not separated by commas can use a decimal comma unquoted, so callers
/// usually check the delimiter first.
pub fn detect_decimal_comma(rows: &[Vec<String>]) -> bool {
    let mut comma = false;
    for field in rows.iter().flatten() {
        match shape(field) {
            Some(Shape::DecimalComma) => comma = true,
            Some(Shape::DecimalDot) => return false,
            _ => {}
        }
    }
    comma
}

/// Rewrites every decimal-comma number to the dot form; returns how many fields changed.
pub fn normalize_decimal_comma(rows: &mut [Vec<String>]) -> usize {
    let mut changed = 0;
    for field in rows.iter_mut().flatten() {
        if let Some(rewritten) = decimal_comma_field(field) {
            *field = rewritten;
            changed += 1;
        }
    }
    changed
}

// What `normalize_decimal_comma` makes of one field, so a save can tell what the import
// turned a field of the file into.
pub fn decimal_comma_field(field: &str) -> Option<String> {
    matches!(
        shape(field),
        Some(Shape::DecimalComma | Shape::Grouped | Shape::Ambiguous)
    )
    .then(|| field.replace('.', "").replace(',', "."))
}

const CURRENCIES: [char; 2] = ['$', '€'];

// The number a text stands for exactly as the engine's import parses it, and no other way: `$`
// and `€` are the currencies it knows, a percent sign goes last, and grouping commas need
// groups of three. A text it keeps as text is never taken for a number here.
pub fn number_value(text: &str) -> Option<f64> {
    let value = text.trim();
    if let Some(body) = value.strip_suffix('%') {
        return plain_number(body.trim()).map(|(number, _)| number / 100.0);
    }
    for currency in CURRENCIES {
        if let Some(body) = value
            .strip_prefix('-')
            .and_then(|rest| rest.strip_prefix(currency))
        {
            // The engine keeps the sign of a scientific number after "-$" as it was written.
            let (number, scientific) = plain_number(body.trim())?;
            return Some(if scientific { number } else { -number });
        }
        if let Some(body) = value
            .strip_prefix(currency)
            .or_else(|| value.strip_suffix(currency))
        {
            return plain_number(body.trim()).map(|(number, _)| number);
        }
    }
    plain_number(value).map(|(number, _)| number)
}

// A sign, digits with commas in groups of three, a decimal dot and an exponent, as the engine
// reads them; also whether there was an exponent.
fn plain_number(value: &str) -> Option<(f64, bool)> {
    let characters: Vec<char> = value.chars().collect();
    let len = characters.len();
    let mut position = 0;
    let sign = match characters.first()? {
        '-' => {
            position = 1;
            -1.0
        }
        '+' => {
            position = 1;
            1.0
        }
        _ => 1.0,
    };
    if position >= len || characters[position] == ',' {
        return None;
    }
    let mut digits = String::new();
    let mut groups = Vec::new();
    while position < len {
        let c = characters[position];
        if c.is_ascii_digit() {
            digits.push(c);
        } else if c == ',' {
            groups.push(digits.len());
        } else {
            break;
        }
        position += 1;
    }
    if groups.iter().any(|at| (digits.len() - at) % 3 != 0) {
        return None;
    }
    if position < len && characters[position] == '.' {
        digits.push('.');
        position += 1;
        while position < len && characters[position].is_ascii_digit() {
            digits.push(characters[position]);
            position += 1;
        }
    }
    let mut scientific = false;
    if position + 1 < len && matches!(characters[position], 'e' | 'E') {
        scientific = true;
        let next = characters[position + 1];
        if next == '-' || next == '+' || next.is_ascii_digit() {
            digits.push('e');
            digits.push(next);
            position += 2;
            while position < len && characters[position].is_ascii_digit() {
                digits.push(characters[position]);
                position += 1;
            }
        }
    }
    if position != len {
        return None;
    }
    let number: f64 = digits.parse().ok()?;
    Some((sign * number, scientific))
}

// A spreadsheet keeps 15 significant digits of a number, so two texts that agree to that many
// are one value and two that differ within them are not.
fn fifteen_digits(value: f64) -> f64 {
    format!("{value:.14e}").parse().unwrap_or(value)
}

fn significant_digits(text: &str) -> usize {
    let mantissa = text.split(['e', 'E']).next().unwrap_or(text);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    digits.trim_matches('0').len()
}

// Text of up to 15 significant digits stands for a stored value when they agree to 15 digits;
// one with more digits has to be exactly that value.
fn agrees(parsed: f64, text: &str, stored: f64) -> bool {
    if !parsed.is_finite() || !stored.is_finite() {
        return false;
    }
    if significant_digits(text) > 15 {
        parsed == stored
    } else {
        fifteen_digits(parsed) == fifteen_digits(stored)
    }
}

fn reads_as(text: &str, stored: f64) -> bool {
    number_value(text).is_some_and(|parsed| agrees(parsed, text, stored))
}

// The text that gives a stored number back when a file is read: what the cell shows if that
// reads as the number, else the number itself. A constant of more than 15 digits is always
// written whole, as the cell shows only 15; a formula's result is written as shown, as a
// spreadsheet does.
pub fn stored_number_text(shown: &str, stored: f64, formula: bool) -> String {
    let whole = stored.to_string();
    if (!formula && significant_digits(&whole) > 15) || !reads_as(shown, stored) {
        whole
    } else {
        shown.to_string()
    }
}

// Whether the engine's input for a cell is what importing `expected` gives, up to how the
// engine writes it: the same text, the same number (against the stored one when it is known,
// else to the 15 digits the input text keeps), or the same boolean in another case.
pub fn same_value(input: &str, expected: &str, stored: Option<f64>) -> bool {
    if input == expected {
        return true;
    }
    match (number_value(input), number_value(expected)) {
        (Some(typed), Some(imported)) => match stored {
            Some(stored) => agrees(imported, expected, stored),
            None => {
                typed.is_finite()
                    && imported.is_finite()
                    && fifteen_digits(typed) == fifteen_digits(imported)
            }
        },
        _ => {
            ["true", "false"]
                .iter()
                .any(|word| input.eq_ignore_ascii_case(word))
                && input.eq_ignore_ascii_case(expected)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Shape {
    /// "1,5", "1.234,5", "-0,25%"
    DecimalComma,
    /// "1.5" or "12.3456": a dot that cannot be a thousands separator.
    DecimalDot,
    /// "1.234.567": dots that can only be thousands separators.
    Grouped,
    /// "1.234": a thousands separator or three decimals, depending on the locale.
    Ambiguous,
}

fn shape(field: &str) -> Option<Shape> {
    let body = field.strip_prefix(['-', '+']).unwrap_or(field);
    let body = body.strip_suffix('%').unwrap_or(body);
    let (whole, decimals) = match body.split_once(',') {
        Some((whole, decimals)) => (whole, Some(decimals)),
        None => (body, None),
    };
    if let Some(decimals) = decimals
        && (decimals.is_empty() || !decimals.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let groups: Vec<&str> = whole.split('.').collect();
    if groups
        .iter()
        .any(|g| g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    // "0.123" or "007.5": a leading zero cannot start a thousands group.
    if groups.len() > 1 && groups[0].starts_with('0') {
        return if decimals.is_none() && groups.len() == 2 {
            Some(Shape::DecimalDot)
        } else {
            None
        };
    }
    if decimals.is_some() {
        let grouped =
            groups.len() == 1 || (groups[0].len() <= 3 && groups[1..].iter().all(|g| g.len() == 3));
        return grouped.then_some(Shape::DecimalComma);
    }
    match groups.len() {
        1 => None,
        2 if groups[1].len() == 3 && groups[0].len() <= 3 => Some(Shape::Ambiguous),
        2 => Some(Shape::DecimalDot),
        _ if groups[0].len() <= 3 && groups[1..].iter().all(|g| g.len() == 3) => {
            Some(Shape::Grouped)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(fields: &[&str]) -> Vec<Vec<String>> {
        vec![fields.iter().map(ToString::to_string).collect()]
    }

    #[test]
    fn detects_and_rewrites_decimal_commas() {
        let mut data = rows(&[
            "Total",
            "1.234,56",
            "-0,5",
            "12,5%",
            "1.234.567",
            "2026",
            "a,b",
        ]);
        assert!(detect_decimal_comma(&data));
        assert_eq!(normalize_decimal_comma(&mut data), 4);
        assert_eq!(
            data[0],
            [
                "Total", "1234.56", "-0.5", "12.5%", "1234567", "2026", "a,b"
            ]
        );
    }

    #[test]
    fn the_same_value_is_told_whatever_way_the_engine_writes_it() {
        for (input, expected) in [
            ("7", "007"),
            ("1.5", "1.50"),
            ("1000", "1e3"),
            ("100000", "1E5"),
            ("0.12345", "12.345%"),
            ("12.345%", "12.345%"),
            ("1.999", "$1.999"),
            ("-5", "-$5"),
            ("1234.5678", "1,234.5678"),
            ("-0.1", "-0.10"),
            ("1234567890123", "1234567890123"),
            ("0.000000001", "1E-9"),
            ("TRUE", "true"),
            ("abc", "abc"),
            ("12345678901234600", "12345678901234567"),
            ("1.5", "1.50"),
        ] {
            assert!(same_value(input, expected, None), "{input} {expected}");
        }
        for (input, expected) in [
            ("1.5", "1.6"),
            ("7", "007x"),
            ("1.5", "abc"),
            ("abc", "abd"),
            ("1234", "1,23"),
            ("TRUE", "false"),
            ("1", "inf"),
            ("9780201379624", "9780201379625"),
            ("1", "1.00000000000001"),
            ("1e999", "1e998"),
        ] {
            assert!(!same_value(input, expected, None), "{input} {expected}");
        }
    }

    #[test]
    fn a_number_is_what_the_engine_import_reads_and_nothing_more() {
        for (text, value) in [
            ("5", 5.0),
            (" 5 ", 5.0),
            ("-$5", -5.0),
            ("$ 5", 5.0),
            ("5$", 5.0),
            ("\u{20ac}5", 5.0),
            ("5 %", 0.05),
            ("-1,234.5", -1234.5),
            ("1e3", 1000.0),
            (".5", 0.5),
        ] {
            assert_eq!(number_value(text), Some(value), "{text}");
        }
        for text in [
            "\u{a3}5", "\u{a5}5", "+$5", "$5%", "$5$", "- 5", "1,23", ",5", "5e", "inf", "", "$",
            "1.2.3", "5 000",
        ] {
            assert_eq!(number_value(text), None, "{text}");
        }
    }

    #[test]
    fn a_currency_the_engine_keeps_as_text_is_not_the_number_it_looks_like() {
        assert!(!same_value("5", "\u{a3}5", None));
        assert!(!same_value("\u{a3}7", "7", None));
        assert!(same_value("5", "$5", None));
    }

    #[test]
    fn numbers_past_15_digits_are_the_same_only_when_exactly_the_same() {
        let field = "1234567890123456";
        assert!(same_value("1234567890123460", field, None));
        assert!(same_value(
            "1234567890123460",
            field,
            Some(1_234_567_890_123_456.0)
        ));
        assert!(!same_value(
            "1234567890123460",
            field,
            Some(1_234_567_890_123_457.0)
        ));
        let long = "12345678901234567";
        assert!(same_value(
            "12345678901234568",
            long,
            Some(12_345_678_901_234_567.0)
        ));
        assert!(!same_value(
            "12345678901234568",
            long,
            Some(12_345_678_901_234_580.0)
        ));
        assert!(same_value("0.5", "0.50", Some(0.5)));
        assert!(reads_as("1,234.5", 1234.5));
        assert!(!reads_as("1.23457E+15", 1_234_567_890_123_457.0));
        assert!(!reads_as("text", 1.0));
    }

    #[test]
    fn a_stored_number_is_written_as_shown_unless_that_loses_digits_it_was_typed_with() {
        let sum = 0.1 + 0.2;
        assert_eq!(stored_number_text("0.3", sum, true), "0.3");
        assert_eq!(stored_number_text("0.3", sum, false), "0.30000000000000004");
        assert_eq!(
            stored_number_text("1.23457E+15", 1_234_567_890_123_457.0, false),
            "1234567890123457"
        );
        assert_eq!(stored_number_text("1,234.50", 1234.5, false), "1,234.50");
        assert_eq!(stored_number_text("12%", 0.12, false), "12%");
        assert_eq!(stored_number_text("1.5E+20", 1.5e20, false), "1.5E+20");
    }

    #[test]
    fn a_decimal_comma_file_is_compared_after_the_import_rewrites_its_numbers() {
        for (field, input) in [
            ("1.234.567,89", "1234567.89"),
            ("1,50", "1.5"),
            ("00123", "123"),
            ("12,345%", "0.12345"),
            ("1.234", "1234"),
        ] {
            let expected = decimal_comma_field(field).unwrap_or_else(|| field.to_string());
            assert!(same_value(input, &expected, None), "{field} {input}");
        }
    }

    #[test]
    fn a_decimal_dot_means_the_file_is_not_decimal_comma() {
        assert!(!detect_decimal_comma(&rows(&["1,5", "2.75"])));
        assert!(!detect_decimal_comma(&rows(&["1.234", "7"])));
        assert!(!detect_decimal_comma(&rows(&["1,5", "0.123"])));
        assert!(!detect_decimal_comma(&rows(&["1,5", "0.500"])));
        let mut late_dot = vec![vec!["1,5".to_string()]; 5_000];
        late_dot.push(vec!["2.75".to_string()]);
        assert!(!detect_decimal_comma(&late_dot));
    }

    #[test]
    fn leaves_non_numbers_alone() {
        for field in [
            "1,",
            ",5",
            "1,2,3",
            "12.34,5",
            "1.2.3",
            "",
            "x1,5",
            "1 234,5",
            "0.123,5",
            "007.123.456",
        ] {
            assert_eq!(shape(field), None, "{field}");
        }
    }
}

#[cfg(test)]
mod no_panic {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

        #[test]
        fn number_rewrite_accepts_any_field(field in prop_oneof![any::<String>(), "[-+0-9.,%]{0,16}"]) {
            let mut rows = vec![vec![field]];
            let _ = super::detect_decimal_comma(&rows);
            let _ = super::normalize_decimal_comma(&mut rows);
        }
    }
}
