#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatCategory {
    General,
    Number,
    Currency,
    Accounting,
    Date,
    Time,
    Percentage,
    Fraction,
    Scientific,
    Text,
    Custom,
}

const CURRENCY_SYMBOLS: [char; 4] = ['$', '€', '£', '¥'];

impl FormatCategory {
    pub fn of(code: &str) -> Self {
        let code = code.trim();
        if code.is_empty() || code.eq_ignore_ascii_case("general") {
            return Self::General;
        }
        let section = code.split(';').next().unwrap_or(code);
        let core = without_literals(section);
        if core == "@" {
            return Self::Text;
        }
        if let Some(category) = date_or_time(&core) {
            return category;
        }
        let upper = core.to_ascii_uppercase();
        if upper.contains("E+") || upper.contains("E-") {
            return Self::Scientific;
        }
        if core.contains('/') {
            return Self::Fraction;
        }
        if core.contains('%') {
            return if is_plain_number(&core.replace('%', "")) {
                Self::Percentage
            } else {
                Self::Custom
            };
        }
        Self::Custom
    }
}

fn date_or_time(core: &str) -> Option<FormatCategory> {
    let lower = core.to_ascii_lowercase();
    let has = |tokens: &[char]| lower.chars().any(|c| tokens.contains(&c));
    if has(&['d', 'y']) {
        Some(FormatCategory::Date)
    } else if has(&['h', 's', 'a', 'p']) {
        Some(FormatCategory::Time)
    } else if has(&['m']) {
        Some(FormatCategory::Date)
    } else {
        None
    }
}

// The digit placeholders Excel's Number category offers: 0, 0.00, #,##0 and #,##0.00.
fn is_plain_number(core: &str) -> bool {
    let digits: String = core
        .chars()
        .filter(|c| !matches!(c, ' ' | '_' | '(' | ')' | '-' | '+') && !CURRENCY_SYMBOLS.contains(c))
        .collect();
    let Some(decimals) = digits
        .strip_prefix("#,##0")
        .or_else(|| digits.strip_prefix('0'))
    else {
        return false;
    };
    match decimals.strip_prefix('.') {
        None => decimals.is_empty(),
        Some(zeros) => !zeros.is_empty() && zeros.chars().all(|c| c == '0'),
    }
}

// Drops quoted text, escapes and bracket tags so only format tokens remain. Currency
// symbols and elapsed-time tags such as [h] survive as the token they stand for.
fn without_literals(section: &str) -> String {
    let mut core = String::new();
    let mut chars = section.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for literal in chars.by_ref().take_while(|&l| l != '"') {
                    if CURRENCY_SYMBOLS.contains(&literal) {
                        core.push(literal);
                    }
                }
            }
            '\\' => {
                if let Some(literal) = chars.next()
                    && CURRENCY_SYMBOLS.contains(&literal)
                {
                    core.push(literal);
                }
            }
            '[' => {
                let tag: String = chars.by_ref().take_while(|&t| t != ']').collect();
                match tag.chars().next() {
                    Some('$') => core.push('$'),
                    Some(elapsed @ ('h' | 'H' | 'm' | 'M' | 's' | 'S')) => core.push(elapsed),
                    _ => {}
                }
            }
            _ => core.push(c),
        }
    }
    core
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_and_text() {
        assert_eq!(FormatCategory::of("General"), FormatCategory::General);
        assert_eq!(FormatCategory::of("general"), FormatCategory::General);
        assert_eq!(FormatCategory::of(""), FormatCategory::General);
        assert_eq!(FormatCategory::of("@"), FormatCategory::Text);
    }

    #[test]
    fn percent_codes_are_percentage() {
        assert_eq!(FormatCategory::of("0%"), FormatCategory::Percentage);
        assert_eq!(FormatCategory::of("0.00%"), FormatCategory::Percentage);
    }

    #[test]
    fn date_codes() {
        for code in ["dd/mm/yyyy", "m/d/yyyy", "d-mmm-yy", "mmmm yyyy", "mmm-yy", "yyyy-mm-dd h:mm"] {
            assert_eq!(FormatCategory::of(code), FormatCategory::Date, "{code}");
        }
    }

    #[test]
    fn time_codes() {
        for code in ["h:mm:ss", "h:mm AM/PM", "[h]:mm:ss", "mm:ss", "[$-409]h:mm:ss AM/PM"] {
            assert_eq!(FormatCategory::of(code), FormatCategory::Time, "{code}");
        }
    }

    #[test]
    fn scientific_codes() {
        for code in ["0.00E+00", "0E+0", "##0.0E+0"] {
            assert_eq!(FormatCategory::of(code), FormatCategory::Scientific, "{code}");
        }
    }

    #[test]
    fn fraction_codes() {
        for code in ["# ?/?", "# ??/??", "# ?/8", "# ?/16"] {
            assert_eq!(FormatCategory::of(code), FormatCategory::Fraction, "{code}");
        }
    }

    #[test]
    fn percentage_decimals_do_not_matter() {
        for code in ["0%", "0.0%", "0.000%", "0.00%;[Red]-0.00%"] {
            assert_eq!(FormatCategory::of(code), FormatCategory::Percentage, "{code}");
        }
    }

    #[test]
    fn percent_with_other_tokens_is_custom() {
        assert_eq!(FormatCategory::of("0.0% ##x"), FormatCategory::Custom);
    }
}
