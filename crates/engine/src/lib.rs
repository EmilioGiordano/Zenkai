#![forbid(unsafe_code)]

mod error;
mod file;
mod model;
mod preflight;
mod sheet_patch;
mod sheet_settings;
mod workbook;

pub use error::EngineError;
pub use file::{Opened, Unsupported, open_xlsx, save_xlsx_atomic, write_atomic};
pub use preflight::run_with_engine_stack;
pub use workbook::{Copied, Engine, Workbook, format_preview};
