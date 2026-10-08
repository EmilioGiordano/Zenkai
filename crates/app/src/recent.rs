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

// Written beside the list and renamed over it, so a crash never leaves half a list.
pub fn save(recent: &[PathBuf]) -> std::io::Result<()> {
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
    let temp = file.with_extension("txt.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, &file)
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
    use super::{MAX_RECENT, parse, with};
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
}
