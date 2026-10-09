#![forbid(unsafe_code)]

mod empty_rows;
mod error;
mod file;
mod model;
mod preflight;
mod workbook;
mod xlsx_read;

pub use error::EngineError;
pub use file::{
    Opened, Unsupported, open_xlsx, open_xlsx_with, save_xlsx_atomic, scan_unsupported,
    write_atomic,
};
pub use preflight::run_with_engine_stack;
pub use workbook::{Copied, Engine, Workbook, format_preview};
pub use xlsx_read::{ReadBook, ReaderComparison, XlsxReader, compare_readers, read_xlsx_bytes};
