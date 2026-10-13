use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use ironcalc::base::types::Workbook;
use zenkai_xlsx_reader::{PackageLayout, SharedStrings};

use crate::error::EngineError;
use crate::fingerprint::{SheetPrint, book_print, sheet_print};
use crate::save::ModelData;
use crate::sheet_settings::Carried;

// A file is taken as unchanged on disk while its length and modification time are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: SystemTime,
}

impl Stamp {
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

// What a save knows about the file it writes before the rename gives it a stamp.
#[derive(Clone, Debug)]
pub(crate) struct Contents {
    pub(crate) worksheets: Vec<String>,
    pub(crate) strings: Strings,
    pub(crate) book: u64,
    pub(crate) sheets: Vec<SheetPrint>,
    // The sheet the file opens on. A save that copies the workbook part keeps it.
    pub(crate) selected: usize,
    pub(crate) metadata: Metadata,
}

// The workbook's file on disk as it was last read or written, so a save can copy the
// parts that did not change since.
#[derive(Debug)]
pub(crate) struct SavedFile {
    pub(crate) path: PathBuf,
    pub(crate) stamp: Stamp,
    pub(crate) contents: Contents,
}

impl SavedFile {
    // Only a file still as it was is copied from, and only into a file of its own kind:
    // the parts of an .xlsm are not those of an .xlsx.
    pub(crate) fn fits(&self, target: &Path) -> bool {
        let extension = |path: &Path| {
            path.extension()
                .map(|extension| extension.to_ascii_lowercase())
        };
        extension(&self.path) == extension(target)
            && Stamp::of(&self.path).is_ok_and(|stamp| stamp == self.stamp)
    }
}

#[derive(Clone, Default)]
pub(crate) struct SavedSlot(Arc<Mutex<Option<Arc<SavedFile>>>>);

impl SavedSlot {
    pub(crate) fn current(&self) -> Option<Arc<SavedFile>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set(&self, file: Option<SavedFile>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = file.map(Arc::new);
    }
}

pub(crate) fn sheet_prints(
    data: ModelData<'_>,
    carried: &Carried,
) -> Result<Vec<SheetPrint>, EngineError> {
    data.book
        .worksheets
        .iter()
        .zip(data.formulas)
        .map(|(sheet, formulas)| sheet_print(sheet, formulas, carried.written(sheet)))
        .collect()
}

pub(crate) fn selected_sheet(book: &Workbook) -> usize {
    book.views.get(&0).map_or(0, |view| view.sheet as usize)
}

// A file just read by the streaming reader, unchanged on disk while it was read.
pub(crate) struct FileOnDisk<'a> {
    pub(crate) path: &'a Path,
    pub(crate) stamp: Stamp,
    pub(crate) layout: PackageLayout,
}

// When a save could copy from the file: its strings and styles line up with the model's,
// so a rewritten sheet's indexes mean the same there. `data` must still hold the values the
// file stores, before any recalculation.
pub(crate) fn opened(
    FileOnDisk {
        path,
        stamp,
        layout,
    }: FileOnDisk<'_>,
    data: ModelData<'_>,
    carried: &Carried,
) -> Option<SavedFile> {
    let styles = &data.book.styles;
    let strings = match layout.shared_strings {
        SharedStrings::Absent => Strings::Absent,
        SharedStrings::Aligned(count) => Strings::Aligned(count),
        SharedStrings::Unaligned => return None,
    };
    let aligned = layout.styles.cell_xfs == styles.cell_xfs.len()
        && layout.styles.dxfs == styles.dxfs.len()
        && layout.worksheets.len() == data.book.worksheets.len();
    if !aligned {
        return None;
    }
    let prints = book_print(data.book).and_then(|book| Ok((book, sheet_prints(data, carried)?)));
    match prints {
        Ok((book, sheets)) => Some(SavedFile {
            path: path.to_path_buf(),
            stamp,
            contents: Contents {
                worksheets: layout.worksheets,
                strings,
                book,
                sheets,
                selected: selected_sheet(data.book),
                metadata: Metadata::Unknown,
            },
        }),
        Err(error) => {
            tracing::warn!(%error, "the first save writes the whole file");
            None
        }
    }
}
