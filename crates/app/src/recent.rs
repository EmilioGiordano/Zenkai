use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 5;

fn file() -> Option<PathBuf> {
    Some(crate::recovery::directory()?.parent()?.join("recent.txt"))
}

/// The recently opened workbooks, newest first; a missing list is an empty one.
pub fn load() -> Vec<PathBuf> {
    let Some(file) = file() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&file) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            tracing::warn!(%error, "could not read the recent files list");
            Vec::new()
        }
    }
}

fn parse(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .take(MAX_RECENT)
        .collect()
}

/// `path` moved to the front, without duplicates.
pub fn with(recent: &[PathBuf], path: &Path) -> Vec<PathBuf> {
    std::iter::once(path.to_path_buf())
        .chain(recent.iter().filter(|p| p.as_path() != path).cloned())
        .take(MAX_RECENT)
        .collect()
}

// Written beside the list and renamed over it, so a crash never leaves half a list; a
// save overtaken by a newer one (`latest`) gives way.
pub fn save(
    recent: &[PathBuf],
    generation: u64,
    latest: &std::sync::atomic::AtomicU64,
) -> std::io::Result<()> {
    let Some(file) = file() else {
        return Ok(());
    };
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text: String = recent
        .iter()
        .map(|path| format!("{}\n", path.display()))
        .collect();
    let temp = file.with_extension(format!("{generation}.tmp"));
    // Check and rename under one lock, so an older save can never land after a newer one.
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _saving = SAVING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let written = std::fs::write(&temp, text).and_then(|()| {
        if latest.load(std::sync::atomic::Ordering::SeqCst) == generation {
            std::fs::rename(&temp, &file)
        } else {
            Ok(())
        }
    });
    if temp.exists()
        && let Err(error) = std::fs::remove_file(&temp)
    {
        tracing::debug!(%error, "could not remove a stale recent-list temp file");
    }
    written
}

pub fn label(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let folder = path
        .parent()
        .map_or_else(String::new, |p| p.display().to_string());
    format!("{name}  ({folder})")
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECENT, label, parse, with};
    use std::path::{Path, PathBuf};

    #[test]
    fn newest_first_without_duplicates_and_capped() {
        let mut recent: Vec<PathBuf> = Vec::new();
        for name in ["a", "b", "c", "a", "d", "e", "f"] {
            recent = with(&recent, Path::new(name));
        }
        let names: Vec<_> = recent
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["f", "e", "d", "a", "c"]);
        assert_eq!(recent.len(), MAX_RECENT);
    }

    #[test]
    fn parses_one_path_per_line() {
        assert_eq!(parse("C:\\a.xlsx\n\nD:\\b.csv\n").len(), 2);
    }

    #[test]
    fn opening_a_listed_file_moves_it_to_the_front() {
        let recent = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        assert_eq!(
            with(&recent, Path::new("c")),
            [PathBuf::from("c"), PathBuf::from("a"), PathBuf::from("b")]
        );
    }

    #[test]
    fn adding_to_an_empty_list_gives_just_that_file() {
        assert_eq!(with(&[], Path::new("a")), [PathBuf::from("a")]);
    }

    #[test]
    fn label_shows_the_file_name_then_its_folder() {
        let path = Path::new("reports").join("q1.xlsx");
        assert_eq!(label(&path), "q1.xlsx  (reports)");
    }

    #[test]
    fn label_of_a_bare_name_has_an_empty_folder() {
        assert_eq!(label(Path::new("q1.xlsx")), "q1.xlsx  ()");
    }
}
