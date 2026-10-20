use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Kept, Stamp, Unsupported};
use zenkai_grid::ViewState;
use zenkai_i18n::t;
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
    // Set while the workbook is out of memory by choice; a link restored from a session or
    // found in a folder never has it.
    pub rested: Option<Rested>,
}

// The file as the workbook last had it, to tell on the way back whether it changed while the
// workbook was out of memory. The stamp is the engine's, taken when the file was last read or
// written; without one, a file modified after `at` is taken as changed.
#[derive(Clone, Copy, Debug)]
pub struct FileSeen {
    pub stamp: Option<Stamp>,
    pub at: SystemTime,
}

impl FileSeen {
    pub fn changed(&self, now: Stamp) -> bool {
        match self.stamp {
            Some(seen) => seen != now,
            None => now.modified() > self.at,
        }
    }
}

// What a workbook with unsaved work left in memory when it went to disk: where its dump is,
// and the few things the engine needs to save it as it would have.
#[derive(Clone, Debug)]
pub struct Hibernation {
    pub dump: PathBuf,
    pub kept: Kept,
    // Whether the workbook came from a recovery copy, which can lack what its file holds.
    pub restored: bool,
}

#[derive(Clone, Debug)]
pub struct Rested {
    pub file: FileSeen,
    pub hibernation: Option<Hibernation>,
}

pub fn display_name(path: Option<&Path>, untitled: u32) -> String {
    path.and_then(|path| path.file_name()).map_or_else(
        || t!("document.untitled", number = untitled),
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

    pub fn untitled(self) -> u32 {
        match self {
            Entry::Loaded(document) => document.untitled(),
            Entry::Link(link) => link.untitled,
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
            rested: Some(Rested {
                file: self.file_seen(),
                hibernation: None,
            }),
        }
    }

    fn file_seen(&self) -> FileSeen {
        FileSeen {
            stamp: self.workbook().and_then(|workbook| workbook.file_stamp()),
            at: SystemTime::now(),
        }
    }

    // The link a workbook with unsaved work becomes when it goes to disk. Its recovery copy
    // holds the same work, and is what loads if the dump cannot be read.
    pub fn to_hibernated_link(&self, recovery: PathBuf, dump: PathBuf, kept: Kept) -> Link {
        Link {
            dirty: self.needs_recovery(),
            recovery: Some(recovery),
            rested: Some(Rested {
                file: self.file_seen(),
                hibernation: Some(Hibernation {
                    dump,
                    kept,
                    restored: self.restored,
                }),
            }),
            ..self.to_link()
        }
    }

    // Work that exists only in memory: unsaved edits, or an untitled workbook such as a CSV
    // import that was never saved.
    pub fn needs_recovery(&self) -> bool {
        self.dirty || (self.path.is_none() && !self.is_pristine())
    }

    // Unsaved work whose recovery copy holds all of it, doing nothing right now: if the dump
    // cannot be read back, the copy still has the work.
    pub fn can_hibernate(&self) -> bool {
        self.needs_recovery()
            && self.recovery_is_current()
            && self.has_recovery_copy()
            && self.review.is_empty()
            && !self.has_pending()
            && !self.has_pending_save()
            && self.file_job() == FileJob::Idle
            && !self.hibernation_was_refused()
    }

    // A clean workbook with a file to come back from, doing nothing right now.
    pub fn can_unload(&self) -> bool {
        self.path.is_some()
            && !self.needs_recovery()
            && !self.has_pending()
            && self.file_job() == FileJob::Idle
    }
}
