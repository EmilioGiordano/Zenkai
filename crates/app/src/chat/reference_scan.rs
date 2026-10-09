use std::borrow::Cow;

use zenkai_types::Range;

use super::reference::Reference;

const MAX_SHEET_NAME_CHARS: usize = 31;
const FORBIDDEN_IN_SHEET_NAME: [char; 7] = ['[', ']', ':', '\\', '/', '?', '*'];
const MAX_CELL_CHARS: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Reference(Reference),
}

struct Found {
    start: usize,
    end: usize,
    reference: Reference,
}

fn is_cell_start(chars: &[char], at: usize) -> Option<usize> {
    let mut end = at;
    if chars.get(end) == Some(&'$') {
        end += 1;
    }
    let letters = end;
    while chars.get(end).is_some_and(char::is_ascii_uppercase) {
        end += 1;
    }
    if !(1..=3).contains(&(end - letters)) {
        return None;
    }
    if chars.get(end) == Some(&'$') {
        end += 1;
    }
    let digits = end;
    while chars.get(end).is_some_and(char::is_ascii_digit) {
        end += 1;
    }
    ((1..=7).contains(&(end - digits)) && end - at <= MAX_CELL_CHARS).then_some(end)
}

fn parse_range(chars: &[char], at: usize) -> Option<(Range, usize)> {
    let first_end = is_cell_start(chars, at)?;
    let mut end = first_end;
    if chars.get(first_end) == Some(&':')
        && let Some(second_end) = is_cell_start(chars, first_end + 1)
    {
        end = second_end;
    }
    let text: String = chars[at..end].iter().collect();
    Some((Range::parse_a1(&text)?, end))
}

fn parse_sheet(chars: &[char], at: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut index = at;
    if chars.get(at) == Some(&'\'') {
        index += 1;
        loop {
            match (chars.get(index)?, chars.get(index + 1)) {
                ('\'', Some('\'')) => {
                    name.push('\'');
                    index += 2;
                }
                ('\'', _) => {
                    index += 1;
                    break;
                }
                (c, _) => {
                    name.push(*c);
                    index += 1;
                }
            }
        }
    } else {
        let first = *chars.get(at)?;
        if !(first.is_alphabetic() || first == '_') {
            return None;
        }
        while let Some(c) = chars.get(index)
            && (c.is_alphanumeric() || *c == '_' || *c == '.')
        {
            name.push(*c);
            index += 1;
        }
    }
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_SHEET_NAME_CHARS
        && !name.contains(FORBIDDEN_IN_SHEET_NAME)
        && !name.contains(char::is_control);
    (valid && chars.get(index) == Some(&'!')).then_some((name, index + 1))
}

fn boundary_before(chars: &[char], at: usize) -> bool {
    match at.checked_sub(1).map(|before| chars[before]) {
        None => true,
        Some(c) if c.is_whitespace() => true,
        Some('"' | '“' | '{' | '*' | ',' | ';' | '>') => true,
        Some('(') => at < 2 || !chars[at - 2].is_alphanumeric(),
        Some(_) => false,
    }
}

fn boundary_after(chars: &[char], end: usize) -> bool {
    match chars.get(end) {
        None => true,
        Some(c) if c.is_whitespace() => true,
        Some('.' | ':' | ',') => !chars
            .get(end + 1)
            .is_some_and(|next| next.is_alphanumeric()),
        Some(';' | '?' | '!' | ')' | ']' | '}' | '"' | '”' | '*' | '\'') => true,
        Some(_) => false,
    }
}

// A bare single cell such as Q3 or CO2 is ordinary text, so a reference needs a sheet or a range.
fn reference_at(chars: &[char], at: usize) -> Option<(Reference, usize)> {
    if !boundary_before(chars, at) {
        return None;
    }
    let (sheet, cell_at) = match parse_sheet(chars, at) {
        Some((sheet, after)) => (Some(sheet), after),
        None => (None, at),
    };
    let (range, end) = parse_range(chars, cell_at)?;
    let qualified = sheet.is_some() || range.start != range.end;
    (qualified && boundary_after(chars, end)).then_some((Reference { sheet, range }, end))
}

fn find(chars: &[char], skip: impl Fn(&[char], usize) -> Option<usize>) -> Vec<Found> {
    let mut found = Vec::new();
    let mut offsets = Vec::with_capacity(chars.len() + 1);
    let mut bytes = 0;
    for c in chars {
        offsets.push(bytes);
        bytes += c.len_utf8();
    }
    offsets.push(bytes);
    let mut at = 0;
    while at < chars.len() {
        if let Some(after) = skip(chars, at) {
            at = after;
        } else if let Some((reference, end)) = reference_at(chars, at) {
            found.push(Found {
                start: offsets[at],
                end: offsets[end],
                reference,
            });
            at = end;
        } else {
            at += 1;
        }
    }
    found
}

pub fn split(text: &str) -> Vec<Part> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut taken = 0;
    for found in find(&chars, |_, _| None) {
        if found.start > taken {
            parts.push(Part::Text(text[taken..found.start].to_string()));
        }
        parts.push(Part::Reference(found.reference));
        taken = found.end;
    }
    if taken < text.len() {
        parts.push(Part::Text(text[taken..].to_string()));
    }
    parts
}

pub fn parse_exact(text: &str) -> Option<Reference> {
    let chars: Vec<char> = text.chars().collect();
    match reference_at(&chars, 0) {
        Some((reference, end)) if end == chars.len() => Some(reference),
        _ => None,
    }
}

fn run_of(chars: &[char], at: usize, mark: char) -> usize {
    chars[at..].iter().take_while(|c| **c == mark).count()
}

