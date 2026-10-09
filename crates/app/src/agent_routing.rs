use zenkai_agent::protected_view::FileOrigin;
use zenkai_agent::settings::PermissionMode;
use zenkai_agent::tools::{self, AgentAccess, ReadOnlyReason, ToolError, WorkbookSummary};
use zenkai_types::WorkbookId;

use crate::document::Document;
use crate::documents::Documents;
use crate::entry::Entry;

// A workbook in the sidebar that is not loaded is refused instead of loaded: an agent must not
// make Zenkai read files from disk, and the user decides what is open.
pub fn target(documents: &Documents, id: WorkbookId) -> Result<&Document, ToolError> {
    match documents.entry(id) {
        Some(Entry::Loaded(document)) => Ok(document),
        Some(Entry::Link(_)) => Err(ToolError::NotLoaded(id)),
        None => Err(ToolError::UnknownWorkbook(id)),
    }
}

pub fn access(document: &Document, permission: PermissionMode) -> AgentAccess {
    if document.read_only {
        AgentAccess::ReadOnly(ReadOnlyReason::ValuesOnlyFile)
    } else if document.origin == FileOrigin::Internet {
        AgentAccess::ReadOnly(ReadOnlyReason::ProtectedView)
    } else if permission == PermissionMode::ReadOnly {
        AgentAccess::ReadOnly(ReadOnlyReason::Settings)
    } else {
        AgentAccess::Editable
    }
}

pub fn summaries(documents: &Documents, permission: PermissionMode) -> Vec<WorkbookSummary> {
    documents
        .iter()
        .map(|document| {
            let space = documents
                .spaces()
                .get(document.space)
                .map_or("", |space| space.name.as_str());
            let path = document
                .path
                .as_ref()
                .map(|path| path.display().to_string());
            tools::workbook_summary(
                document.id,
                &document.name(),
                &tools::WorkbookPlace {
                    space,
                    location: path.as_deref().unwrap_or("not saved"),
                },
                &document.sheets,
                access(document, permission),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use zenkai_agent::tools::{
        PlannedWrite, ReadRange, ReadRequest, ToolReply, WriteCells, WriteRequest, plan_write,
    };
    use zenkai_engine::Workbook;

    use super::*;
    use crate::document;
    use crate::entry::LinkStatus;

    fn open(documents: &mut Documents, name: &str) -> WorkbookId {
        let workbook = Workbook::new_empty().unwrap();
        documents.open(workbook, Some(PathBuf::from(name)), Vec::new())
    }

    fn two_documents() -> (Documents, WorkbookId, WorkbookId) {
        let mut documents = Documents::new(Workbook::new_empty().unwrap());
        documents.active_mut().dirty = true;
        let first = open(&mut documents, "first.xlsx");
        let second = open(&mut documents, "second.xlsx");
        (documents, first, second)
    }

    fn write_request(id: WorkbookId, entry: &str) -> WriteRequest {
        WriteRequest::WriteCells(WriteCells {
            workbook: id,
            sheet: "Sheet1".to_string(),
            start: "A1".to_string(),
            rows: vec![vec![entry.to_string()]],
        })
    }

    fn write(documents: &mut Documents, id: WorkbookId, entry: &str) {
        let plan: PlannedWrite = {
            let document = target(documents, id).unwrap();
            plan_write(&write_request(id, entry), &document.sheets).unwrap()
        };
        let document = documents.get_mut(id).unwrap();
        document.queue(Box::new(move |workbook| plan.apply(workbook)));
        let (shared, edits) = document.take_batch().unwrap();
        assert!(document::run_batch(&shared, edits).is_empty());
        let generation = document.generation();
        assert!(document.finish_batch(generation));
    }

    fn read_a1(documents: &Documents, id: WorkbookId) -> Result<String, ToolError> {
        let document = target(documents, id)?;
        let request = ReadRequest::ReadRange(ReadRange {
            workbook: id,
            sheet: "Sheet1".to_string(),
            range: "A1".to_string(),
            page: 0,
        });
        let shared = document.begin_read().unwrap();
        match tools::read(&request, &document::read_shared(&shared))? {
            ToolReply::Cells(page) => Ok(page.values[0][0].clone()),
            other => panic!("unexpected reply {other:?}"),
        }
    }

    #[test]
    fn every_loaded_document_is_listed_with_its_id() {
        let (documents, first, second) = two_documents();
        let listed: Vec<WorkbookId> = summaries(&documents, PermissionMode::AskBeforeWrite)
            .iter()
            .map(|summary| summary.id)
            .collect();
        assert!(listed.contains(&first) && listed.contains(&second));
        assert_eq!(listed.len(), 3);
    }

    #[test]
    fn each_write_lands_in_the_document_its_id_names() {
        let (mut documents, first, second) = two_documents();
        assert_eq!(documents.active_id(), second);
        write(&mut documents, first, "one");
        write(&mut documents, second, "two");
        assert_eq!(read_a1(&documents, first).unwrap(), "one");
        assert_eq!(read_a1(&documents, second).unwrap(), "two");
    }

    #[test]
    fn a_closed_document_id_is_unknown_and_the_other_still_works() {
        let (mut documents, first, second) = two_documents();
        write(&mut documents, first, "kept");
        documents.close(second, || Workbook::new_empty().unwrap());
        assert_eq!(
            read_a1(&documents, second),
            Err(ToolError::UnknownWorkbook(second))
        );
        assert_eq!(read_a1(&documents, first).unwrap(), "kept");
    }

    #[test]
    fn an_untouched_blank_replaced_by_an_open_has_no_id_any_more() {
        let mut documents = Documents::new(Workbook::new_empty().unwrap());
        let blank = documents.active_id();
        let opened = open(&mut documents, "opened.xlsx");
        assert_eq!(
            target(&documents, blank).err(),
            Some(ToolError::UnknownWorkbook(blank))
        );
        assert!(target(&documents, opened).is_ok());
    }

    #[test]
    fn an_unloaded_document_is_not_loaded_rather_than_unknown() {
        let (mut documents, first, _) = two_documents();
        assert!(documents.unload(first));
        assert_eq!(read_a1(&documents, first), Err(ToolError::NotLoaded(first)));
        assert!(
            summaries(&documents, PermissionMode::AskBeforeWrite)
                .iter()
                .all(|summary| summary.id != first)
        );
        assert!(matches!(
            documents.entry(first),
            Some(Entry::Link(link)) if link.status == LinkStatus::NotLoaded
        ));
    }

    #[test]
    fn protected_view_and_settings_make_a_document_read_only_for_agents() {
        let (mut documents, first, second) = two_documents();
        documents.get_mut(first).unwrap().origin = FileOrigin::Internet;
        let listed = summaries(&documents, PermissionMode::AskBeforeWrite);
        let access_of = |id| listed.iter().find(|s| s.id == id).unwrap().access;
        assert_eq!(
            access_of(first),
            AgentAccess::ReadOnly(ReadOnlyReason::ProtectedView)
        );
        assert_eq!(access_of(second), AgentAccess::Editable);
        assert!(
            summaries(&documents, PermissionMode::ReadOnly)
                .iter()
                .all(|s| matches!(s.access, AgentAccess::ReadOnly(_)))
        );
    }

    #[test]
    fn the_origin_survives_an_unload() {
        let (mut documents, first, _) = two_documents();
        documents.get_mut(first).unwrap().origin = FileOrigin::Internet;
        assert!(documents.unload(first));
        let link = documents.entry(first).unwrap().to_link();
        assert_eq!(link.origin, Some(FileOrigin::Internet));
    }
}
