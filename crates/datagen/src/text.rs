const SHOWN_CHARS: usize = 80;

// User text echoed in an error is cut short so a hostile spec cannot flood a dialog or agent.
pub(crate) fn clip(text: &str) -> String {
    match text.char_indices().nth(SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

// Unicode general category Cf (Unicode 15.1): bidi overrides, zero-width and tag characters.
const FORMAT_CHARACTERS: &[(char, char)] = &[
    ('\u{00AD}', '\u{00AD}'),
    ('\u{0600}', '\u{0605}'),
    ('\u{061C}', '\u{061C}'),
    ('\u{06DD}', '\u{06DD}'),
    ('\u{070F}', '\u{070F}'),
    ('\u{0890}', '\u{0891}'),
    ('\u{08E2}', '\u{08E2}'),
    ('\u{180E}', '\u{180E}'),
    ('\u{200B}', '\u{200F}'),
    ('\u{202A}', '\u{202E}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206F}'),
    ('\u{FEFF}', '\u{FEFF}'),
    ('\u{FFF9}', '\u{FFFB}'),
    ('\u{110BD}', '\u{110BD}'),
    ('\u{110CD}', '\u{110CD}'),
    ('\u{13430}', '\u{1343F}'),
    ('\u{1BCA0}', '\u{1BCA3}'),
    ('\u{1D173}', '\u{1D17A}'),
    ('\u{E0001}', '\u{E0001}'),
    ('\u{E0020}', '\u{E007F}'),
];

pub(crate) fn is_format_character(symbol: char) -> bool {
    FORMAT_CHARACTERS
        .iter()
        .any(|&(first, last)| (first..=last).contains(&symbol))
}

pub(crate) fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|letter| match letter {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

// A leading apostrophe makes the engine, like Excel, store the rest as literal text. Without
// it "+54 9 11 ..." becomes a formula, "0042" loses its zeros and "TRUE" turns boolean.
pub(crate) fn literal_input(text: String) -> String {
    let starts_with_letter = text.chars().next().is_some_and(char::is_alphabetic);
    let is_boolean = text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false");
    let may_be_date = text.contains(|c: char| c.is_ascii_digit()) && !text.contains(' ');
    if starts_with_letter && !is_boolean && !may_be_date {
        text
    } else {
        format!("'{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_short_text_and_cuts_long_text_on_a_char_boundary() {
        let exact = "ñ".repeat(SHOWN_CHARS);
        assert_eq!(clip(&exact), exact);
        assert_eq!(clip(&"ñ".repeat(SHOWN_CHARS + 1)), format!("{exact}…"));
    }

    #[test]
    fn fold_lowercases_and_strips_accents() {
        assert_eq!(fold("Teléfono ÑANDÚ Güemes"), "telefono nandu guemes");
    }

    #[test]
    fn literal_input_quotes_only_text_the_engine_would_reinterpret() {
        assert_eq!(literal_input("Lucía".into()), "Lucía");
        assert_eq!(
            literal_input("Av. Corrientes 1234".into()),
            "Av. Corrientes 1234"
        );
        assert_eq!(
            literal_input("+54 9 11 4821-3307".into()),
            "'+54 9 11 4821-3307"
        );
        assert_eq!(literal_input("0042".into()), "'0042");
        assert_eq!(literal_input("True".into()), "'True");
        assert_eq!(literal_input("MAR-12-2020".into()), "'MAR-12-2020");
        assert_eq!(literal_input("=SUM(A1)".into()), "'=SUM(A1)");
    }
}
