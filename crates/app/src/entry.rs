use std::path::{Path, PathBuf};

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::Unsupported;
use zenkai_grid::ViewState;
use zenkai_types::{SheetId, WorkbookId};

use crate::document::{Document, FileJob};
use crate::spaces::SpaceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    NotLoaded,
    Loading,
    Missing,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub id: WorkbookId,
    pub space: SpaceId,
    pub untitled: u32,
    pub path: Option<PathBuf>,
    // The autosaved copy holding unsaved work; loaded instead of `path`.
    pub recovery: Option<PathBuf>,
    pub dirty: bool,
    pub read_only: bool,
    pub unsupported: Vec<Unsupported>,
    pub sheet: SheetId,
    pub view: ViewState,
    pub size: Option<u64>,
    pub status: LinkStatus,
    // The session pointed at a recovery copy that could not be trusted or found.
    pub recovery_lost: bool,
    // Known once the workbook has been loaded; a link restored from a session works it out on load.
    pub origin: Option<FileOrigin>,
}

pub fn display_name(path: Option<&Path>, untitled: u32) -> String {
    path.and_then(|path| path.file_name()).map_or_else(
        || format!("Book{untitled}"),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl Link {
    pub fn name(&self) -> String {
        display_name(self.path.as_deref(), self.untitled)
    }
}

// What the list stores for a workbook that is not on screen.
pub enum Slot {
    Loaded(Box<Document>),
    Link(Link),
}

// A borrowed look at any workbook of the list, on screen or not.
#[derive(Clone, Copy)]
pub enum Entry<'a> {
    Loaded(&'a Document),
    Link(&'a Link),
}

impl Slot {
    pub fn as_entry(&self) -> Entry<'_> {
        match self {
            Slot::Loaded(document) => Entry::Loaded(document),
            Slot::Link(link) => Entry::Link(link),
        }
    }

    pub fn id(&self) -> WorkbookId {
        self.as_entry().id()
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self, Slot::Loaded(_))
    }

    pub fn set_space(&mut self, space: SpaceId) {
        match self {
            Slot::Loaded(document) => document.space = space,
            Slot::Link(link) => link.space = space,
        }
    }

    pub fn loaded_mut(&mut self) -> Option<&mut Document> {
        match self {
            Slot::Loaded(document) => Some(document),
            Slot::Link(_) => None,
        }
    }
}

impl<'a> Entry<'a> {
    pub fn id(self) -> WorkbookId {
        match self {
            Entry::Loaded(document) => document.id,
            Entry::Link(link) => link.id,
        }
    }

    pub fn space(self) -> SpaceId {
        match self {
            Entry::Loaded(document) => document.space,
            Entry::Link(link) => link.space,
        }
    }

    pub fn path(self) -> Option<&'a Path> {
        match self {
            Entry::Loaded(document) => document.path.as_deref(),
            Entry::Link(link) => link.path.as_deref(),
        }
    }

    pub fn dirty(self) -> bool {
        match self {
            Entry::Loaded(document) => document.dirty,
            Entry::Link(link) => link.dirty,
        }
    }

    pub fn name(self) -> String {
        match self {
            Entry::Loaded(document) => document.name(),
            Entry::Link(link) => link.name(),
        }
    }

    pub fn loaded(self) -> Option<&'a Document> {
        match self {
            Entry::Loaded(document) => Some(document),
            Entry::Link(_) => None,
        }
    }

    pub fn to_link(self) -> Link {
        match self {
            Entry::Loaded(document) => document.to_link(),
            Entry::Link(link) => link.clone(),
        }
    }
}

impl Document {
    pub fn to_link(&self) -> Link {
        Link {
            id: self.id,
            space: self.space,
            untitled: self.untitled(),
            path: self.path.clone(),
            recovery: None,
            dirty: self.dirty,
            read_only: self.read_only,
            unsupported: self.unsupported.clone(),
            sheet: self.sheet,
            view: self.view,
            size: None,
            status: LinkStatus::NotLoaded,
            recovery_lost: false,
            origin: Some(self.origin),
        }
    }

    // Work that exists only in memory: unsaved edits, or an untitled workbook such as a CSV
    // import that was never saved.
    pub fn needs_recovery(&self) -> bool {
        self.dirty || (self.path.is_none() && !self.is_pristine())
    }

    // A clean workbook with a file to come back from, doing nothing right now.
    pub fn can_unload(&self) -> bool {
        self.path.is_some()
            && !self.needs_recovery()
            && !self.has_pending()
            && self.file_job() == FileJob::Idle
    }
}
