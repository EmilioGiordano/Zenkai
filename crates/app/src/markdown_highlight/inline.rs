use std::collections::{HashMap, HashSet};

use super::{Spans, Token};

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

// What earlier searches on the same line already showed, so a closer that is missing is looked
// for once and a closer that is far away is found once: scanning stays linear in the line.
#[derive(Default)]
struct Searched {
    ticks_missing: HashSet<usize>,
    closers: HashMap<(u8, usize), Option<usize>>,
    bracket: Option<Option<usize>>,
    paren_missing_from: Option<usize>,
}

impl Searched {
    fn closer(&mut self, line: &str, from: usize, delimiter: u8, length: usize) -> Option<usize> {
        match self.closers.get(&(delimiter, length)) {
            Some(None) => return None,
            Some(Some(close)) if *close > from => return Some(*close),
            _ => {}
        }
        let found = closing_run(line, from, delimiter, length);
        self.closers.insert((delimiter, length), found);
        found
    }

    fn bracket_end(&mut self, line: &str, from: usize) -> Option<usize> {
        match self.bracket {
            Some(None) => return None,
            Some(Some(end)) if end >= from => return Some(end),
            _ => {}
        }
        let found = line[from..].find(']').map(|found| from + found);
        self.bracket = Some(found);
        found
    }

    fn paren_end(&mut self, line: &str, from: usize) -> Option<usize> {
        if self
            .paren_missing_from
            .is_some_and(|missing| from >= missing)
        {
            return None;
        }
        let found = line[from..].find(')').map(|found| from + found);
        if found.is_none() {
            self.paren_missing_from = Some(from);
        }
        found
    }
}

pub(super) fn scan_inline(line: &str, start: usize, spans: &mut Spans) {
    let bytes = line.as_bytes();
    let mut searched = Searched::default();
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
                while !searched.ticks_missing.contains(&length)
                    && let Some(found) = line[search..].find('`')
                {
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
                if end.is_none() {
                    searched.ticks_missing.insert(length);
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
                    .then(|| searched.closer(line, at + length, delimiter, length))
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
                let text_end = searched.bracket_end(line, at + 1);
                let target = text_end
                    .filter(|end| bytes.get(end + 1) == Some(&b'('))
                    .and_then(|end| {
                        searched
                            .paren_end(line, end + 2)
                            .map(|found| (end, found + 1))
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

#[cfg(test)]
mod tests {
    use crate::markdown_highlight::blocks::scan_line;
    use crate::markdown_highlight::{MAX_HIGHLIGHTED_BYTES, Token, kinds};

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
    fn a_huge_line_of_unclosed_openers_is_scanned_in_bounded_time() {
        let started = std::time::Instant::now();
        for opener in ["[", "**x", "`a", "[a](", "_a ", "*a ", "``a `b "] {
            let line = opener.repeat(4 * 1024 * 1024 / opener.len());
            let head: String = line.chars().take(MAX_HIGHLIGHTED_BYTES).collect();
            scan_line(&head, None);
            // Without the cap, the memo alone keeps even the whole line linear.
            scan_line(&line, None);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }
}
