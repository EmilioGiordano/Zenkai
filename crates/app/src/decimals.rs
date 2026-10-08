// Excel's Increase / Decrease Decimal: one more or one fewer zero after the decimal point
// in every section of the number format. A General number starts from what it shows.

pub fn step_decimals(code: &str, shown_number: Option<&str>, more: bool) -> Option<String> {
    if code.eq_ignore_ascii_case("general") {
        let shown = shown_number?;
        let places = shown.split_once('.').map_or(0, |(_, fraction)| {
            fraction.chars().take_while(char::is_ascii_digit).count()
        });
        let places = if more {
            places + 1
        } else {
            places.checked_sub(1)?
        };
        return Some(if places == 0 {
            "0".to_string()
        } else {
            format!("0.{}", "0".repeat(places))
        });
    }
    let sections: Vec<String> = split_sections(code)
        .into_iter()
        .map(|section| step_section(section, more).unwrap_or_else(|| section.to_string()))
        .collect();
    let stepped = sections.join(";");
    (stepped != code).then_some(stepped)
}

// Sections are separated by ';' outside quoted text.
fn split_sections(code: &str) -> Vec<&str> {
    let mut sections = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (index, c) in code.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                sections.push(&code[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    sections.push(&code[start..]);
    sections
}

// Quoted text, escaped characters and bracketed parts never change. Dates, times and
// fractions have no decimals to step, as in Excel.
fn step_section(section: &str, more: bool) -> Option<String> {
    let chars: Vec<char> = section.chars().collect();
    let mut quoted = false;
    let mut bracket = false;
    let mut escaped = false;
    let mut point = None;
    let mut last_digit = None;
    for (index, c) in chars.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '\\' => escaped = true,
            '[' => bracket = true,
            ']' => bracket = false,
            _ if bracket => {}
            'd' | 'D' | 'm' | 'M' | 'y' | 'Y' | 'h' | 'H' | 's' | 'S' | '/' => return None,
            '.' if point.is_none() => point = Some(index),
            '0' | '#' | '?' => last_digit = Some(index),
            'E' | 'e' => break,
            _ => {}
        }
    }
    let last_digit = last_digit?;
    let mut out = chars.clone();
    match (point, more) {
        (Some(_), true) => out.insert(last_digit + 1, '0'),
        (None, true) => {
            out.insert(last_digit + 1, '0');
            out.insert(last_digit + 1, '.');
        }
        (Some(point), false) if last_digit > point => {
            out.remove(last_digit);
            if last_digit == point + 1 {
                out.remove(point);
            }
        }
        _ => return None,
    }
    Some(out.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::step_decimals;

    #[test]
    fn general_starts_from_the_shown_decimals() {
        assert_eq!(
            step_decimals("general", Some("3.14"), true).as_deref(),
            Some("0.000")
        );
        assert_eq!(
            step_decimals("general", Some("3.14"), false).as_deref(),
            Some("0.0")
        );
        assert_eq!(
            step_decimals("general", Some("7"), true).as_deref(),
            Some("0.0")
        );
        assert_eq!(step_decimals("general", Some("7"), false), None);
        assert_eq!(step_decimals("general", None, true), None);
    }

    #[test]
    fn formats_gain_or_lose_a_zero_in_every_section() {
        let step = |code: &str, more| step_decimals(code, Some("1"), more);
        assert_eq!(step("#,##0.00", true).as_deref(), Some("#,##0.000"));
        assert_eq!(step("#,##0.0", false).as_deref(), Some("#,##0"));
        assert_eq!(step("0%", true).as_deref(), Some("0.0%"));
        assert_eq!(
            step("$#,##0.00;[Red]-$#,##0.00", false).as_deref(),
            Some("$#,##0.0;[Red]-$#,##0.0")
        );
        assert_eq!(step("0.00\" kg\"", true).as_deref(), Some("0.000\" kg\""));
        assert_eq!(step("0.00E+00", true).as_deref(), Some("0.000E+00"));
        assert_eq!(step("0.0\"x;0\"", true).as_deref(), Some("0.00\"x;0\""));
    }

    #[test]
    fn dates_times_fractions_and_text_do_not_change() {
        let step = |code: &str| step_decimals(code, Some("1"), true);
        for code in ["dd/mm/yyyy", "mm:ss.0", "# ?/?", "@", "\\0", "0"] {
            if code == "0" {
                assert_eq!(step_decimals(code, Some("1"), false), None);
            } else {
                assert_eq!(step(code), None, "{code}");
            }
        }
    }
}

#[cfg(test)]
mod no_panic {
    use proptest::prelude::*;

    const FORMAT_CHARS: [char; 24] = [
        '0', '#', '?', '.', ',', ';', '%', '$', '"', '\\', '[', ']', 'A', 'd', 'y', 'm', 'h', 's',
        'E', '+', '_', ' ', '/', '@',
    ];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

        #[test]
        fn decimal_steps_accept_any_code(
            code in prop_oneof![
                any::<String>(),
                prop::collection::vec(
                    prop::sample::select(&FORMAT_CHARS[..]),
                    0..30,
                )
                .prop_map(|chars| chars.into_iter().collect::<String>()),
            ],
            shown in any::<String>(),
            more in any::<bool>(),
        ) {
            let _ = super::step_decimals(&code, Some(&shown), more);
        }

    }
}
