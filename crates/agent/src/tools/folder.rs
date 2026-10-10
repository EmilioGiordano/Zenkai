use std::io;
use std::path::{Path, PathBuf};

const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];
const FORBIDDEN_CHARACTERS: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("the path is empty")]
    Empty,
    #[error(
        "\"{0}\" is not relative to the working folder; give a path such as \"budget.xlsx\" or \"reports/budget.xlsx\""
    )]
    NotRelative(String),
    #[error("\"{0}\" leaves the working folder, which is not allowed")]
    Escapes(String),
    #[error("\"{0}\" is not a valid file or folder name on Windows")]
    BadName(String),
    #[error("could not check \"{path}\": {message}")]
    Unreadable { path: String, message: String },
}

// The folder the chat agent works in. `shown` is the path as the user knows it; `canonical`
// is what containment is checked against, so a junction or symlink cannot hide an escape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingFolder {
    shown: PathBuf,
    canonical: PathBuf,
}

impl WorkingFolder {
    pub fn new(path: &Path) -> io::Result<WorkingFolder> {
        let canonical = path.canonicalize()?;
        if !canonical.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a folder", path.display()),
            ));
        }
        Ok(WorkingFolder {
            shown: path.to_path_buf(),
            canonical,
        })
    }

    pub fn path(&self) -> &Path {
        &self.shown
    }

    pub fn resolve(&self, requested: &str) -> Result<InsidePath, PathError> {
        let relative = relative_path(requested)?;
        let path = self.shown.join(&relative);
        self.check_inside(&path, requested)?;
        Ok(InsidePath { path, relative })
    }

    // Creates the missing folders of `inside` and checks again, so a folder swapped for a
    // junction between the first check and the creation is still caught.
    pub fn create_parents(&self, inside: &InsidePath) -> Result<(), PathError> {
        let shown = inside.relative.display().to_string();
        if let Some(parent) = inside.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| PathError::Unreadable {
                path: shown.clone(),
                message: error.to_string(),
            })?;
        }
        self.check_inside(&inside.path, &shown)
    }

    fn check_inside(&self, path: &Path, requested: &str) -> Result<(), PathError> {
        let mut existing = path;
        while std::fs::symlink_metadata(existing).is_err() {
            existing = match existing.parent() {
                Some(parent) => parent,
                None => return Err(PathError::Escapes(requested.to_string())),
            };
        }
        let real = existing
            .canonicalize()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => PathError::Escapes(requested.to_string()),
                _ => PathError::Unreadable {
                    path: requested.to_string(),
                    message: error.to_string(),
                },
            })?;
        if real.starts_with(&self.canonical) {
            Ok(())
        } else {
            Err(PathError::Escapes(requested.to_string()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsidePath {
    path: PathBuf,
    relative: PathBuf,
}

impl InsidePath {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn relative(&self) -> &Path {
        &self.relative
    }

    pub fn extension(&self) -> Option<String> {
        self.path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
    }

    pub fn with_extension(&self, extension: &str) -> InsidePath {
        let add = |path: &Path| {
            let mut name = path.as_os_str().to_owned();
            name.push(".");
            name.push(extension);
            PathBuf::from(name)
        };
        InsidePath {
            path: add(&self.path),
            relative: add(&self.relative),
        }
    }
}

// Both separators split on every platform, so a test on Linux checks what Windows would do.
fn relative_path(requested: &str) -> Result<PathBuf, PathError> {
    if requested.is_empty() {
        return Err(PathError::Empty);
    }
    let not_relative = || PathError::NotRelative(requested.to_string());
    if requested.starts_with(['/', '\\']) {
        return Err(not_relative());
    }
    let mut chars = requested.chars();
    if let (Some(drive), Some(':')) = (chars.next(), chars.next())
        && drive.is_ascii_alphabetic()
    {
        return Err(not_relative());
    }
    let mut relative = PathBuf::new();
    for part in requested.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return Err(PathError::Escapes(requested.to_string())),
            name => {
                check_name(name).map_err(|()| PathError::BadName(name.to_string()))?;
                relative.push(name);
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(PathError::Empty);
    }
    Ok(relative)
}

// Windows drops trailing dots and spaces and maps device names to devices, so either would
// make the file land somewhere other than the name says.
fn check_name(name: &str) -> Result<(), ()> {
    let bad_character = name
        .chars()
        .any(|c| c.is_control() || FORBIDDEN_CHARACTERS.contains(&c));
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let reserved = RESERVED_NAMES.contains(&stem.to_ascii_lowercase().as_str());
    if bad_character || reserved || name.ends_with(['.', ' ']) {
        Err(())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> (tempfile::TempDir, WorkingFolder) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("space");
        std::fs::create_dir(&root).unwrap();
        let folder = WorkingFolder::new(&root).unwrap();
        (temp, folder)
    }

    #[test]
    fn a_relative_name_lands_inside_the_folder() {
        let (_temp, folder) = folder();
        let inside = folder.resolve("budget.xlsx").unwrap();
        assert_eq!(inside.path(), folder.path().join("budget.xlsx"));
        let nested = folder.resolve("reports\\2026/./sept.xlsx").unwrap();
        assert_eq!(
            nested.relative(),
            Path::new("reports").join("2026").join("sept.xlsx")
        );
    }

    #[test]
    fn parent_steps_are_refused_even_when_they_come_back() {
        let (_temp, folder) = folder();
        for path in [
            "../x.xlsx",
            "a/../../x.xlsx",
            "a/../x.xlsx",
            "..\\space\\x.xlsx",
        ] {
            assert!(
                matches!(folder.resolve(path), Err(PathError::Escapes(_))),
                "{path}"
            );
        }
    }

    #[test]
    fn absolute_unc_device_and_other_drive_paths_are_refused() {
        let (_temp, folder) = folder();
        let absolute = folder.path().join("x.xlsx").display().to_string();
        for path in [
            absolute.as_str(),
            "/etc/x.xlsx",
            "\\Windows\\x.xlsx",
            "C:\\Users\\me\\Documents\\x.xlsx",
            "D:x.xlsx",
            "d:/x.xlsx",
            "\\\\server\\share\\x.xlsx",
            "//server/share/x.xlsx",
            "\\\\?\\C:\\x.xlsx",
            "\\\\.\\PhysicalDrive0",
        ] {
            assert!(
                matches!(folder.resolve(path), Err(PathError::NotRelative(_))),
                "{path}"
            );
        }
    }

    #[test]
    fn names_windows_would_change_or_treat_as_devices_are_refused() {
        let (_temp, folder) = folder();
        for path in [
            "x.xlsx:stream",
            "CON",
            "nul.xlsx",
            "reports/COM1.txt",
            "Lpt9 .xlsx",
            "x.xlsx.",
            "x.xlsx ",
            "what?.xlsx",
            "a*b.xlsx",
            "tab\t.xlsx",
        ] {
            assert!(
                matches!(folder.resolve(path), Err(PathError::BadName(_))),
                "{path}"
            );
        }
        assert!(folder.resolve("console.xlsx").is_ok());
    }

    #[test]
    fn an_empty_path_is_refused() {
        let (_temp, folder) = folder();
        for path in ["", ".", "./", "a/.."] {
            assert!(folder.resolve(path).is_err(), "{path}");
        }
    }

    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[test]
    #[cfg(windows)]
    fn a_junction_that_points_outside_is_an_escape() {
        let (temp, folder) = folder();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        assert!(junction(&folder.path().join("door"), &outside));
        assert!(matches!(
            folder.resolve("door/x.xlsx"),
            Err(PathError::Escapes(_))
        ));
        assert!(matches!(
            folder.resolve("door/new/deeper/x.xlsx"),
            Err(PathError::Escapes(_))
        ));
    }

    #[test]
    #[cfg(windows)]
    fn a_junction_that_stays_inside_is_allowed() {
        let (_temp, folder) = folder();
        let real = folder.path().join("real");
        std::fs::create_dir(&real).unwrap();
        assert!(junction(&folder.path().join("alias"), &real));
        assert!(folder.resolve("alias/x.xlsx").is_ok());
    }

    #[test]
    fn a_symlink_to_a_file_outside_is_an_escape() {
        let (temp, folder) = folder();
        let target = temp.path().join("secret.xlsx");
        std::fs::write(&target, b"x").unwrap();
        let link = folder.path().join("link.xlsx");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link);
        if made.is_err() {
            // Windows needs Developer Mode or elevation to create symlinks.
            return;
        }
        assert!(matches!(
            folder.resolve("link.xlsx"),
            Err(PathError::Escapes(_))
        ));
    }

    #[test]
    #[cfg(windows)]
    fn a_folder_replaced_by_a_junction_after_the_check_is_caught_on_creation() {
        let (temp, folder) = folder();
        let inside = folder.resolve("later/x.xlsx").unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        assert!(junction(&folder.path().join("later"), &outside));
        assert!(matches!(
            folder.create_parents(&inside),
            Err(PathError::Escapes(_))
        ));
    }

    #[test]
    fn missing_folders_are_created_inside() {
        let (_temp, folder) = folder();
        let inside = folder.resolve("a/b/x.xlsx").unwrap();
        folder.create_parents(&inside).unwrap();
        assert!(folder.path().join("a").join("b").is_dir());
    }

    #[test]
    fn an_extension_is_added_to_both_spellings() {
        let (_temp, folder) = folder();
        let inside = folder
            .resolve("reports/budget")
            .unwrap()
            .with_extension("xlsx");
        assert_eq!(inside.relative(), Path::new("reports").join("budget.xlsx"));
        assert_eq!(inside.extension().as_deref(), Some("xlsx"));
    }
}
