use std::path::PathBuf;

use zenkai_types::{SheetId, WorkbookId};

use crate::documents::Documents;
use crate::entry::Entry;
use crate::sidebar_item;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Workbook(WorkbookId),
    Sheet(WorkbookId, SheetId),
    Recent(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Hit {
    pub label: String,
    pub detail: String,
    pub target: Target,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Group {
    pub heading: &'static str,
    pub hits: Vec<Hit>,
}

// A link has not read its sheets, so only workbooks in memory contribute sheets.
pub fn collect(documents: &Documents, recent: &[PathBuf]) -> Vec<Group> {
    let workbooks = documents
        .spaces()
        .iter()
        .flat_map(|space| {
            documents.members(space.id).map(|entry| Hit {
                label: entry.name(),
                detail: format!("{}, {}", space.name, folder_of(entry)),
                target: Target::Workbook(entry.id()),
            })
        })
        .collect();
    let recent = recent
        .iter()
        .filter(|path| !documents.has_path(path))
        .map(|path| Hit {
            label: path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            detail: sidebar_item::folder_name(path).unwrap_or_default(),
            target: Target::Recent(path.clone()),
        })
        .collect();
    let sheets = documents
        .iter()
        .flat_map(|document| {
            document.sheets.iter().map(|sheet| Hit {
                label: sheet.name.clone(),
                detail: document.name(),
                target: Target::Sheet(document.id, sheet.id),
            })
        })
        .collect();
    [
        Group {
            heading: "Workbooks",
            hits: workbooks,
        },
        Group {
            heading: "Recent",
            hits: recent,
        },
        Group {
            heading: "Sheets",
            hits: sheets,
        },
    ]
    .into_iter()
    .filter(|group| !group.hits.is_empty())
    .collect()
}

fn folder_of(entry: Entry) -> String {
    entry
        .path()
        .and_then(sidebar_item::folder_name)
        .unwrap_or_else(|| "not saved".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{FileRecord, Session, SpaceRecord, ViewRecord};
    use zenkai_engine::Workbook;

    fn blank() -> Workbook {
        Workbook::new_empty().unwrap()
    }

    fn record(path: &str) -> FileRecord {
        FileRecord {
            path: Some(path.to_string()),
            recovery: None,
            untitled: 0,
            dirty: false,
            read_only: false,
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
        }
    }

    fn labels(group: &Group) -> Vec<&str> {
        group.hits.iter().map(|hit| hit.label.as_str()).collect()
    }

    #[test]
    fn a_fresh_workbook_offers_itself_and_its_sheet() {
        let documents = Documents::new(blank());
        let groups = collect(&documents, &[]);
        let headings: Vec<_> = groups.iter().map(|group| group.heading).collect();
        assert_eq!(headings, ["Workbooks", "Sheets"]);
        assert_eq!(labels(&groups[0]), ["Book1"]);
        assert_eq!(groups[0].hits[0].detail, "Workbooks, not saved");
        assert_eq!(labels(&groups[1]), ["Sheet1"]);
        assert_eq!(groups[1].hits[0].detail, "Book1");
    }

    #[test]
    fn links_are_found_by_name_and_space_but_their_sheets_are_not_listed() {
        let mut documents = Documents::new(blank());
        let session = Session::new(
            false,
            vec![SpaceRecord {
                name: "Q3 close".to_string(),
                collapsed: false,
                color: Default::default(),
                files: vec![record("C:/data/sales.xlsx")],
            }],
        );
        documents.restore(&session, None);
        let groups = collect(&documents, &[]);
        let workbooks = &groups[0];
        assert_eq!(labels(workbooks), ["Book1", "sales.xlsx"]);
        assert_eq!(workbooks.hits[1].detail, "Q3 close, data");
        let sheet_details: Vec<_> = groups[1]
            .hits
            .iter()
            .map(|hit| hit.detail.as_str())
            .collect();
        assert_eq!(sheet_details, ["Book1"]);
    }

    #[test]
    fn recent_files_that_are_already_listed_are_not_repeated() {
        let mut documents = Documents::new(blank());
        let session = Session::new(
            false,
            vec![SpaceRecord {
                name: "Q3".to_string(),
                collapsed: false,
                color: Default::default(),
                files: vec![record("C:/data/sales.xlsx")],
            }],
        );
        documents.restore(&session, None);
        let recent = [
            PathBuf::from("C:/data/sales.xlsx"),
            PathBuf::from("C:/old/budget.xlsx"),
        ];
        let groups = collect(&documents, &recent);
        let recent_group = groups.iter().find(|g| g.heading == "Recent").unwrap();
        assert_eq!(labels(recent_group), ["budget.xlsx"]);
        assert_eq!(
            recent_group.hits[0].target,
            Target::Recent(PathBuf::from("C:/old/budget.xlsx"))
        );
        assert_eq!(recent_group.hits[0].detail, "old");
    }

    #[test]
    fn a_hit_for_a_sheet_points_at_its_workbook_and_sheet() {
        let documents = Documents::new(blank());
        let id = documents.active_id();
        let groups = collect(&documents, &[]);
        let sheets = groups.iter().find(|g| g.heading == "Sheets").unwrap();
        assert_eq!(sheets.hits[0].target, Target::Sheet(id, SheetId(0)));
    }
}
