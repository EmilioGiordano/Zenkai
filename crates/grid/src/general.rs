// Excel's General format never shows #### for a plain number: it drops decimals and then
// switches to scientific notation until the number fits the column.

/// Shorter spellings of a General number, from the most to the least precise.
pub fn shorter_spellings(text: &str) -> Vec<String> {
    let Ok(value) = text.parse::<f64>() else {
        return Vec::new();
    };
    if !value.is_finite() || value == 0.0 {
        return Vec::new();
    }
    let mut spellings = Vec::new();
    if let Some((_, decimals)) = text.split_once('.')
        && !text.contains(['e', 'E'])
    {
        for places in (0..decimals.len()).rev() {
            let rounded = round_decimal(text, places);
            let trimmed = if rounded.contains('.') {
                rounded.trim_end_matches('0').trim_end_matches('.')
            } else {
                rounded.as_str()
            };
            if trimmed
                .trim_start_matches('-')
                .chars()
                .any(|c| c != '0' && c != '.')
            {
                spellings.push(trimmed.to_string());
            }
        }
    }
    for places in (0..=5).rev() {
        spellings.push(scientific(value, places));
    }
    spellings.dedup();
    spellings
}

// Rounds the written digits half up, as Excel does, rather than the binary value
// (0.000012345 is 0.0000123449999... in binary).
fn round_decimal(text: &str, places: usize) -> String {
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if fraction.len() <= places {
        return text.to_string();
    }
    let mut kept: Vec<u8> = whole.bytes().chain(fraction.bytes().take(places)).collect();
    if fraction.as_bytes()[places] >= b'5' {
        let mut index = kept.len();
        loop {
            if index == 0 {
                kept.insert(0, b'1');
                break;
            }
            index -= 1;
            if kept[index] == b'9' {
                kept[index] = b'0';
            } else {
                kept[index] += 1;
                break;
            }
        }
    }
    let split = kept.len() - places;
    let (whole, fraction) = kept.split_at(split);
    let whole = String::from_utf8_lossy(whole);
    if places == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{}", String::from_utf8_lossy(fraction))
    }
}

// 123456789012 with 5 places is "1.23457E+11", as Excel writes it.
fn scientific(value: f64, places: usize) -> String {
    let formatted = format!("{value:.places$e}");
    let Some((mantissa, exponent)) = formatted.split_once('e') else {
        return formatted;
    };
    let mantissa = if mantissa.contains('.') {
        mantissa.trim_end_matches('0').trim_end_matches('.')
    } else {
        mantissa
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}E{sign}{:02}", exponent.abs())
}

#[cfg(test)]
mod tests {
    use super::shorter_spellings;

    #[test]
    fn large_integers_go_scientific_like_excel() {
        let spellings = shorter_spellings("123456789012");
        assert_eq!(spellings[0], "1.23457E+11");
        assert_eq!(spellings.last().map(String::as_str), Some("1E+11"));
    }

    #[test]
    fn decimals_round_before_going_scientific() {
        let spellings = shorter_spellings("3.14159265358979");
        assert_eq!(spellings[0], "3.1415926535898");
        assert!(spellings.contains(&"3.14".to_string()));
        assert!(spellings.contains(&"3".to_string()));
        assert_eq!(shorter_spellings("-0.000012345")[0], "-0.00001235");
        assert!(shorter_spellings("-0.000012345").contains(&"-1.23E-05".to_string()));
        assert_eq!(shorter_spellings("9.96")[0], "10");
    }

    #[test]
    fn text_and_zero_have_no_spellings() {
        assert!(shorter_spellings("abc").is_empty());
        assert!(shorter_spellings("0").is_empty());
    }
}
