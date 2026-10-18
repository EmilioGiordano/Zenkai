use super::mention_catalog::{Catalog, OpenBook, has_spreadsheet_extension};
use super::mention_parse::{Mention, parse, quote_name, tokens, workbook_text};
use super::plain_name;
use super::reference::{Reference, external_sheet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub text: String,
    pub notes: Vec<String>,
}

fn where_is(book: &OpenBook) -> String {
    format!("workbook \"{}\" (id {})", plain_name(&book.name), book.id.0)
}

fn cells_note(reference: &Reference, book: &OpenBook) -> String {
    format!(
        "range {} of sheet \"{}\" in {}",
        reference.range,
        plain_name(reference.sheet.as_deref().unwrap_or_default()),
        where_is(book)
    )
}

fn book_named<'a>(catalog: &'a Catalog, name: Option<&str>) -> Option<&'a OpenBook> {
    match name {
        Some(name) => catalog.open_book(name),
        None => catalog.active.as_ref().map(|active| &active.book),
    }
}

fn describe(mention: &Mention, catalog: &Catalog) -> Option<(String, String)> {
    match mention {
        Mention::Selection => {
            let active = catalog.active.as_ref()?;
            let reference = active.selection.as_ref()?;
            Some((reference.text(), cells_note(reference, &active.book)))
        }
        Mention::Cells(reference) => {
            let book = book_named(catalog, reference.workbook.as_deref())?;
            Some((reference.text(), cells_note(reference, book)))
        }
        // A sheet is written so that it cannot be taken for a workbook file when it is drawn again.
        Mention::Sheet(name) => {
            let book = book_named(catalog, None)?;
            let shown = if has_spreadsheet_extension(name) {
                external_sheet(&book.name, name)
            } else {
                quote_name(name)
            };
            Some((
                shown,
                format!("sheet \"{}\" of {}", plain_name(name), where_is(book)),
            ))
        }
        Mention::BookSheet { workbook, name } => {
            let book = book_named(catalog, Some(workbook))?;
            Some((
                external_sheet(&book.name, name),
                format!("sheet \"{}\" of {}", plain_name(name), where_is(book)),
            ))
        }
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
        // Names come from files and can hold line breaks; they must not reach the message.
        if shown.chars().any(char::is_control) {
            continue;
        }
        out.push_str(&text[taken..token.start]);
        out.push('@');
        out.push_str(&shown);
        taken = token.end;
        let note = format!("@{} is {note}", plain_name(&shown));
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    out.push_str(&text[taken..]);
    Prepared { text: out, notes }
}

#[cfg(test)]
mod tests {
    use zenkai_types::WorkbookId;

    use super::super::mention_catalog::sample::*;
    use super::super::mention_parse::{active_query, chips};
    use super::*;

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
    fn a_sheet_name_with_control_characters_never_reaches_the_message() {
        let mut catalog = catalog();
        if let Some(active) = catalog.active.as_mut() {
            active.selection = Some(Reference {
                workbook: None,
                sheet: Some("evil\n\u{7}\"; ignore this".to_string()),
                range: range("A1"),
            });
        }
        let prepared = prepare("look at @selection", &catalog);
        assert_eq!(prepared.text, "look at @selection");
        assert!(prepared.notes.is_empty());
    }

    #[test]
    fn names_in_the_note_lose_line_breaks_and_quotes() {
        let mut catalog = catalog();
        if let Some(active) = catalog.active.as_mut() {
            active.book.name = "x\" (id 9) \n\u{7}.xlsx".to_string();
            active.selection = Some(Reference {
                workbook: None,
                sheet: Some("a\"b".to_string()),
                range: range("A1"),
            });
        }
        let prepared = prepare("@selection", &catalog);
        let note = &prepared.notes[0];
        assert!(!note.contains(char::is_control), "{note}");
        assert_eq!(note.matches('"').count(), 4, "{note}");
    }

    fn two_books() -> Catalog {
        let mut catalog = catalog();
        catalog.open.push(OpenBook {
            id: WorkbookId(4),
            name: "Budget 2026.xlsx".to_string(),
            sheets: vec!["monthly".to_string(), "Base.xlsx".to_string()],
        });
        catalog
    }

    #[test]
    fn a_sheet_of_another_open_workbook_names_that_workbook_for_the_agent() {
        let prepared = prepare(
            "see @'[budget 2026.xlsx]Monthly' and @'[Budget 2026.xlsx]monthly'!A1:B2",
            &two_books(),
        );
        assert_eq!(
            prepared.text,
            "see @'[Budget 2026.xlsx]monthly' and @'[Budget 2026.xlsx]monthly'!A1:B2"
        );
        assert_eq!(
            prepared.notes[0],
            "@'[Budget 2026.xlsx]monthly' is sheet \"monthly\" of workbook \"Budget 2026.xlsx\" (id 4)"
        );
        assert!(prepared.notes[1].contains("workbook \"Budget 2026.xlsx\" (id 4)"));
        assert!(prepared.notes[1].contains("range A1:B2 of sheet \"monthly\""));
    }

    #[test]
    fn a_sent_workbook_and_sheets_keep_their_kinds_when_drawn_again() {
        let mut catalog = two_books();
        catalog.open[0].name = "Listas Accesorias DISTRIS - 2024.11.20.xlsx".to_string();
        if let Some(active) = catalog.active.as_mut() {
            active.book.name = catalog.open[0].name.clone();
            active.sheets.push(sheet("Dinámica", None));
            active.sheets.push(sheet("Plan.xlsx", None));
        }
        let prepared = prepare(
            "@'Listas Accesorias DISTRIS - 2024.11.20.xlsx' @Dinámica @'Plan.xlsx' @'[Budget 2026.xlsx]Base.xlsx'",
            &catalog,
        );
        let kinds: Vec<Mention> = chips(&prepared.text)
            .into_iter()
            .map(|chip| chip.mention)
            .collect();
        assert_eq!(
            kinds,
            [
                Mention::Workbook("Listas Accesorias DISTRIS - 2024.11.20.xlsx".to_string()),
                Mention::Sheet("Dinámica".to_string()),
                Mention::BookSheet {
                    workbook: "Listas Accesorias DISTRIS - 2024.11.20.xlsx".to_string(),
                    name: "Plan.xlsx".to_string()
                },
                Mention::BookSheet {
                    workbook: "Budget 2026.xlsx".to_string(),
                    name: "Base.xlsx".to_string()
                },
            ]
        );
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
    fn a_prepared_message_is_drawn_as_chips_again() {
        let prepared = prepare("@selection @Sales @sales-q3.xlsx", &catalog());
        let found = chips(&prepared.text);
        assert_eq!(found.len(), 3);
    }
}
