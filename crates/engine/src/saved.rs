use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use ironcalc::base::types::Workbook;
use zenkai_xlsx_reader::{PackageLayout, PartNames, SharedStrings};

use crate::error::EngineError;
use crate::fingerprint::{ROW_PRINT_BUDGET_BYTES, SheetPrint, book_print, sheet_print};
use crate::held::Held;
use crate::save::ModelData;
use crate::sheet_elements::Relationships;
use crate::sheet_settings::Carried;
use crate::style_growth::StylesBase;

// A file is taken as unchanged on disk while its length and modification time are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: SystemTime,
}

impl Stamp {
    // Matches no file, so a path that could not be stamped is asked about on the next save.
    pub(crate) fn never_matching() -> Stamp {
        Stamp {
            len: u64::MAX,
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    pub(crate) fn of(path: &Path) -> io::Result<Stamp> {
        let metadata = std::fs::metadata(path)?;
        Ok(Stamp {
            len: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strings {
    Absent,
    // The file's shared strings part holds the model's first `n` strings in order.
    Aligned(usize),
}

// Whether the file holds the metadata part a dynamic array formula's cm="1" points into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Metadata {
    Written,
    Unknown,
}

// What the file still holds of a sheet's elements that only a save into that same file
// writes back (`sheet_elements`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Remains {
    // Everything the sheet was read with, and its relationships part.
    All,
    // Its elements, without the relationships part or anything pointing into it.
    Unlinked,
    // Nothing: a whole save wrote the sheet from the model alone.
    Nothing,
}

impl Remains {
    pub(crate) fn unlinked(self) -> Remains {
        match self {
            Remains::All | Remains::Unlinked => Remains::Unlinked,
            Remains::Nothing => Remains::Nothing,
        }
    }

    // What the file holds once the sheet is rewritten into it, keeping its relationships
    // part or not.
    pub(crate) fn rewritten(self, relationships: Relationships) -> Remains {
        match relationships {
            Relationships::Kept => self,
            Relationships::Dropped => self.unlinked(),
        }
    }
}

// What a save knows about the file it writes before the rename gives it a stamp.
#[derive(Clone, Debug)]
pub(crate) struct Contents {
    pub(crate) worksheets: Vec<String>,
    pub(crate) strings: Strings,
    pub(crate) styles: StylesBase,
    pub(crate) book: u64,
    pub(crate) sheets: Vec<SheetPrint>,
    // The sheet the file opens on. A save that copies the workbook part keeps it.
    pub(crate) selected: usize,
    pub(crate) metadata: Metadata,
    pub(crate) held: Held,
    // By position, as `worksheets`.
    pub(crate) remains: Vec<Remains>,
    // The losses `Carried` counted when the file was written: it lacks those, and only
    // later ones are still in it.
    pub(crate) losses_through: u64,
}

// The workbook's file on disk as it was last read or written, so a save can copy the
// parts that did not change since.
#[derive(Debug)]
pub(crate) struct SavedFile {
    pub(crate) path: PathBuf,
    pub(crate) stamp: Stamp,
    pub(crate) contents: Contents,
    // Worksheet parts a row-level save found it cannot splice, so the next save writes them
    // whole instead of finding out again after the work is done.
    pub(crate) unspliceable: Vec<String>,
}

impl SavedFile {
    // Only into a file of its own kind: the parts of an .xlsm are not those of an .xlsx.
    // Whether the file is still as it was is checked by the save, outside the lock.
    pub(crate) fn same_kind(&self, target: &Path) -> bool {
        let extension = |path: &Path| {
            path.extension()
                .map(|extension| extension.to_ascii_lowercase())
        };
        extension(&self.path) == extension(target)
    }
}

// The file a workbook was opened from or last saved to, whether or not a save can copy
// from it: a save to that path must find it as it was, or ask.
#[derive(Clone, Debug)]
pub(crate) struct KnownFile {
    pub(crate) path: PathBuf,
    pub(crate) stamp: Stamp,
}

// Two spellings of one file are one file; a path that does not resolve is only itself, so a
// Save As to a name that does not exist yet is never taken for the workbook's own file.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[derive(Default)]
struct Remembered {
    file: Option<Arc<SavedFile>>,
    known: Option<Arc<KnownFile>>,
}

#[derive(Clone, Default)]
pub(crate) struct SavedSlot(Arc<Mutex<Remembered>>);

impl SavedSlot {
    fn lock(&self) -> std::sync::MutexGuard<'_, Remembered> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn current(&self) -> Option<Arc<SavedFile>> {
        self.lock().file.clone()
    }

    // Only the file saves copy from; what is known of the path on disk stays.
    pub(crate) fn set(&self, file: Option<SavedFile>) {
        self.lock().file = file.map(Arc::new);
    }

    pub(crate) fn known(&self) -> Option<Arc<KnownFile>> {
        self.lock().known.clone()
    }

    pub(crate) fn set_known(&self, known: Option<KnownFile>) {
        self.lock().known = known.map(Arc::new);
    }

    // Keeps the file `base` for later saves, minus the splice of `part`. A slot that holds
    // another file by now is left alone.
    pub(crate) fn refuse_splice(&self, base: &Arc<SavedFile>, part: String) {
        let mut held = self.lock();
        if !held
            .file
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, base))
        {
            return;
        }
        let mut unspliceable = base.unspliceable.clone();
        unspliceable.push(part);
        held.file = Some(Arc::new(SavedFile {
            path: base.path.clone(),
            stamp: base.stamp,
            contents: base.contents.clone(),
            unspliceable,
        }));
    }
}

pub(crate) fn sheet_prints(
    data: ModelData<'_>,
    carried: &Carried,
) -> Result<Vec<SheetPrint>, EngineError> {
    let mut budget = ROW_PRINT_BUDGET_BYTES;
    data.book
        .worksheets
        .iter()
        .zip(data.formulas)
        .map(|(sheet, formulas)| {
            let moves = carried.moves(sheet.sheet_id);
            sheet_print(sheet, formulas, carried.written(sheet), moves, &mut budget)
        })
        .collect()
}

pub(crate) fn selected_sheet(book: &Workbook) -> usize {
    book.views.get(&0).map_or(0, |view| view.sheet as usize)
}

// Where an opened file's parts are and what they hold that Zenkai does not model.
pub(crate) struct OpenedParts {
    pub(crate) layout: PackageLayout,
    pub(crate) held: Held,
}

// A file just read by the streaming reader, unchanged on disk while it was read.
pub(crate) struct FileOnDisk<'a> {
    pub(crate) path: &'a Path,
    pub(crate) stamp: Stamp,
    pub(crate) parts: OpenedParts,
}

