use zenkai_types::{Range, WorkbookId};

use super::plain_name;
use super::reference::{Reference, quote_sheet};

const SPREADSHEET_EXTENSIONS: [&str; 7] = ["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"];
const SELECTION: &str = "selection";
const MAX_QUOTED_CHARS: usize = 64;
const MAX_TOKEN_CHARS: usize = 300;
const MAX_QUERY_CHARS: usize = 100;
const TRAILING_PUNCTUATION: [char; 10] = ['.', ',', ';', ':', '!', '?', ')', ']', '}', '"'];
const BOUNDARY_BEFORE: [char; 8] = ['(', '[', '{', '"', ',', ';', '>', '*'];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Names,
    UsedRanges,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetEntry {
    pub name: String,
    pub used: Option<Range>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenBook {
    pub id: WorkbookId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBook {
    pub book: OpenBook,
    pub sheets: Vec<SheetEntry>,
    pub selection: Option<Reference>,
}

// Everything a mention can name: the same reach the agent has through its tools.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub active: Option<ActiveBook>,
    pub open: Vec<OpenBook>,
    pub folder: Vec<String>,
}

impl Catalog {
    fn sheet(&self, name: &str) -> Option<String> {
        let wanted = name.to_lowercase();
        self.active
            .as_ref()?
            .sheets
            .iter()
            .find(|sheet| sheet.name.to_lowercase() == wanted)
            .map(|sheet| sheet.name.clone())
    }

    fn open_book(&self, name: &str) -> Option<&OpenBook> {
        self.open
            .iter()
            .find(|book| book.name.eq_ignore_ascii_case(name))
    }

    fn workbook(&self, name: &str) -> Option<String> {
        self.open_book(name)
            .map(|book| book.name.clone())
            .or_else(|| {
                self.folder
                    .iter()
                    .find(|file| file.eq_ignore_ascii_case(name))
                    .cloned()
            })
    }

    fn folder_only(&self) -> impl Iterator<Item = &String> {
        self.folder.iter().filter(|file| {
            let file_name = file.rsplit('/').next().unwrap_or(file);
            self.open_book(file_name).is_none()
        })
    }
}

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

fn has_spreadsheet_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && SPREADSHEET_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
    })
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

fn boundary_before(previous: Option<char>) -> bool {
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

struct Token<'a> {
    start: usize,
    end: usize,
    body: &'a str,
}

fn tokens(text: &str) -> Vec<Token<'_>> {
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
    pub reference: Option<Reference>,
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
                reference: match mention {
                    Mention::Cells(reference) => Some(reference),
                    _ => None,
                },
            })
        })
        .collect()
}

fn quote_name(name: &str) -> String {
    format!("'{}'", name.replace('\'', "''"))
}

fn workbook_text(name: &str) -> String {
    let plain = has_spreadsheet_extension(name)
        && !name.contains(|c: char| c.is_whitespace() || c == '\'' || c == '!');
    if plain {
        name.to_string()
    } else {
        quote_name(name)
    }
}

