#![forbid(unsafe_code)]

mod error;
mod file;
mod model;
mod preflight;
mod rectangles;
mod sheet_patch;
mod sheet_settings;
#[cfg(test)]
mod used_end_property;
mod workbook;

pub use error::EngineError;
pub use file::{
    Opened, Unsupported, open_xlsx, save_xlsx_atomic, scan_unsupported, write_atomic, xlsx_bytes,
};
pub use preflight::run_with_engine_stack;
pub use workbook::{Copied, Engine, Workbook, format_preview};