// When a save could copy from the file: its strings and cell formats line up with the
// model's, so a rewritten sheet's indexes mean the same there, and each part name is there once, so
// the part a save rewrites is the one a reader reads. `data` must still hold the values
// the file stores, before any recalculation.
pub(crate) fn opened(
    FileOnDisk {
        path,
        stamp,
        parts: OpenedParts { layout, held },
    }: FileOnDisk<'_>,
    data: ModelData<'_>,
    carried: &Carried,
) -> Option<SavedFile> {
    let strings = match layout.shared_strings {
        SharedStrings::Absent => Strings::Absent,
        SharedStrings::Aligned(count) => Strings::Aligned(count),
        SharedStrings::Unaligned => {
            tracing::info!("the shared strings do not line up; the first save writes everything");
            return None;
        }
    };
    let aligned = layout.worksheets.len() == data.book.worksheets.len()
        && layout.part_names == PartNames::Unique;
    if !aligned {
        tracing::info!("the sheet parts do not line up; the first save writes everything");
        return None;
    }
    let prints = StylesBase::opened(layout.styles, &data.book.styles).and_then(|styles| {
        let book = book_print(data.book)?;
        Ok((styles, book, sheet_prints(data, carried)?))
    });
    match prints {
        Ok((None, ..)) => {
            tracing::info!("the styles do not line up; the first save writes everything");
            None
        }
        Ok((Some(styles), book, sheets)) => Some(SavedFile {
            path: path.to_path_buf(),
            stamp,
            contents: Contents {
                remains: vec![Remains::All; layout.worksheets.len()],
                worksheets: layout.worksheets,
                strings,
                styles,
                book,
                sheets,
                selected: selected_sheet(data.book),
                metadata: Metadata::Unknown,
                held,
                losses_through: carried.losses(),
            },
            unspliceable: Vec::new(),
        }),
        Err(error) => {
            tracing::warn!(%error, "the first save writes the whole file");
            None
        }
    }
}
