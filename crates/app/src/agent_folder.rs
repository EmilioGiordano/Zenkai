use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zenkai_agent::tools::WorkingFolder;
use zenkai_i18n::t;

use crate::files;

const DEFAULT_FOLDER_NAME: &str = "Zenkai";
const LISTED_EXTENSIONS: [&str; 7] = ["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"];
const SCAN_DEPTH: usize = 3;
const SCAN_ENTRIES: usize = 5_000;

#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error("{}", t!("chat.error.no_documents_folder"))]
    NoDocumentsFolder,
    #[error("{}", t!("chat.error.folder", path = .path.display(), error = .source))]
    Unusable {
        path: PathBuf,
        source: std::io::Error,
    },
}

// What the user is looking at when a conversation starts, read on the UI thread without I/O.
#[derive(Clone, Debug, Default)]
pub struct FolderHint {
    pub space_files: Vec<PathBuf>,
    pub active_file: Option<PathBuf>,
}

// A space has no folder of its own, so it is the folder its saved workbooks share; when they
// are spread out, the folder of the workbook on screen.
pub fn choose(hint: &FolderHint) -> Option<PathBuf> {
    shared_parent(&hint.space_files).or_else(|| {
        hint.active_file
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
    })
}

fn shared_parent(files: &[PathBuf]) -> Option<PathBuf> {
    let mut parents = files.iter().filter_map(|file| file.parent());
    let first = parents.next()?;
    parents
        .all(|parent| files::same_path(parent, first))
        .then(|| first.to_path_buf())
}

pub fn default_folder() -> Option<PathBuf> {
    dirs::document_dir().map(|documents| documents.join(DEFAULT_FOLDER_NAME))
}

// Runs off the UI thread. The default folder is created when missing and, like every
// working folder, never removed.
pub fn prepare(hint: &FolderHint) -> Result<WorkingFolder, FolderError> {
    let folder = match choose(hint).filter(|folder| folder.is_dir()) {
        Some(folder) => folder,
        None => {
            let folder = default_folder().ok_or(FolderError::NoDocumentsFolder)?;
            std::fs::create_dir_all(&folder).map_err(|source| FolderError::Unusable {
                path: folder.clone(),
                source,
            })?;
            folder
        }
    };
    WorkingFolder::new(&folder).map_err(|source| FolderError::Unusable {
        path: folder,
        source,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundFile {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub size: u64,
}

// Spreadsheets written in the folder since `since`, a few levels deep and a bounded number of
// entries, so a huge folder cannot stall the scan. Links and junctions are not followed, and
// Excel's "~$" lock and temp files are skipped. Runs off the UI thread.
pub fn spreadsheets_written_since(folder: &Path, since: SystemTime) -> Vec<FoundFile> {
    let mut found = Vec::new();
    let mut pending = vec![(folder.to_path_buf(), 0)];
    let mut visited = 0;
    while let Some((directory, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::debug!(%error, folder = %directory.display(), "could not list a folder");
                continue;
            }
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > SCAN_ENTRIES {
                return found;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth + 1 < SCAN_DEPTH {
                pending.push((path, depth + 1));
            } else if kind.is_file()
                && is_listed_spreadsheet(&path)
                && let Ok(metadata) = entry.metadata()
                && metadata.modified().is_ok_and(|modified| modified >= since)
                && let Ok(relative) = path.strip_prefix(folder)
            {
                found.push(FoundFile {
                    relative: relative.to_path_buf(),
                    size: metadata.len(),
                    path,
                });
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn is_listed_spreadsheet(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy());
    let lock_or_temp = name.as_deref().is_some_and(|name| name.starts_with("~$"));
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    !lock_or_temp
        && extension
            .as_deref()
            .is_some_and(|extension| LISTED_EXTENSIONS.contains(&extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(space: &[&str], active: Option<&str>) -> FolderHint {
        FolderHint {
            space_files: space.iter().map(PathBuf::from).collect(),
            active_file: active.map(PathBuf::from),
        }
    }

    #[test]
    fn a_space_whose_workbooks_share_a_folder_works_there() {
        let chosen = choose(&hint(
            &["C:/Q3/ventas.xlsx", "C:/Q3/gastos.xlsx"],
            Some("C:/Q3/ventas.xlsx"),
        ));
        assert_eq!(chosen, Some(PathBuf::from("C:/Q3")));
    }

    #[test]
    fn the_space_folder_wins_while_an_untitled_workbook_is_on_screen() {
        let chosen = choose(&hint(&["C:/Q3/ventas.xlsx"], None));
        assert_eq!(chosen, Some(PathBuf::from("C:/Q3")));
    }

    #[test]
    fn a_spread_out_space_falls_back_to_the_file_on_screen() {
        let chosen = choose(&hint(
            &["C:/Q3/ventas.xlsx", "D:/Other/gastos.xlsx"],
            Some("D:/Other/gastos.xlsx"),
        ));
        assert_eq!(chosen, Some(PathBuf::from("D:/Other")));
    }

    #[test]
    fn nothing_saved_means_no_choice_and_the_default_folder() {
        assert_eq!(choose(&hint(&[], None)), None);
        assert_eq!(choose(&hint(&["C:/A/x.xlsx", "C:/B/y.xlsx"], None)), None);
        let default = default_folder().unwrap();
        assert!(default.ends_with(DEFAULT_FOLDER_NAME));
    }

    #[test]
    fn an_existing_chosen_folder_is_used_as_it_is() {
        let temp = tempfile::tempdir().unwrap();
        let prepared = prepare(&FolderHint {
            space_files: vec![temp.path().join("book.xlsx")],
            active_file: None,
        })
        .unwrap();
        assert_eq!(prepared.path(), temp.path());
    }

    #[test]
    fn only_spreadsheets_written_during_the_turn_are_found() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old.xlsx");
        std::fs::write(&old, b"x").unwrap();
        let since = SystemTime::now();
        let file = |name: &str| {
            std::fs::File::options()
                .write(true)
                .open(temp.path().join(name))
                .unwrap()
        };
        file("old.xlsx")
            .set_modified(since - std::time::Duration::from_secs(60))
            .unwrap();
        std::fs::create_dir_all(temp.path().join("reports")).unwrap();
        for name in [
            "budget.xlsx",
            "reports/data.csv",
            "notes.txt",
            "~$budget.xlsx",
            "build.py",
        ] {
            std::fs::write(temp.path().join(name), b"x").unwrap();
            file(name)
                .set_modified(since + std::time::Duration::from_secs(1))
                .unwrap();
        }
        let found = spreadsheets_written_since(temp.path(), since);
        let relative: Vec<PathBuf> = found.iter().map(|file| file.relative.clone()).collect();
        assert_eq!(
            relative,
            [
                PathBuf::from("budget.xlsx"),
                Path::new("reports").join("data.csv")
            ]
        );
    }
}
