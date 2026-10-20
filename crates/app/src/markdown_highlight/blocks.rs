use super::inline::scan_inline;
use super::{Spans, Token};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fence {
    marker: u8,
    length: usize,
}

fn indent_of(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn fence_run(line: &str) -> Option<Fence> {
    let indent = indent_of(line);
    if indent > 3 {
        return None;
    }
    let rest = &line.as_bytes()[indent..];
    let marker = *rest.first().filter(|byte| matches!(**byte, b'`' | b'~'))?;
    let length = rest.iter().take_while(|byte| **byte == marker).count();
    (length >= 3).then_some(Fence { marker, length })
}

// Whether a line is a fence that opens (with its info string) or closes the given block.
pub fn next_fence(line: &str, current: Option<Fence>) -> Option<Fence> {
    let run = fence_run(line);
    match (current, run) {
        (None, Some(run)) => {
            let info = &line[indent_of(line) + run.length..];
            (run.marker != b'`' || !info.contains('`')).then_some(run)
        }
        (Some(open), Some(run))
            if run.marker == open.marker
                && run.length >= open.length
                && line[indent_of(line) + run.length..].trim().is_empty() =>
        {
            None
        }
        (current, _) => current,
    }
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_')
        && trimmed.chars().filter(|c| *c == first).count() >= 3
        && trimmed.chars().all(|c| c == first || c == ' ')
}

fn list_marker_len(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    match bytes.first()? {
        b'-' | b'*' | b'+' if bytes.get(1).is_none_or(|next| *next == b' ') => Some(1),
        b'0'..=b'9' => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            let after = bytes.get(digits)?;
            (digits <= 9
                && matches!(after, b'.' | b')')
                && bytes.get(digits + 1).is_none_or(|next| *next == b' '))
            .then_some(digits + 1)
        }
        _ => None,
    }
}

// One line of a document, given whether a fenced block is open as the line starts.
pub fn scan_line(line: &str, open: Option<Fence>) -> Spans {
    let mut spans = Spans::new();
    let whole = 0..line.len();
    if line.is_empty() {
        return spans;
    }
    if open.is_some() {
        let closes = next_fence(line, open).is_none();
        spans.push((whole, if closes { Token::Fence } else { Token::Code }));
        return spans;
    }
    if next_fence(line, None).is_some() {
        spans.push((whole, Token::Fence));
        return spans;
    }
    let indent = indent_of(line);
    let rest = &line[indent..];
    if indent <= 3 && rest.starts_with('#') {
        let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
        if hashes <= 6 && rest.as_bytes().get(hashes).is_none_or(|next| *next == b' ') {
            spans.push((indent..indent + hashes, Token::HeadingMarker));
            if indent + hashes < line.len() {
                spans.push((indent + hashes..line.len(), Token::Heading));
            }
            return spans;
        }
    }
    if indent <= 3 && is_rule(line) {
        spans.push((whole, Token::Rule));
        return spans;
    }
    let mut start = indent;
    while line[start..].starts_with('>') {
        spans.push((start..start + 1, Token::QuoteMarker));
        start += 1;
        start += line[start..]
            .bytes()
            .take_while(|byte| *byte == b' ')
            .count();
    }
    if let Some(length) = list_marker_len(&line[start..]) {
        spans.push((start..start + length, Token::ListMarker));
        start += length;
    }
    scan_inline(line, start, &mut spans);
    spans
}

#[cfg(test)]
mod tests {
    use super::{Fence, next_fence, scan_line};
    use crate::markdown_highlight::{Token, kinds};

    #[test]
    fn a_heading_has_a_marker_and_a_title() {
        assert_eq!(
            kinds("## Plan"),
            [("##", Token::HeadingMarker), (" Plan", Token::Heading)]
        );
        assert!(kinds("#hashtag").iter().all(|(_, t)| *t != Token::Heading));
        assert!(
            kinds("####### seven")
                .iter()
                .all(|(_, t)| *t != Token::Heading)
        );
    }

    #[test]
    fn list_and_quote_markers_are_told_from_their_text() {
        assert_eq!(kinds("- item")[0], ("-", Token::ListMarker));
        assert_eq!(kinds("12. item")[0], ("12.", Token::ListMarker));
        assert_eq!(kinds("> quoted")[0], (">", Token::QuoteMarker));
        assert_eq!(kinds("> - nested")[1], ("-", Token::ListMarker));
        assert!(kinds("-item").is_empty());
    }

    #[test]
    fn a_fence_colors_its_lines_until_it_closes() {
        let open = next_fence("```rust", None);
        assert!(open.is_some());
        assert_eq!(scan_line("let x = 1;", open), [(0..10, Token::Code)]);
        assert_eq!(scan_line("```", open), [(0..3, Token::Fence)]);
        assert_eq!(next_fence("```", open), None);
        assert_eq!(next_fence("let x", open), open);
        assert_eq!(scan_line("```rust", None), [(0..7, Token::Fence)]);
    }

    #[test]
    fn a_shorter_or_different_fence_does_not_close_a_block() {
        let open = next_fence("````", None);
        assert_eq!(next_fence("```", open), open);
        assert_eq!(next_fence("~~~~", open), open);
        assert_eq!(next_fence("````", open), None);
        assert_eq!(
            next_fence("``` not a close", next_fence("```", None)),
            Some(Fence {
                marker: b'`',
                length: 3
            })
        );
    }

    #[test]
    fn a_rule_is_not_a_list() {
        assert_eq!(kinds("---"), [("---", Token::Rule)]);
        assert_eq!(kinds("* * *"), [("* * *", Token::Rule)]);
    }

    #[test]
    fn multibyte_text_never_splits_a_character() {
        let line = "# Título **negrita ñ** y `código` [enlace é](http://é)";
        for (span, _) in scan_line(line, None) {
            assert!(line.is_char_boundary(span.start) && line.is_char_boundary(span.end));
        }
    }

    #[test]
    fn spans_never_overlap_and_stay_in_the_line() {
        for line in [
            "> - **a** *b* `c` [d](e)",
            "***both***",
            "__x__ and _y_",
            "[a](b) [c](d",
            "``double `tick` code``",
            "",
        ] {
            let mut end = 0;
            for (span, _) in scan_line(line, None) {
                assert!(span.start >= end && span.end <= line.len(), "{line}");
                end = span.end;
            }
        }
    }
}
