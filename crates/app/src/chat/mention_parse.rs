use zenkai_types::Range;

use super::mention_catalog::{Catalog, has_spreadsheet_extension};
use super::reference::{Reference, quote_sheet};

pub(super) const SELECTION: &str = "selection";
const MAX_QUOTED_CHARS: usize = 64;
const MAX_TOKEN_CHARS: usize = 300;
const MAX_QUERY_CHARS: usize = 100;
const TRAILING_PUNCTUATION: [char; 10] = ['.', ',', ';', ':', '!', '?', ')', ']', '}', '"'];
const BOUNDARY_BEFORE: [char; 8] = ['(', '[', '{', '"', ',', ';', '>', '*'];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mention {
    Selection,
    Sheet(String),
    Cells(Reference),
    Workbook(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MentionError {
    Unterminated,
    BadRange,
    UnknownName,
}

fn read_quoted(rest: &str) -> Result<(String, &str), MentionError> {
    let mut name = String::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if c != '\'' {
            name.push(c);
        } else if chars.next_if(|(_, next)| *next == '\'').is_some() {
            name.push('\'');
        } else {
            return Ok((name, &rest[index + 1..]));
        }
    }
    Err(MentionError::Unterminated)
}

struct Parts<'a> {
    name: String,
    quoted: bool,
    range: Option<&'a str>,
}

fn split(token: &str) -> Result<Parts<'_>, MentionError> {
    if let Some(rest) = token.strip_prefix('\'') {
        let (name, after) = read_quoted(rest)?;
        let range = match after {
            "" => None,
            _ => Some(after.strip_prefix('!').ok_or(MentionError::BadRange)?),
        };
        return Ok(Parts {
            name,
            quoted: true,
            range,
        });
    }
    Ok(match token.split_once('!') {
        Some((name, range)) => Parts {
            name: name.to_string(),
            quoted: false,
            range: Some(range),
        },
        None => Parts {
            name: token.to_string(),
            quoted: false,
            range: None,
        },
    })
}

// Without a catalog only the shape is checked, which is how a sent message is drawn again.
pub fn parse(token: &str, catalog: Option<&Catalog>) -> Result<Mention, MentionError> {
    let Parts {
        name,
        quoted,
        range,
    } = split(token)?;
    if name.is_empty() {
        return Err(MentionError::UnknownName);
    }
    if let Some(range) = range {
        let range = Range::parse_a1(range).ok_or(MentionError::BadRange)?;
        let sheet = match catalog {
            Some(catalog) => catalog.sheet(&name).ok_or(MentionError::UnknownName)?,
            None => name,
        };
        return Ok(Mention::Cells(Reference {
            sheet: Some(sheet),
            range,
        }));
    }
    if !quoted && name.eq_ignore_ascii_case(SELECTION) {
        return Ok(Mention::Selection);
    }
    match catalog {
        Some(catalog) => catalog
            .sheet(&name)
            .map(Mention::Sheet)
            .or_else(|| catalog.workbook(&name).map(Mention::Workbook))
            .ok_or(MentionError::UnknownName),
        None if quoted => Ok(Mention::Sheet(name)),
        None if has_spreadsheet_extension(&name) => Ok(Mention::Workbook(name)),
        None => Err(MentionError::UnknownName),
    }
}

pub(super) fn boundary_before(previous: Option<char>) -> bool {
    previous.is_none_or(|c| c.is_whitespace() || BOUNDARY_BEFORE.contains(&c))
}

fn quoted_end(rest: &str) -> Option<usize> {
    let mut chars = rest.char_indices().skip(1).peekable();
    let mut count = 0;
    while let Some((index, c)) = chars.next() {
        count += 1;
        if c == '\n' || count > MAX_QUOTED_CHARS {
            return None;
        }
        if c == '\'' && chars.next_if(|(_, next)| *next == '\'').is_none() {
            return Some(index + 1);
        }
    }
    None
}

