#![forbid(unsafe_code)]

mod error;
mod file;
mod workbook;

pub use error::EngineError;
pub use file::{Opened, Unsupported, open_xlsx, save_xlsx_atomic, scan_unsupported};
pub use workbook::{Engine, Workbook};
