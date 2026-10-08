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
        if matches!(
            shape(field),
            Some(Shape::DecimalComma | Shape::Grouped | Shape::Ambiguous)
        ) {
            *field = field.replace('.', "").replace(',', ".");
            changed += 1;
        }
    }
    changed
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
