use std::path::{Path, PathBuf};

use zenkai_agent::tools::WorkingFolder;
use zenkai_i18n::t;

use crate::files;

const DEFAULT_FOLDER_NAME: &str = "Zenkai";

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
}
