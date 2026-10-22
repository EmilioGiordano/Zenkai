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
    #[error("{0}")]
    TooLarge(String),
    #[error("the saved copy could not be reopened, the original was left untouched: {0}")]
    VerifyFailed(String),
    #[error("{} changed on disk since it was opened or last saved; nothing was written over it", .0.display())]
    ChangedOnDisk(PathBuf),
    #[error("{} could not be copied from ({reason}); save again to write it whole", .path.display())]
    BaseUnusable { path: PathBuf, reason: String },
    #[error("sheet {} does not exist", .0.0)]
    UnknownSheet(SheetId),
    #[error("{0}")]
    Rejected(String),
}

impl EngineError {
    // A file that needs more memory than the budget allows is not a dangerous file.
    pub(crate) fn refused(reason: String) -> EngineError {
        if reason.starts_with(ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX) {
            EngineError::TooLarge(reason)
        } else {
            EngineError::Unsafe(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX;

    use super::*;

    #[test]
    fn a_file_over_the_memory_budget_is_too_large_not_unsafe() {
        let error = EngineError::refused(format!("{BUDGET_REFUSAL_PREFIX}12.3 GB to open"));
        assert!(matches!(error, EngineError::TooLarge(_)));
        assert!(!error.to_string().contains("harm"), "{error}");
    }

    #[test]
    fn any_other_refusal_stays_unsafe() {
        let error = EngineError::refused("a formula is nested 300 levels deep".to_string());
        assert!(matches!(error, EngineError::Unsafe(_)));
    }
}
