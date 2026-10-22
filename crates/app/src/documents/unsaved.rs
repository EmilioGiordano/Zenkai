use std::path::{Path, PathBuf};
use std::time::Duration;

use zenkai_types::WorkbookId;

use super::Documents;
use crate::document::Document;
use crate::entry::Entry;
use crate::text_document::TextDocument;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsaved {
    pub id: WorkbookId,
    pub name: String,
    pub folder: Option<PathBuf>,
    pub since_edit: Option<Duration>,
}

// A restored workbook that has not loaded yet still holds its unsaved work in a recovery copy.
fn holds_unsaved_work(entry: Entry<'_>) -> bool {
    entry.dirty() || entry.loaded().is_some_and(Document::needs_recovery)
}

impl Documents {
    pub fn unsaved(&self) -> Vec<Unsaved> {
        self.entries()
            .filter(|entry| holds_unsaved_work(*entry))
            .map(|entry| Unsaved {
                id: entry.id(),
                name: entry.name(),
                folder: entry
                    .path()
                    .and_then(Path::parent)
                    .filter(|folder| !folder.as_os_str().is_empty())
                    .map(Path::to_path_buf),
                since_edit: entry
                    .loaded()
                    .and_then(Document::since_last_edit)
                    .or_else(|| entry.text().and_then(TextDocument::since_last_edit)),
            })
            .collect()
    }

    pub fn holds_unsaved_work(&self, id: WorkbookId) -> bool {
        self.entry(id).is_some_and(holds_unsaved_work)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::session::{FileRecord, Session, SpaceRecord, ViewRecord};

    #[test]
    fn a_clean_workbook_and_an_untouched_blank_are_not_listed() {
        let mut documents = documents();
        open_file(&mut documents, "saved.xlsx");
        assert!(documents.unsaved().is_empty());
    }

    #[test]
    fn edited_workbooks_are_listed_in_tab_order_with_their_folder() {
        let mut documents = documents();
        let first = open_file(&mut documents, "reports/q3.xlsx");
        documents.get_mut(first).unwrap().dirty = true;
        let second = open_file(&mut documents, "budget.xlsx");
        documents.get_mut(second).unwrap().dirty = true;
        let listed = documents.unsaved();
        let names: Vec<_> = listed.iter().map(|unsaved| unsaved.name.as_str()).collect();
        assert_eq!(names, ["q3.xlsx", "budget.xlsx"]);
        assert_eq!(listed[0].folder.as_deref(), Some(Path::new("reports")));
        assert_eq!(listed[1].folder, None);
        assert!(documents.holds_unsaved_work(first));
    }

    #[test]
    fn a_workbook_never_saved_is_listed_without_a_folder_once_it_has_work() {
        let documents = busy();
        let blank = documents.active_id().unwrap();
        let listed = documents.unsaved();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, blank);
        assert_eq!(listed[0].folder, None);
        assert_eq!(listed[0].name, "Book1");
    }

    #[test]
    fn a_restored_link_holding_unsaved_work_is_listed_before_it_loads() {
        let record = FileRecord {
            path: Some("kept.xlsx".to_string()),
            recovery: Some("autosave-1-2-3.xlsx".to_string()),
            untitled: 0,
            dirty: true,
            read_only: false,
            kind: Default::default(),
            mode: Default::default(),
            base: None,
            pdf: Default::default(),
            unsupported: Vec::new(),
            active: false,
            sheet: 0,
            view: ViewRecord {
                active_row: 0,
                active_col: 0,
                corner_row: 0,
                corner_col: 0,
                top: 0,
                left: 0,
            },
        };
        let session = Session::new(
            true,
            vec![SpaceRecord {
                name: "Work".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![record],
            }],
        );
        let mut documents = documents();
        documents.restore(&session, None);
        let listed = documents.unsaved();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "kept.xlsx");
        assert_eq!(listed[0].since_edit, None);
    }
}
