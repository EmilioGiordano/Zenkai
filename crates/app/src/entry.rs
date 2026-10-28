use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Kept, Stamp, Unsupported};
use zenkai_grid::ViewState;
use zenkai_i18n::t;
use zenkai_types::{SheetId, WorkbookId};

use crate::document::{Document, FileJob};
use crate::pdf::PdfPosition;
use crate::pdf_tab::PdfTab;
use crate::spaces::SpaceId;
use crate::text_document::{DocumentKind, TextDocument, ViewMode, text_name};
use crate::text_file::StampRecord;
use crate::text_zoom::TextZoom;

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
    pub kind: DocumentKind,
    pub mode: ViewMode,
    // The disk stamp a text's edits were based on, kept across a restart.
    pub base: Option<StampRecord>,
    pub pdf: PdfPosition,
    pub text_zoom: TextZoom,
    // Set while the workbook is out of memory by choice; a link restored from a session or
    // found in a folder never has it.
    pub rested: Option<Box<Rested>>,
}

// The file as the workbook last had it, to tell on the way back whether it changed while the
// workbook was out of memory. The stamp is the engine's, taken when the file was last read or
// written; without one, `at` is the file's modified time as the workbook knew it, or when it
// last looked, and a file modified after it is taken as changed.
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
        match self.kind {
            DocumentKind::Workbook => display_name(self.path.as_deref(), self.untitled),
            DocumentKind::Markdown | DocumentKind::Pdf => {
                text_name(self.path.as_deref(), self.untitled)
            }
        }
    }
}

// What the list stores for a workbook that is not on screen.
pub enum Slot {
    Loaded(Box<Document>),
    Text(Box<TextDocument>),
    Pdf(Box<PdfTab>),
    Link(Link),
}

// A borrowed look at any workbook of the list, on screen or not.
#[derive(Clone, Copy)]
pub enum Entry<'a> {
    Loaded(&'a Document),
    Text(&'a TextDocument),
    Pdf(&'a PdfTab),
    Link(&'a Link),
}

impl Slot {
    pub fn as_entry(&self) -> Entry<'_> {
        match self {
            Slot::Loaded(document) => Entry::Loaded(document),
            Slot::Text(text) => Entry::Text(text),
            Slot::Pdf(pdf) => Entry::Pdf(pdf),
            Slot::Link(link) => Entry::Link(link),
        }
    }

    pub fn id(&self) -> WorkbookId {
        self.as_entry().id()
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self, Slot::Loaded(_) | Slot::Text(_) | Slot::Pdf(_))
    }

    pub fn set_space(&mut self, space: SpaceId) {
        match self {
            Slot::Loaded(document) => document.space = space,
            Slot::Text(text) => text.space = space,
            Slot::Pdf(pdf) => pdf.space = space,
            Slot::Link(link) => link.space = space,
        }
    }

    pub fn loaded_mut(&mut self) -> Option<&mut Document> {
        match self {
            Slot::Loaded(document) => Some(document),
            Slot::Text(_) | Slot::Pdf(_) | Slot::Link(_) => None,
        }
    }
}

impl<'a> Entry<'a> {
    pub fn id(self) -> WorkbookId {
        match self {
            Entry::Loaded(document) => document.id,
            Entry::Text(text) => text.id,
            Entry::Pdf(pdf) => pdf.id,
            Entry::Link(link) => link.id,
        }
    }

    pub fn space(self) -> SpaceId {
        match self {
            Entry::Loaded(document) => document.space,
            Entry::Text(text) => text.space,
            Entry::Pdf(pdf) => pdf.space,
            Entry::Link(link) => link.space,
        }
    }

    pub fn path(self) -> Option<&'a Path> {
        match self {
            Entry::Loaded(document) => document.path.as_deref(),
            Entry::Text(text) => text.path.as_deref(),
            Entry::Pdf(pdf) => Some(pdf.path.as_path()),
            Entry::Link(link) => link.path.as_deref(),
        }
    }

    pub fn untitled(self) -> u32 {
        match self {
            Entry::Loaded(document) => document.untitled(),
            Entry::Text(text) => text.untitled(),
            Entry::Pdf(_) => 0,
            Entry::Link(link) => link.untitled,
        }
    }

    pub fn dirty(self) -> bool {
        match self {
            Entry::Loaded(document) => document.dirty,
            Entry::Text(text) => text.needs_recovery(),
            Entry::Pdf(_) => false,
            Entry::Link(link) => link.dirty,
        }
    }

    pub fn name(self) -> String {
        match self {
            Entry::Loaded(document) => document.name(),
            Entry::Text(text) => text.name(),
            Entry::Pdf(pdf) => pdf.name(),
            Entry::Link(link) => link.name(),
        }
    }

    pub fn loaded(self) -> Option<&'a Document> {
        match self {
            Entry::Loaded(document) => Some(document),
            Entry::Text(_) | Entry::Pdf(_) | Entry::Link(_) => None,
        }
    }

    pub fn text(self) -> Option<&'a TextDocument> {
        match self {
            Entry::Text(text) => Some(text),
            Entry::Loaded(_) | Entry::Pdf(_) | Entry::Link(_) => None,
        }
    }

    pub fn pdf(self) -> Option<&'a PdfTab> {
        match self {
            Entry::Pdf(pdf) => Some(pdf),
            Entry::Loaded(_) | Entry::Text(_) | Entry::Link(_) => None,
        }
    }

    pub fn kind(self) -> DocumentKind {
        match self {
            Entry::Loaded(_) => DocumentKind::Workbook,
            Entry::Text(text) => text.kind,
            Entry::Pdf(_) => DocumentKind::Pdf,
            Entry::Link(link) => link.kind,
        }
    }

    pub fn to_link(self) -> Link {
        match self {
            Entry::Loaded(document) => document.to_link(),
            Entry::Text(text) => text.to_link(),
            Entry::Pdf(pdf) => pdf.to_link(),
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
            kind: DocumentKind::Workbook,
            mode: ViewMode::default(),
            base: None,
            pdf: Default::default(),
            text_zoom: Default::default(),
            rested: Some(Box::new(Rested {
                file: self.file_seen(),
                hibernation: None,
            })),
        }
    }

    // A restored workbook stands on a recovery copy, whose own stamp says nothing about the
    // file, so the guard it was restored with is what carries on.
    fn file_seen(&self) -> FileSeen {
        self.file_guard().unwrap_or_else(|| FileSeen {
            stamp: self.workbook().and_then(|workbook| workbook.file_stamp()),
            at: SystemTime::now(),
        })
    }

    // The link a workbook with unsaved work becomes when it goes to disk. Its recovery copy
    // holds the same work, and is what loads if the dump cannot be read.
    pub fn to_hibernated_link(&self, recovery: PathBuf, dump: PathBuf, kept: Kept) -> Link {
        Link {
            dirty: self.needs_recovery(),
            recovery: Some(recovery),
            rested: Some(Box::new(Rested {
                file: self.file_seen(),
                hibernation: Some(Hibernation {
                    dump,
                    kept,
                    restored: self.restored,
                }),
            })),
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
