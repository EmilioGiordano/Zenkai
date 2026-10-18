use zenkai_types::{Range, SheetId, WorkbookId};

use super::reference::Reference;

const SPREADSHEET_EXTENSIONS: [&str; 7] = ["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Used {
    Unknown,
    Empty,
    Range(Range),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetEntry {
    pub id: SheetId,
    pub name: String,
    pub used: Used,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenBook {
    pub id: WorkbookId,
    pub name: String,
    pub sheets: Vec<String>,
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
    pub fn fill_used(&mut self, book: WorkbookId, used: &[(SheetId, Used)]) {
        let Some(active) = self.active.as_mut().filter(|active| active.book.id == book) else {
            return;
        };
        for sheet in &mut active.sheets {
            if let Some((_, found)) = used.iter().find(|(id, _)| *id == sheet.id) {
                sheet.used = *found;
            }
        }
    }

    pub(super) fn sheet(&self, name: &str) -> Option<String> {
        let wanted = name.to_lowercase();
        self.active
            .as_ref()?
            .sheets
            .iter()
            .find(|sheet| sheet.name.to_lowercase() == wanted)
            .map(|sheet| sheet.name.clone())
    }

    pub(super) fn sheet_in(&self, book: &str, sheet: &str) -> Option<(String, String)> {
        let open = self.open_book(book)?;
        let wanted = sheet.to_lowercase();
        open.sheets
            .iter()
            .find(|name| name.to_lowercase() == wanted)
            .map(|name| (open.name.clone(), name.clone()))
    }

    pub(super) fn open_book(&self, name: &str) -> Option<&OpenBook> {
        self.open
            .iter()
            .find(|book| book.name.eq_ignore_ascii_case(name))
    }

    pub(super) fn workbook(&self, name: &str) -> Option<String> {
        self.open_book(name)
            .map(|book| book.name.clone())
            .or_else(|| {
                self.folder
                    .iter()
                    .find(|file| file.eq_ignore_ascii_case(name))
                    .cloned()
            })
    }

    pub(super) fn folder_only(&self) -> impl Iterator<Item = &String> {
        self.folder.iter().filter(|file| {
            let file_name = file.rsplit('/').next().unwrap_or(file);
            self.open_book(file_name).is_none()
        })
    }
}

pub(super) fn has_spreadsheet_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && SPREADSHEET_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

#[cfg(test)]
pub(super) mod sample {
    use super::*;
    use crate::chat::mention_parse::Mention;

    pub fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    pub fn sheet(name: &str, used: Option<&str>) -> SheetEntry {
        SheetEntry {
            id: SheetId(0),
            name: name.to_string(),
            used: used.map_or(Used::Empty, |text| Used::Range(range(text))),
        }
    }

    pub fn catalog() -> Catalog {
        let book = OpenBook {
            id: WorkbookId(3),
            name: "sales-q3.xlsx".to_string(),
            sheets: vec![
                "Sales".to_string(),
                "Sales summary".to_string(),
                "selection".to_string(),
            ],
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
                    workbook: None,
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

    pub fn cells(sheet: &str, text: &str) -> Mention {
        Mention::Cells(Reference {
            workbook: None,
            sheet: Some(sheet.to_string()),
            range: range(text),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::sample::*;
    use super::*;

    #[test]
    fn a_sheet_of_another_open_workbook_is_found_by_book_and_sheet_ignoring_case() {
        let mut catalog = catalog();
        catalog.open.push(OpenBook {
            id: WorkbookId(4),
            name: "Budget.xlsx".to_string(),
            sheets: vec!["monthly".to_string(), "Summary".to_string()],
        });
        assert_eq!(
            catalog.sheet_in("budget.XLSX", "SUMMARY"),
            Some(("Budget.xlsx".to_string(), "Summary".to_string()))
        );
        assert_eq!(catalog.sheet_in("Budget.xlsx", "Nope"), None);
        assert_eq!(catalog.sheet_in("Other.xlsx", "Summary"), None);
    }

    #[test]
    fn used_ranges_fill_in_by_sheet_for_the_right_workbook_only() {
        let mut catalog = catalog();
        if let Some(active) = catalog.active.as_mut() {
            active.sheets[0].used = Used::Unknown;
        }
        let found = [(SheetId(0), Used::Range(range("A1:B2")))];
        catalog.fill_used(WorkbookId(9), &found);
        let first = |catalog: &Catalog| catalog.active.as_ref().map(|a| a.sheets[0].used);
        assert_eq!(first(&catalog), Some(Used::Unknown));
        catalog.fill_used(WorkbookId(3), &found);
        assert_eq!(first(&catalog), Some(Used::Range(range("A1:B2"))));
    }
}
