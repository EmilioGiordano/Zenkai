// File-controlled names reach the agent, the picker and the labels, so every one of them is
// checked with these predicates. Format characters can reorder or hide text without being
// visible, which is why they are refused along with the control characters.
pub(super) fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{061C}'
                | '\u{FEFF}'
        )
}

pub(super) const MAX_SHEET_NAME_CHARS: usize = 31;

pub(super) fn is_safe_name(name: &str) -> bool {
    !name.chars().any(is_unsafe_char)
}

// The external form '[book]sheet' ends the book at the first closing bracket.
pub(super) fn can_hold_external_sheet(book: &str) -> bool {
    is_safe_name(book) && !book.contains(['[', ']'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_and_invisible_format_characters_are_unsafe() {
        for c in [
            '\n', '\u{7}', '\u{85}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}',
            '\u{2028}', '\u{2029}', '\u{202A}', '\u{202E}', '\u{2060}', '\u{2064}', '\u{2066}',
            '\u{2069}', '\u{061C}', '\u{FEFF}',
        ] {
            assert!(is_unsafe_char(c), "{c:?}");
        }
    }

    #[test]
    fn ordinary_names_with_accents_spaces_and_symbols_are_safe() {
        for name in [
            "Dinámica - 2024.11.20.xlsx",
            "año 2026",
            "売上",
            "a-b_c (1)",
            "it's",
        ] {
            assert!(is_safe_name(name), "{name}");
        }
    }

    #[test]
    fn brackets_cannot_sit_inside_the_external_form() {
        assert!(can_hold_external_sheet("budget.xlsx"));
        assert!(!can_hold_external_sheet("[a] b.xlsx"));
        assert!(!can_hold_external_sheet("a]b.xlsx"));
        assert!(!can_hold_external_sheet("evil\u{202E}.xlsx"));
    }
}
