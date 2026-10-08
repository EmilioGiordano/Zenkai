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
