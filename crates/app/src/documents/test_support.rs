use std::path::PathBuf;

use zenkai_engine::Workbook;
use zenkai_types::WorkbookId;

use super::{Documents, Loaded};
use crate::entry::Entry;

pub fn blank() -> Workbook {
    Workbook::new_empty().unwrap()
}

pub fn documents() -> Documents {
    Documents::new(blank())
}

// The workbook on screen has work, so opening another adds a tab instead of replacing it.
pub fn busy() -> Documents {
    let mut documents = documents();
    documents.active_mut().unwrap().dirty = true;
    documents
}

pub fn open_file(documents: &mut Documents, name: &str) -> WorkbookId {
    documents.open(blank(), Some(PathBuf::from(name)), Vec::new())
}

pub fn names(documents: &Documents) -> Vec<String> {
    documents.entries().map(Entry::name).collect()
}

pub fn loaded(id: WorkbookId) -> Loaded {
    Loaded {
        id,
        workbook: blank(),
        unsupported: Vec::new(),
        read_only: false,
        origin: zenkai_agent::protected_view::FileOrigin::Local,
        from_recovery: false,
    }
}