fn line_starts_fence(chars: &[char], at: usize) -> Option<usize> {
    let line_start = at == 0 || chars[at - 1] == '\n';
    let indent = chars[at..].iter().take_while(|c| **c == ' ').count();
    let mark = *chars.get(at + indent)?;
    (line_start && indent < 4 && matches!(mark, '`' | '~') && run_of(chars, at + indent, mark) >= 3)
        .then_some(at + indent)
}

// The end of a stretch markdown must not be touched: a code fence, a code span, a link target
// or an autolink.
fn protected_end(chars: &[char], at: usize) -> Option<usize> {
    if let Some(fence) = line_starts_fence(chars, at) {
        let mark = chars[fence];
        let mut line = at;
        let mut opened = false;
        while line < chars.len() {
            let closes = opened && line_starts_fence(chars, line).is_some_and(|f| chars[f] == mark);
            let next = chars[line..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |newline| line + newline + 1);
            opened = true;
            line = next;
            if closes {
                break;
            }
        }
        return Some(line);
    }
    match chars[at] {
        '`' => {
            let length = run_of(chars, at, '`');
            let mut index = at + length;
            while index < chars.len() {
                if chars[index] == '`' {
                    let closing = run_of(chars, index, '`');
                    if closing == length {
                        return Some(index + closing);
                    }
                    index += closing;
                } else {
                    index += 1;
                }
            }
            Some(chars.len())
        }
        ']' if chars.get(at + 1) == Some(&'(') => Some(
            chars[at..]
                .iter()
                .position(|c| *c == ')')
                .map_or(chars.len(), |close| at + close + 1),
        ),
        '<' => chars[at..]
            .iter()
            .take_while(|c| **c != '\n')
            .position(|c| *c == '>')
            .map(|close| at + close + 1),
        _ => None,
    }
}

fn escape_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '<' | '&') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

pub const LINK_TARGET: &str = "zenkai-ref:";

// Turns each reference in an agent's answer into a link the text view renders as a badge.
pub fn link_references(markdown: &str) -> Cow<'_, str> {
    let chars: Vec<char> = markdown.chars().collect();
    let found = find(&chars, protected_end);
    if found.is_empty() {
        return Cow::Borrowed(markdown);
    }
    let mut linked = String::with_capacity(markdown.len() + found.len() * 16);
    let mut taken = 0;
    for item in found {
        linked.push_str(&markdown[taken..item.start]);
        linked.push('[');
        linked.push_str(&escape_markdown(&item.reference.text()));
        linked.push_str("](");
        linked.push_str(LINK_TARGET);
        linked.push(')');
        taken = item.end;
    }
    linked.push_str(&markdown[taken..]);
    Cow::Owned(linked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn references(text: &str) -> Vec<String> {
        split(text)
            .into_iter()
            .filter_map(|part| match part {
                Part::Reference(reference) => Some(reference.text()),
                Part::Text(_) => None,
            })
            .collect()
    }

    #[test]
    fn qualified_and_bare_ranges_are_found_in_prose() {
        assert_eq!(
            references("Normalize Sales!B2:B200001 and see 'Cash flow'!A5:B11, then B2:C3."),
            ["Sales!B2:B200001", "'Cash flow'!A5:B11", "B2:C3"]
        );
        assert_eq!(references("look at Sales!B4."), ["Sales!B4"]);
        assert_eq!(references("(see 'It''s'!$A$1)"), ["'It''s'!A1"]);
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for text in [
            "Q3 close",
            "the CO2 level in F1",
            "meet at 10:30",
            "=SUM(B2:B10)",
            "sales!b4",
            "Sales!B4B",
            "Sales!B4(x)",
            "A1:",
            "v2.B3:B4x",
            "ZZZZ1:ZZZZ2",
            "Sales!A1048577",
            "foo_B2:B3",
        ] {
            assert!(references(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn sheet_names_follow_excel_limits() {
        let long = format!("'{}'!A1", "x".repeat(32));
        assert!(references(&long).is_empty());
        assert!(references("'a/b'!A1").is_empty());
        assert!(references("'a:b'!A1").is_empty());
        assert!(references("2024!A1").is_empty());
        assert_eq!(references("Año.2024!A1"), ["'Año.2024'!A1"]);
    }

    #[test]
    fn what_the_composer_writes_is_read_back() {
        for (sheet, text) in [
            ("Ventas", "B2:B20"),
            ("My Sheet", "A1"),
            ("Bob's", "A1:C3"),
            ("A1", "B2"),
            ("2024", "B2"),
        ] {
            let range = Range::parse_a1(text).unwrap();
            let written = super::super::reference::sheet_range(sheet, range);
            let read = parse_exact(&written).unwrap();
            assert_eq!(read.sheet.as_deref(), Some(sheet));
            assert_eq!(read.range, range);
        }
    }

    #[test]
    fn split_keeps_the_text_between_references() {
        assert_eq!(
            split("in Sales!B4 now"),
            [
                Part::Text("in ".to_string()),
                Part::Reference(parse_exact("Sales!B4").unwrap()),
                Part::Text(" now".to_string()),
            ]
        );
    }

    #[test]
    fn links_skip_code_and_existing_links() {
        let source = "Use `Sales!B4` or [Sales!B5](http://x) in\n```\nSales!B6\n```\nbut Sales!B7 <Sales!B8>";
        assert_eq!(
            link_references(source),
            "Use `Sales!B4` or [Sales!B5](http://x) in\n```\nSales!B6\n```\nbut [Sales!B7](zenkai-ref:) <Sales!B8>"
        );
        assert!(matches!(link_references("nothing here"), Cow::Borrowed(_)));
    }

    #[test]
    fn link_text_escapes_markdown_characters() {
        assert_eq!(
            link_references("see Sales_2024!B4 now"),
            "see [Sales\\_2024!B4](zenkai-ref:) now"
        );
    }
}
