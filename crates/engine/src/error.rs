use std::path::PathBuf;

use zenkai_types::SheetId;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the file is not a valid xlsx workbook: {0}")]
    InvalidFile(String),
    #[error("the file was refused because it could harm Zenkai: {0}")]
    Unsafe(String),
    #[error("the saved copy could not be reopened, the original was left untouched: {0}")]
    VerifyFailed(String),
    #[error("{} changed on disk while it was being saved; save again", .0.display())]
    ChangedOnDisk(PathBuf),
    #[error("{} could not be copied from ({reason}); save again to write it whole", .path.display())]
    BaseUnusable { path: PathBuf, reason: String },
    #[error("sheet {} does not exist", .0.0)]
    UnknownSheet(SheetId),
    #[error("{0}")]
    Rejected(String),
}
