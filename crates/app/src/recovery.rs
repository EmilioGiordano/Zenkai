use std::path::{Path, PathBuf};
use std::time::Duration;

use zenkai_engine::{EngineError, Workbook, save_xlsx_atomic};

pub const AUTOSAVE_EVERY: Duration = Duration::from_secs(60);
const PREFIX: &str = "autosave-";

pub fn directory() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("Zenkai").join("recovery"))
}

pub fn own_file(directory: &Path) -> PathBuf {
    directory.join(format!("{PREFIX}{}.xlsx", std::process::id()))
}

pub fn leftovers(directory: &Path) -> Vec<PathBuf> {
    let own = own_file(directory);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path != &own
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".xlsx"))
        })
        .collect();
    found.sort();
    found
}

pub fn write(workbook: &Workbook, path: &Path) -> Result<(), EngineError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EngineError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    save_xlsx_atomic(workbook, path)
}

pub fn remove(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(?path, %error, "could not remove a recovery file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_engine::Engine;
    use zenkai_types::{CellPos, SheetId};

    #[test]
    fn autosave_round_trips_and_leftovers_skip_our_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), CellPos::default(), "42")
            .unwrap();
        let own = own_file(dir.path());
        write(&book, &own).unwrap();
        write(&book, &own).unwrap();
        let other = dir.path().join("autosave-1.xlsx");
        std::fs::copy(&own, &other).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert_eq!(leftovers(dir.path()), vec![other.clone()]);
        let reopened = zenkai_engine::open_xlsx(&other).unwrap();
        assert_eq!(
            reopened.workbook.cell(SheetId(0), CellPos::default()).text,
            "42"
        );
        remove(&other);
        remove(&other);
        assert!(leftovers(dir.path()).is_empty());
    }
}
