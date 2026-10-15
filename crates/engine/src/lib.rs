#![forbid(unsafe_code)]

mod error;
mod file;
mod fingerprint;
mod held;
mod model;
mod preflight;
mod rectangles;
mod row_split;
mod save;
mod save_patch;
mod save_sheet;
mod saved;
mod sheet_copy;
mod sheet_settings;
#[cfg(test)]
mod used_end_property;
mod workbook;
mod xlsx_read;

pub use error::EngineError;
pub use file::{Opened, Unsupported, open_xlsx, open_xlsx_with, scan_unsupported, write_atomic};
pub use preflight::run_with_engine_stack;
pub use save::{SaveSnapshot, SaveTarget, save_xlsx};
pub use workbook::{Copied, Engine, Workbook, format_preview};
pub use xlsx_read::{ReadBook, ReaderComparison, XlsxReader, compare_readers, read_xlsx_bytes};
