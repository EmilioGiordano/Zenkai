#![forbid(unsafe_code)]

mod carried_moves;
#[cfg(test)]
mod content_cells_property;
mod edit_profile;
mod error;
mod file;
mod fingerprint;
mod held;
mod hibernation;
mod model;
mod preflight;
mod rectangles;
mod row_split;
mod save;
mod save_parts;
mod save_patch;
mod save_rows;
mod save_sheet;
mod saved;
mod sheet_copy;
mod sheet_elements;
mod sheet_settings;
mod style_growth;
mod table_headers;
#[cfg(test)]
mod used_end_property;
mod workbook;
mod xlsx_read;

pub use edit_profile::{EditProfile, RecalcMode};
pub use error::EngineError;
pub use file::{Opened, Unsupported, open_xlsx, open_xlsx_with, scan_unsupported, write_atomic};
pub use preflight::run_with_engine_stack;
pub use save::{SaveSnapshot, SaveTarget, save_xlsx, write_xlsx_unverified};
pub use saved::Stamp;
pub use workbook::{Copied, Engine, Workbook, format_preview};
pub use xlsx_read::{
    ListedSheet, ReadBook, ReaderComparison, XlsxReader, compare_readers, read_xlsx_bytes,
    sheet_names_of,
};