fn sheet_text(name: &str) -> String {
    if name.eq_ignore_ascii_case(SELECTION) {
        quote_name(name)
    } else {
        quote_sheet(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub text: String,
    pub notes: Vec<String>,
}

fn where_is(book: &ActiveBook) -> String {
    format!(
        "workbook \"{}\" (id {})",
        plain_name(&book.book.name),
        book.book.id.0
    )
}

fn cells_note(reference: &Reference, book: &ActiveBook) -> String {
    format!(
        "range {} of sheet \"{}\" in {}",
        reference.range,
        plain_name(reference.sheet.as_deref().unwrap_or_default()),
        where_is(book)
    )
}

fn describe(mention: &Mention, catalog: &Catalog) -> Option<(String, String)> {
    let active = catalog.active.as_ref();
    match mention {
        Mention::Selection => {
            let book = active?;
            let reference = book.selection.as_ref()?;
            Some((reference.text(), cells_note(reference, book)))
        }
        Mention::Cells(reference) => Some((reference.text(), cells_note(reference, active?))),
        // Always quoted: a bare word cannot be told from other text when the message is drawn again.
        Mention::Sheet(name) => Some((
            quote_name(name),
            format!("sheet \"{}\" of {}", plain_name(name), where_is(active?)),
        )),
        Mention::Workbook(name) => {
            let note = match catalog.open_book(name) {
                Some(book) => format!(
                    "the open workbook \"{}\" (id {})",
                    plain_name(&book.name),
                    book.id.0
                ),
                None => format!(
                    "the file \"{}\" in the working folder, not open; open_workbook takes that path",
                    plain_name(name)
                ),
            };
            Some((workbook_text(name), note))
        }
    }
}

// Normalises the mentions of a message and explains each one for the agent. A mention that
// names nothing known stays as the user typed it.
pub fn prepare(text: &str, catalog: &Catalog) -> Prepared {
    let mut out = String::with_capacity(text.len());
    let mut notes: Vec<String> = Vec::new();
    let mut taken = 0;
    for token in tokens(text) {
        let Ok(mention) = parse(token.body, Some(catalog)) else {
            continue;
        };
        let Some((shown, note)) = describe(&mention, catalog) else {
            continue;
        };
        out.push_str(&text[taken..token.start]);
        out.push('@');
        out.push_str(&shown);
        taken = token.end;
        let note = format!("@{shown} is {note}");
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    out.push_str(&text[taken..]);
    Prepared { text: out, notes }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Selection,
    Sheets,
    Workbooks,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    Address(String),
    UsedRange(String),
    EmptySheet,
    Open,
    WorkingFolder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub group: Group,
    pub label: String,
    pub hint: Hint,
    pub insert: String,
}

pub fn candidates(query: &str, catalog: &Catalog) -> Vec<Candidate> {
    let wanted = query.trim_matches('\'').to_lowercase();
    let matches = |name: &str| name.to_lowercase().contains(&wanted);
    let mut found = Vec::new();
    if let Some(book) = &catalog.active {
        if let Some(selection) = book.selection.as_ref().filter(|_| matches(SELECTION)) {
            found.push(Candidate {
                group: Group::Selection,
                label: format!("@{SELECTION}"),
                hint: Hint::Address(selection.text()),
                insert: format!("@{SELECTION} "),
            });
        }
        for sheet in book.sheets.iter().filter(|sheet| matches(&sheet.name)) {
            found.push(Candidate {
                group: Group::Sheets,
                label: format!("@{}", sheet.name),
                hint: sheet
                    .used
                    .map_or(Hint::EmptySheet, |range| Hint::UsedRange(range.to_string())),
                insert: format!("@{} ", sheet_text(&sheet.name)),
            });
        }
    }
    let open = catalog.open.iter().filter(|book| matches(&book.name));
    for book in open {
        found.push(Candidate {
            group: Group::Workbooks,
            label: format!("@{}", book.name),
            hint: Hint::Open,
            insert: format!("@{} ", workbook_text(&book.name)),
        });
    }
    for file in catalog.folder_only().filter(|file| matches(file)) {
        found.push(Candidate {
            group: Group::Workbooks,
            label: format!("@{file}"),
            hint: Hint::WorkingFolder,
            insert: format!("@{} ", workbook_text(file)),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    fn sheet(name: &str, used: Option<&str>) -> SheetEntry {
        SheetEntry {
            name: name.to_string(),
            used: used.map(range),
        }
    }

    fn catalog() -> Catalog {
        let book = OpenBook {
            id: WorkbookId(3),
            name: "sales-q3.xlsx".to_string(),
        };
        Catalog {
            active: Some(ActiveBook {
                book: book.clone(),
                sheets: vec![
                    sheet("Sales", Some("A1:H240")),
                    sheet("Sales summary", Some("A1:D18")),
                    sheet("selection", None),
                ],
                selection: Some(Reference {
                    sheet: Some("Sales".to_string()),
                    range: range("B4:D12"),
                }),
            }),
            open: vec![book],
            folder: vec![
                "sales-archive.xlsx".to_string(),
                "reports/budget 2026.xlsx".to_string(),
                "sales-q3.xlsx".to_string(),
            ],
        }
    }

    fn cells(sheet: &str, text: &str) -> Mention {
        Mention::Cells(Reference {
            sheet: Some(sheet.to_string()),
            range: range(text),
        })
    }

    #[test]
    fn a_sheet_and_range_parse_and_take_the_sheets_own_spelling() {
        let found = parse("sales!a1:c10", Some(&catalog()));
        assert_eq!(found, Ok(cells("Sales", "A1:C10")));
    }

    #[test]
    fn a_quoted_sheet_with_spaces_round_trips() {
        let found = parse("'Sales summary'!A1:B4", Some(&catalog()));
        assert_eq!(found, Ok(cells("Sales summary", "A1:B4")));
        let prepared = prepare("see @'Sales summary'!A1:B4 now", &catalog());
        assert_eq!(prepared.text, "see @'Sales summary'!A1:B4 now");
    }

    #[test]
    fn a_doubled_quote_in_a_name_is_one_quote() {
        let mut catalog = catalog();
        if let Some(book) = catalog.active.as_mut() {
            book.sheets.push(sheet("Bob's", None));
        }
        assert_eq!(
            parse("'Bob''s'!A1", Some(&catalog)),
            Ok(cells("Bob's", "A1"))
        );
        assert_eq!(prepare("@'Bob''s'!A1", &catalog).text, "@'Bob''s'!A1");
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
        assert!(found[0].reference.is_some());
        assert!(found[1].reference.is_none());
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
    fn prepare_resolves_selection_and_explains_each_mention_once() {
        let prepared = prepare(
            "Check @selection against @selection and @Sales summary",
            &catalog(),
        );
        assert_eq!(
            prepared.text,
            "Check @Sales!B4:D12 against @Sales!B4:D12 and @'Sales' summary"
        );
        assert_eq!(prepared.notes.len(), 2);
        assert_eq!(
            prepared.notes[0],
            "@Sales!B4:D12 is range B4:D12 of sheet \"Sales\" in workbook \"sales-q3.xlsx\" (id 3)"
        );
    }

    #[test]
    fn prepare_tells_open_workbooks_from_folder_files() {
        let prepared = prepare("@sales-q3.xlsx and @sales-archive.xlsx", &catalog());
        assert_eq!(prepared.text, "@sales-q3.xlsx and @sales-archive.xlsx");
        assert!(prepared.notes[0].contains("open workbook \"sales-q3.xlsx\" (id 3)"));
        assert!(prepared.notes[1].contains("working folder"));
    }

    #[test]
    fn prepare_leaves_unknown_mentions_and_plain_text_alone() {
        let text = "Ask @Nowhere!A1 and @bob, write to bob@example.com, or @'oops";
        let prepared = prepare(text, &catalog());
        assert_eq!(prepared.text, text);
        assert!(prepared.notes.is_empty());
    }

    #[test]
    fn prepare_without_a_workbook_leaves_the_selection_alone() {
        let prepared = prepare("@selection", &Catalog::default());
        assert_eq!(prepared.text, "@selection");
    }

    #[test]
    fn names_from_files_cannot_break_out_of_the_note() {
        let mut catalog = catalog();
        if let Some(book) = catalog.active.as_mut() {
            book.book.name = "x\" (id 9) \n.xlsx".to_string();
        }
        let prepared = prepare("@selection", &catalog);
        assert!(!prepared.notes[0].contains('\n'));
        assert_eq!(prepared.notes[0].matches('"').count(), 4);
    }

    #[test]
    fn garbage_never_panics() {
        let catalog = catalog();
        let long = format!("@'{}", "a".repeat(10_000));
        let long_plain = format!("@{}", "b".repeat(10_000));
        for text in [
            "@",
            "@@",
            "@'",
            "@''",
            "@'''",
            "@!",
            "@'!",
            "é@é",
            "@é!A1",
            "x @'a''",
            long.as_str(),
            long_plain.as_str(),
        ] {
            let _ = prepare(text, &catalog);
            let _ = chips(text);
            for cursor in 0..=text.len() {
                let _ = active_query(text, cursor);
            }
        }
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

    #[test]
    fn candidates_group_selection_sheets_and_workbooks() {
        let found = candidates("", &catalog());
        let labels: Vec<&str> = found.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "@selection",
                "@Sales",
                "@Sales summary",
                "@selection",
                "@sales-q3.xlsx",
                "@sales-archive.xlsx",
                "@reports/budget 2026.xlsx",
            ]
        );
        assert_eq!(found[0].hint, Hint::Address("Sales!B4:D12".to_string()));
        assert_eq!(found[1].hint, Hint::UsedRange("A1:H240".to_string()));
        assert_eq!(found[3].hint, Hint::EmptySheet);
        assert_eq!(found[3].insert, "@'selection' ");
        assert_eq!(found[4].hint, Hint::Open);
        assert_eq!(found[5].hint, Hint::WorkingFolder);
        assert_eq!(found[2].insert, "@'Sales summary' ");
        assert_eq!(found[6].insert, "@'reports/budget 2026.xlsx' ");
    }

    #[test]
    fn typing_narrows_the_candidates_and_ignores_quotes_and_case() {
        let labels = |query: &str| -> Vec<String> {
            candidates(query, &catalog())
                .into_iter()
                .map(|c| c.label)
                .collect()
        };
        assert_eq!(labels("ARCH"), ["@sales-archive.xlsx"]);
        assert_eq!(labels("'Sales su"), ["@Sales summary"]);
        assert!(labels("Sales!A1").is_empty());
        assert!(labels("zzz").is_empty());
    }

    #[test]
    fn a_folder_file_that_is_already_open_is_listed_once() {
        let found = candidates("q3", &catalog());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hint, Hint::Open);
    }

    #[test]
    fn a_prepared_message_is_drawn_as_chips_again() {
        let prepared = prepare("@selection @Sales @sales-q3.xlsx", &catalog());
        let found = chips(&prepared.text);
        assert_eq!(found.len(), 3);
    }
}