fn token_end(rest: &str) -> usize {
    if rest.starts_with('\'') {
        let Some(closed) = quoted_end(rest) else {
            return 0;
        };
        let range_len = rest[closed..].strip_prefix('!').map_or(0, |after| {
            1 + after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '$' || *c == ':')
                .map(char::len_utf8)
                .sum::<usize>()
        });
        return closed + range_len;
    }
    let raw: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace())
        .take(MAX_TOKEN_CHARS)
        .collect();
    raw.trim_end_matches(TRAILING_PUNCTUATION).len()
}

pub(super) struct Token<'a> {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) body: &'a str,
}

pub(super) fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut found = Vec::new();
    let mut previous = None;
    let mut resume = 0;
    for (index, c) in text.char_indices() {
        if index >= resume && c == '@' && boundary_before(previous) {
            let body_start = index + 1;
            let end = body_start + token_end(&text[body_start..]);
            if end > body_start {
                found.push(Token {
                    start: index,
                    end,
                    body: &text[body_start..end],
                });
                resume = end;
            }
        }
        previous = Some(c);
    }
    found
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub start: usize,
    pub end: usize,
    pub mention: Mention,
}

impl Chip {
    pub fn reference(&self) -> Option<&Reference> {
        match &self.mention {
            Mention::Cells(reference) => Some(reference),
            _ => None,
        }
    }
}

// The mentions of a sent message that are drawn as chips, found by shape alone.
pub fn chips(text: &str) -> Vec<Chip> {
    tokens(text)
        .into_iter()
        .filter_map(|token| {
            let mention = parse(token.body, None).ok()?;
            Some(Chip {
                start: token.start,
                end: token.end,
                mention,
            })
        })
        .collect()
}

pub(super) fn quote_name(name: &str) -> String {
    format!("'{}'", name.replace('\'', "''"))
}

pub(super) fn workbook_text(name: &str) -> String {
    let plain = has_spreadsheet_extension(name)
        && !name.contains(|c: char| c.is_whitespace() || c == '\'' || c == '!');
    if plain {
        name.to_string()
    } else {
        quote_name(name)
    }
}

