// Excel's Increase / Decrease Decimal: one more or one fewer zero after the decimal point
// in every section of the number format. A General cell starts from what it shows.

pub fn step_decimals(code: &str, shown: &str, more: bool) -> Option<String> {
    if code.eq_ignore_ascii_case("general") {
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
    let sections: Vec<String> = code
        .split(';')
        .map(|section| step_section(section, more).unwrap_or_else(|| section.to_string()))
        .collect();
    let stepped = sections.join(";");
    (stepped != code).then_some(stepped)
}

// Quoted text and bracketed parts ("[Red]", "\"kg\"") are left alone; dates have no
// digit placeholders and do not change.
fn step_section(section: &str, more: bool) -> Option<String> {
    let chars: Vec<char> = section.chars().collect();
    let mut quoted = false;
    let mut bracket = false;
    let mut point = None;
    let mut last_digit = None;
    for (index, c) in chars.iter().enumerate() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            _ if quoted || bracket => {}
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
            step_decimals("general", "3.14", true).as_deref(),
            Some("0.000")
        );
        assert_eq!(
            step_decimals("general", "3.14", false).as_deref(),
            Some("0.0")
        );
        assert_eq!(step_decimals("general", "7", true).as_deref(), Some("0.0"));
        assert_eq!(step_decimals("general", "7", false), None);
    }

    #[test]
    fn formats_gain_or_lose_a_zero_in_every_section() {
        assert_eq!(
            step_decimals("#,##0.00", "1", true).as_deref(),
            Some("#,##0.000")
        );
        assert_eq!(
            step_decimals("#,##0.0", "1", false).as_deref(),
            Some("#,##0")
        );
        assert_eq!(step_decimals("0%", "1", true).as_deref(), Some("0.0%"));
        assert_eq!(
            step_decimals("$#,##0.00;[Red]-$#,##0.00", "1", false).as_deref(),
            Some("$#,##0.0;[Red]-$#,##0.0")
        );
        assert_eq!(
            step_decimals("0.00\" kg\"", "1", true).as_deref(),
            Some("0.000\" kg\"")
        );
        assert_eq!(step_decimals("dd/mm/yyyy", "1", true), None);
        assert_eq!(step_decimals("0", "1", false), None);
    }
}
