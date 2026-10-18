use super::mention_catalog::{Catalog, Used};
use super::mention_parse::{SELECTION, sheet_text, workbook_text};

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
    Pending,
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
    let matches =
        |name: &str| !name.chars().any(char::is_control) && name.to_lowercase().contains(&wanted);
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
                hint: match sheet.used {
                    Used::Unknown => Hint::Pending,
                    Used::Empty => Hint::EmptySheet,
                    Used::Range(range) => Hint::UsedRange(range.to_string()),
                },
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
    use super::super::mention_catalog::sample::*;
    use super::*;

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
    fn a_sheet_whose_used_range_is_not_known_yet_has_no_hint() {
        let mut catalog = catalog();
        if let Some(active) = catalog.active.as_mut() {
            active.sheets[0].used = Used::Unknown;
        }
        assert_eq!(candidates("", &catalog)[1].hint, Hint::Pending);
    }

    #[test]
    fn names_with_control_characters_are_not_offered() {
        let mut catalog = catalog();
        if let Some(active) = catalog.active.as_mut() {
            active.sheets.push(sheet("evil\nname", None));
        }
        catalog.folder.push("bad\u{7}.xlsx".to_string());
        let labels: Vec<String> = candidates("", &catalog)
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();
        assert!(
            labels
                .iter()
                .all(|l| !l.contains("evil") && !l.contains("bad"))
        );
    }

    #[test]
    fn a_folder_file_that_is_already_open_is_listed_once() {
        let found = candidates("q3", &catalog());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hint, Hint::Open);
    }
}
