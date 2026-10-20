use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope, RopeExt,
};
use gpui_kit::*;

pub const LANGUAGE: &str = "markdown";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Heading,
    HeadingMarker,
    ListMarker,
    QuoteMarker,
    Rule,
    Fence,
    Code,
    CodeSpan,
    Strong,
    Emphasis,
    LinkText,
    LinkUri,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fence {
    marker: u8,
    length: usize,
}

type Spans = Vec<(Range<usize>, Token)>;

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

fn closing_run(line: &str, from: usize, delimiter: u8, length: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut at = from;
    while at + length <= bytes.len() {
        let closes = bytes[at..at + length].iter().all(|byte| *byte == delimiter)
            && bytes.get(at + length) != Some(&delimiter)
            && at > from
            && !bytes[at - 1].is_ascii_whitespace();
        if closes {
            return Some(at);
        }
        at += 1;
    }
    None
}

fn word_byte(byte: Option<&u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric())
}

fn scan_inline(line: &str, start: usize, spans: &mut Spans) {
    let bytes = line.as_bytes();
    let mut at = start;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => {
                at += 1 + line[at + 1..].chars().next().map_or(0, char::len_utf8);
            }
            b'`' => {
                let length = bytes[at..].iter().take_while(|byte| **byte == b'`').count();
                let mut search = at + length;
                let mut end = None;
                while let Some(found) = line[search..].find('`') {
                    let open = search + found;
                    let run = bytes[open..]
                        .iter()
                        .take_while(|byte| **byte == b'`')
                        .count();
                    if run == length {
                        end = Some(open + run);
                        break;
                    }
                    search = open + run;
                }
                match end {
                    Some(end) => {
                        spans.push((at..end, Token::CodeSpan));
                        at = end;
                    }
                    None => at += length,
                }
            }
            delimiter @ (b'*' | b'_') => {
                let run = bytes[at..]
                    .iter()
                    .take_while(|byte| **byte == delimiter)
                    .count();
                let length = if run >= 2 { 2 } else { 1 };
                let opens = bytes
                    .get(at + length)
                    .is_some_and(|next| !next.is_ascii_whitespace() && *next != delimiter);
                let intraword = delimiter == b'_' && at > 0 && word_byte(bytes.get(at - 1));
                let close = (opens && !intraword)
                    .then(|| closing_run(line, at + length, delimiter, length))
                    .flatten()
                    .filter(|close| delimiter != b'_' || !word_byte(bytes.get(close + length)));
                match close {
                    Some(close) => {
                        let token = if length == 2 {
                            Token::Strong
                        } else {
                            Token::Emphasis
                        };
                        spans.push((at..close + length, token));
                        at = close + length;
                    }
                    None => at += run,
                }
            }
            b'[' => {
                let text_end = line[at + 1..].find(']').map(|found| at + 1 + found);
                let target = text_end
                    .filter(|end| bytes.get(end + 1) == Some(&b'('))
                    .and_then(|end| {
                        line[end + 2..]
                            .find(')')
                            .map(|found| (end, end + 2 + found + 1))
                    });
                match target {
                    Some((text_end, uri_end)) => {
                        spans.push((at..text_end + 1, Token::LinkText));
                        spans.push((text_end + 1..uri_end, Token::LinkUri));
                        at = uri_end;
                    }
                    None => at += 1,
                }
            }
            _ => at += line[at..].chars().next().map_or(1, char::len_utf8),
        }
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

pub struct MarkdownHighlighter {
    text: Rope,
    // The fence open as each line starts; always holds at least line 0, and is cut back to
    // the edited line on a change, then extended only as far as the lines asked for.
    fences: RefCell<Vec<Option<Fence>>>,
}

impl MarkdownHighlighter {
    pub fn factory() -> InputHighlighterFactory {
        Rc::new(|language| {
            (language == LANGUAGE)
                .then(|| Box::new(MarkdownHighlighter::new()) as Box<dyn InputHighlighter>)
        })
    }

    fn new() -> MarkdownHighlighter {
        MarkdownHighlighter {
            text: Rope::new(),
            fences: RefCell::new(vec![None]),
        }
    }

    fn fence_at(&self, row: usize) -> Option<Fence> {
        let mut fences = self.fences.borrow_mut();
        while fences.len() <= row {
            let previous = fences.len() - 1;
            let mut state = fences[previous];
            let slice = self.text.slice_line(previous);
            if slice
                .chars()
                .find(|c| *c != ' ')
                .is_some_and(|c| matches!(c, '`' | '~'))
            {
                state = next_fence(&slice.to_string(), state);
            }
            fences.push(state);
        }
        fences[row]
    }
}

fn style_of(token: Token, resolver: &dyn HighlightStyleResolver) -> HighlightStyle {
    let named = |name: &str| resolver.style(name).unwrap_or_default();
    match token {
        Token::Heading => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..named("title")
        },
        Token::HeadingMarker | Token::QuoteMarker => named("punctuation.special"),
        Token::ListMarker | Token::Rule => named("punctuation.list_marker"),
        Token::Fence => named("punctuation.delimiter"),
        Token::Code => named("text.literal"),
        Token::CodeSpan => named("text.code.span"),
        Token::Strong => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..named("emphasis.strong")
        },
        Token::Emphasis => HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..named("emphasis")
        },
        Token::LinkText => named("link_text"),
        Token::LinkUri => named("link_uri"),
    }
}

impl InputHighlighter for MarkdownHighlighter {
    fn language(&self) -> SharedString {
        LANGUAGE.into()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.text = text.clone();
        let keep = edit.map_or(1, |edit| edit.start_position.row + 1);
        self.fences.get_mut().truncate(keep.max(1));
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let end = range.end.min(self.text.len());
        let start = range.start.min(end);
        let first = self.text.offset_to_point(start).row;
        let last = self.text.offset_to_point(end).row;
        let mut runs = Vec::new();
        let mut position = start;
        for row in first..=last {
            let line_start = self.text.line_start_offset(row);
            let line = self.text.slice_line(row).to_string();
            let open = self.fence_at(row);
            for (span, token) in scan_line(&line, open) {
                let from = (line_start + span.start).max(position).min(end);
                let to = (line_start + span.end).min(end);
                if from >= to {
                    continue;
                }
                if from > position {
                    runs.push((position..from, HighlightStyle::default()));
                }
                runs.push((from..to, style_of(token, resolver)));
                position = to;
            }
        }
        if position < end {
            runs.push((position..end, HighlightStyle::default()));
        }
        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Fence, Token, next_fence, scan_line};

    fn kinds(line: &str) -> Vec<(&str, Token)> {
        scan_line(line, None)
            .into_iter()
            .map(|(span, token)| (&line[span], token))
            .collect()
    }

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
    fn inline_marks_cover_their_delimiters() {
        assert_eq!(kinds("a **bold** b")[0], ("**bold**", Token::Strong));
        assert_eq!(kinds("a *it* b")[0], ("*it*", Token::Emphasis));
        assert_eq!(kinds("a `code` b")[0], ("`code`", Token::CodeSpan));
        assert_eq!(
            kinds("see [docs](http://x.y) now"),
            [
                ("[docs]", Token::LinkText),
                ("(http://x.y)", Token::LinkUri)
            ]
        );
    }

    #[test]
    fn unfinished_marks_and_snake_case_stay_plain() {
        assert!(kinds("2 * 3 = 6").is_empty());
        assert!(kinds("a **b").is_empty());
        assert!(kinds("snake_case_name").is_empty());
        assert!(kinds("`open").is_empty());
        assert!(kinds(r"\*not emphasis\*").is_empty());
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
