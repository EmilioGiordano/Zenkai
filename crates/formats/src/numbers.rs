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

const CURRENCY_SYMBOLS: [char; 4] = ['$', '€', '£', '¥'];

// The number a cell's text stands for, in the English form the engine reads: a sign, a currency
// symbol around the digits, grouping commas, a decimal dot, an exponent and a trailing percent
// sign.
pub fn number_value(text: &str) -> Option<f64> {
    let text = text.trim();
    let (text, percent) = match text.strip_suffix('%') {
        Some(rest) => (rest.trim_end(), true),
        None => (text, false),
    };
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let body = text
        .trim_start_matches(CURRENCY_SYMBOLS)
        .trim_end_matches(CURRENCY_SYMBOLS)
        .trim();
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole = if whole.contains(',') {
        let groups: Vec<&str> = whole.split(',').collect();
        let well_formed = groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()))
            && groups
                .first()
                .is_some_and(|first| (1..=3).contains(&first.len()))
            && groups.iter().skip(1).all(|group| group.len() == 3);
        if !well_formed {
            return None;
        }
        groups.concat()
    } else if whole.bytes().all(|b| b.is_ascii_digit()) {
        whole.to_string()
    } else {
        return None;
    };
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let exponent = match exponent {
        None => "0",
        Some(exponent) => {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            exponent
        }
    };
    let value: f64 = format!("0{whole}.{fraction}0e{exponent}").parse().ok()?;
    let value = if percent { value / 100.0 } else { value };
    Some(if negative { -value } else { value })
}

// Whether the engine's input for a cell is what importing `expected` gives, up to how the
// engine writes it: the same text, the same number (to the 15 digits a spreadsheet keeps), or
// the same boolean in another case.
pub fn same_value(input: &str, expected: &str) -> bool {
    if input == expected {
        return true;
    }
    match (number_value(input), number_value(expected)) {
        (Some(a), Some(b)) => a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs()),
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
        ] {
            assert!(same_value(input, expected), "{input} {expected}");
        }
        for (input, expected) in [
            ("1.5", "1.6"),
            ("7", "007x"),
            ("1.5", "abc"),
            ("abc", "abd"),
            ("1234", "1,23"),
            ("TRUE", "false"),
            ("1", "inf"),
        ] {
            assert!(!same_value(input, expected), "{input} {expected}");
        }
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
            assert!(same_value(input, &expected), "{field} {input}");
        }
    }

    #[test]
    fn comparing_cells_costs_the_same_per_cell_however_many_there_are() {
        let started = std::time::Instant::now();
        let matches = (0..1_100_000)
            .filter(|n| same_value("1.5", if n % 2 == 0 { "1.50" } else { "$1.5" }))
            .count();
        assert_eq!(matches, 1_100_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(60));
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
