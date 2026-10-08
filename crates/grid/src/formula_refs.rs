use zenkai_types::{CellPos, Range};

// Characters after which Excel's point mode lets arrows or clicks insert a reference.
const INSERT_AFTER: &[char] = &[
    '=', '+', '-', '*', '/', '^', '(', ',', ';', ':', '<', '>', '&',
];

pub fn accepts_reference(text: &str, caret: usize) -> bool {
    text.starts_with('=')
        && caret == text.len()
        && text
            .trim_end()
            .chars()
            .last()
            .is_some_and(|c| INSERT_AFTER.contains(&c))
}

pub fn reference_text(anchor: CellPos, corner: CellPos) -> String {
    let range = Range::new(anchor, corner);
    range.to_string()
}

// Same-sheet A1 references in a formula, in order, for colouring them on the grid.
// Text inside string literals and sheet-qualified references are skipped.
pub fn references(formula: &str) -> Vec<Range> {
    if !formula.starts_with('=') {
        return Vec::new();
    }
    let bytes = formula.as_bytes();
    let mut found = Vec::new();
    let mut i = 1;
    let mut in_string = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        let previous = bytes[i - 1];
        let starts_token = !(previous.is_ascii_alphanumeric()
            || matches!(previous, b'_' | b'.' | b'!' | b'$' | b'\''));
        if in_string || !starts_token || !(b == b'$' || b.is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        let end = bytes[i..]
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'$' | b':')))
            .map_or(bytes.len(), |p| i + p);
        let token = &formula[i..end];
        let followed_by_call = bytes.get(end) == Some(&b'(') || bytes.get(end) == Some(&b'!');
        if !followed_by_call && let Some(range) = Range::parse_a1(token) {
            found.push(range);
        }
        i = end.max(i + 1);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(formula: &str) -> Vec<String> {
        references(formula)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn finds_references_but_not_functions_strings_or_other_sheets() {
        assert_eq!(texts("=SUM(A1:B3)+$C$4*d5"), ["A1:B3", "C4", "D5"]);
        assert_eq!(texts("=\"A1\"&B2"), ["B2"]);
        assert_eq!(texts("=Sheet2!A1+A2"), ["A2"]);
        assert_eq!(texts("=LOG10(100)"), Vec::<String>::new());
        assert!(texts("A1").is_empty());
    }

    #[test]
    fn point_mode_only_after_an_operator_at_the_end() {
        assert!(accepts_reference("=", 1));
        assert!(accepts_reference("=SUM(", 5));
        assert!(accepts_reference("=A1+", 4));
        assert!(!accepts_reference("=A1", 3));
        assert!(!accepts_reference("=A1+", 2));
        assert!(!accepts_reference("A1+", 3));
    }
}

/// Excel's F4 while editing a formula: the reference that ends at or contains `caret`
/// cycles A1 → $A$1 → A$1 → $A1 → A1. Returns the new text and caret.
pub fn cycle_reference(text: &str, caret: usize) -> Option<(String, usize)> {
    if !text.starts_with('=') || caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let is_ref_char = |c: char| c.is_ascii_alphanumeric() || c == '$';
    let start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, c)| !is_ref_char(*c))
        .map_or(0, |(at, c)| at + c.len_utf8());
    let end = text[caret..]
        .find(|c: char| !is_ref_char(c))
        .map_or(text.len(), |at| caret + at);
    let token = &text[start..end];
    let bare: String = token.chars().filter(|c| *c != '$').collect();
    let letters = bare.chars().take_while(char::is_ascii_alphabetic).count();
    let (col, row) = bare.split_at(letters);
    let valid = (1..=3).contains(&col.len())
        && !row.is_empty()
        && row.chars().all(|c| c.is_ascii_digit())
        && !row.starts_with('0');
    if !valid {
        return None;
    }
    let col_fixed = token.starts_with('$');
    let row_fixed = token.get(1..).is_some_and(|rest| rest.contains('$'));
    let next = match (col_fixed, row_fixed) {
        (false, false) => format!("${col}${row}"),
        (true, true) => format!("{col}${row}"),
        (false, true) => format!("${col}{row}"),
        (true, false) => format!("{col}{row}"),
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push_str(&text[..start]);
    out.push_str(&next);
    out.push_str(&text[end..]);
    Some((out, start + next.len()))
}

#[cfg(test)]
mod cycle_tests {
    use super::cycle_reference;

    #[test]
    fn f4_cycles_like_excel() {
        let mut text = "=SUM(A1)".to_string();
        let mut seen = Vec::new();
        let mut caret = 6;
        for _ in 0..4 {
            let (next, at) = cycle_reference(&text, caret).unwrap();
            text = next;
            caret = at;
            seen.push(text.clone());
        }
        assert_eq!(seen, ["=SUM($A$1)", "=SUM(A$1)", "=SUM($A1)", "=SUM(A1)"]);
        assert_eq!(cycle_reference("=B12+1", 4).unwrap().0, "=$B$12+1");
        assert_eq!(cycle_reference("=SUM(", 5), None);
        assert_eq!(cycle_reference("A1", 2), None);
        assert_eq!(cycle_reference("=ñA1", 5).unwrap().0, "=ñ$A$1");
        assert_eq!(cycle_reference("=é", 3), None);
    }
}