pub(super) fn sheet_text(name: &str) -> String {
    if name.eq_ignore_ascii_case(SELECTION) {
        quote_name(name)
    } else {
        quote_sheet(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveQuery {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

fn is_open_segment(segment: &str) -> bool {
    !segment.contains(char::is_whitespace)
        || (segment.starts_with('\'') && segment.matches('\'').count() % 2 == 1)
}

// The `@` word that ends at the caret, if the caret is still inside it.
pub fn active_query(text: &str, cursor: usize) -> Option<ActiveQuery> {
    let before = text.get(..cursor)?;
    let mut previous_chars = before.char_indices().rev().take(MAX_QUERY_CHARS).peekable();
    while let Some((index, c)) = previous_chars.next() {
        if c == '\n' {
            return None;
        }
        let preceding = previous_chars.peek().map(|(_, c)| *c);
        if c == '@' && boundary_before(preceding) {
            let segment = &before[index + 1..];
            if is_open_segment(segment) {
                return Some(ActiveQuery {
                    start: index,
                    end: cursor,
                    text: segment.to_string(),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::mention_catalog::sample::*;
    use super::*;

    #[test]
    fn a_sheet_and_range_parse_and_take_the_sheets_own_spelling() {
        let found = parse("sales!a1:c10", Some(&catalog()));
        assert_eq!(found, Ok(cells("Sales", "A1:C10")));
    }

    #[test]
    fn selection_and_names_resolve_in_order() {
        let catalog = catalog();
        assert_eq!(parse("selection", Some(&catalog)), Ok(Mention::Selection));
        assert_eq!(parse("SELECTION", Some(&catalog)), Ok(Mention::Selection));
        assert_eq!(
            parse("'selection'", Some(&catalog)),
            Ok(Mention::Sheet("selection".to_string()))
        );
        assert_eq!(
            parse("Sales", Some(&catalog)),
            Ok(Mention::Sheet("Sales".to_string()))
        );
        assert_eq!(
            parse("sales-q3.xlsx", Some(&catalog)),
            Ok(Mention::Workbook("sales-q3.xlsx".to_string()))
        );
        assert_eq!(
            parse("sales-archive.xlsx", Some(&catalog)),
            Ok(Mention::Workbook("sales-archive.xlsx".to_string()))
        );
    }

    #[test]
    fn unknown_names_and_bad_ranges_are_errors() {
        let catalog = catalog();
        assert_eq!(
            parse("Nowhere!A1", Some(&catalog)),
            Err(MentionError::UnknownName)
        );
        assert_eq!(
            parse("Nowhere", Some(&catalog)),
            Err(MentionError::UnknownName)
        );
        assert_eq!(
            parse("Sales!ZZZZ1", Some(&catalog)),
            Err(MentionError::BadRange)
        );
        assert_eq!(parse("Sales!", Some(&catalog)), Err(MentionError::BadRange));
        assert_eq!(
            parse("'Sales", Some(&catalog)),
            Err(MentionError::Unterminated)
        );
        assert_eq!(
            parse("'Sales'x", Some(&catalog)),
            Err(MentionError::BadRange)
        );
        assert_eq!(parse("", Some(&catalog)), Err(MentionError::UnknownName));
        assert_eq!(parse("!A1", Some(&catalog)), Err(MentionError::UnknownName));
    }

    #[test]
    fn without_a_catalog_only_the_shape_counts() {
        assert_eq!(parse("Anywhere!B2", None), Ok(cells("Anywhere", "B2")));
        assert_eq!(
            parse("budget.xlsx", None),
            Ok(Mention::Workbook("budget.xlsx".to_string()))
        );
        assert_eq!(parse("Sales", None), Err(MentionError::UnknownName));
        assert_eq!(parse("notes.txt", None), Err(MentionError::UnknownName));
    }

    #[test]
    fn chips_are_found_by_shape_and_emails_are_not() {
        let text = "Compare @Sales!A1:C40 with @budget-2026.xlsx, mail bob@example.com or @bob";
        let found = chips(text);
        let words: Vec<&str> = found
            .iter()
            .map(|chip| &text[chip.start..chip.end])
            .collect();
        assert_eq!(words, ["@Sales!A1:C40", "@budget-2026.xlsx"]);
        assert!(found[0].reference().is_some());
        assert!(found[1].reference().is_none());
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_a_mention() {
        let text = "(see @Sales!A1:B2). Then @'My sheet'!C3, done @data.csv?";
        let words: Vec<&str> = chips(text)
            .iter()
            .map(|chip| &text[chip.start..chip.end])
            .collect();
        assert_eq!(words, ["@Sales!A1:B2", "@'My sheet'!C3", "@data.csv"]);
    }

    #[test]
    fn the_query_is_the_at_word_ending_at_the_caret() {
        let found = active_query("Check @Sa", 9).unwrap();
        assert_eq!((found.start, found.end, found.text.as_str()), (6, 9, "Sa"));
        assert_eq!(active_query("@", 1).unwrap().text, "");
        assert_eq!(active_query("Check @Sa ", 10), None);
        assert_eq!(active_query("mail bob@exa", 12), None);
        assert_eq!(active_query("Check @Sa\nnext", 14), None);
        assert_eq!(active_query("no mention", 10), None);
    }

    #[test]
    fn an_open_quote_keeps_the_query_going_across_spaces() {
        let found = active_query("Look at @'Sales su", 18).unwrap();
        assert_eq!(found.text, "'Sales su");
        assert_eq!(active_query("Look at @'Sales' su", 19), None);
    }

    #[test]
    fn the_query_ignores_a_caret_inside_a_character() {
        assert_eq!(active_query("é@a", 1), None);
        assert_eq!(active_query("é @a", 5).unwrap().text, "a");
    }
}
