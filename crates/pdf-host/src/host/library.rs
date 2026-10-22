use std::path::{Path, PathBuf};

use pdfium_render::prelude::Pdfium;

use crate::wire::OpenError;

const LIBRARY_FILE: &str = "pdfium.dll";

pub fn library_path(directory: &Path) -> PathBuf {
    directory.join(LIBRARY_FILE)
}

// Only the folder of the running exe is searched, never the system path or the working
// directory, so a planted pdfium.dll elsewhere is never loaded.
pub fn bind_next_to_exe() -> Result<Pdfium, OpenError> {
    let exe =
        std::env::current_exe().map_err(|error| OpenError::LibraryFailed(error.to_string()))?;
    let directory = exe.parent().ok_or(OpenError::LibraryMissing)?;
    bind_in(directory)
}

pub fn bind_in(directory: &Path) -> Result<Pdfium, OpenError> {
    let path = library_path(directory);
    if !path.is_file() {
        return Err(OpenError::LibraryMissing);
    }
    Pdfium::bind_to_library(&path)
        .map(Pdfium::new)
        .map_err(|error| OpenError::LibraryFailed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_is_looked_up_by_the_folder_it_is_given() {
        let path = library_path(Path::new("C:/app"));
        assert_eq!(path, Path::new("C:/app").join("pdfium.dll"));
    }

    #[test]
    fn a_folder_without_the_library_reports_it_missing() {
        let folder = tempfile::tempdir().unwrap();
        assert!(matches!(
            bind_in(folder.path()),
            Err(OpenError::LibraryMissing)
        ));
    }

    #[test]
    fn a_file_that_is_not_a_library_fails_to_load_without_panicking() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(library_path(folder.path()), b"not a dll").unwrap();
        assert!(matches!(
            bind_in(folder.path()),
            Err(OpenError::LibraryFailed(_))
        ));
    }
}
