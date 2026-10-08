use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::settings::{SCHEMA_FILE, Settings, SettingsError};

pub const SETTINGS_FILE: &str = "settings.json";
const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum SettingsFileError {
    #[error("no folder for the settings: neither APPDATA nor a home folder is set")]
    NoFolder,
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not watch {path} for changes: {source}")]
    Watch {
        path: PathBuf,
        source: notify::Error,
    },
    #[error("could not start the settings watcher thread: {0}")]
    Thread(std::io::Error),
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsPaths {
    pub folder: PathBuf,
}

impl SettingsPaths {
    // Roaming user configuration on Windows, as Zed and VS Code keep theirs; caches and
    // state stay in LOCALAPPDATA.
    pub fn from_environment() -> Result<SettingsPaths, SettingsFileError> {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or(SettingsFileError::NoFolder)?;
        let folder = if cfg!(windows) {
            base.join("Zenkai")
        } else {
            base.join("zenkai")
        };
        Ok(SettingsPaths { folder })
    }

    pub fn settings(&self) -> PathBuf {
        self.folder.join(SETTINGS_FILE)
    }

    pub fn schema(&self) -> PathBuf {
        self.folder.join(SCHEMA_FILE)
    }
}

pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Settings::parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(SettingsError::Read(error.to_string())),
    }
}

// The schema is rewritten on every start so it always matches this build; the settings
// file is created with defaults only when missing, never overwritten.
pub fn prepare(paths: &SettingsPaths) -> Result<Settings, SettingsFileError> {
    std::fs::create_dir_all(&paths.folder).map_err(|source| SettingsFileError::Write {
        path: paths.folder.clone(),
        source,
    })?;
    write_atomic(&paths.schema(), &Settings::schema_json()?)?;
    let file = paths.settings();
    if !file.exists() {
        save(paths, &Settings::default())?;
    }
    Ok(load(&file)?)
}

pub fn save(paths: &SettingsPaths, settings: &Settings) -> Result<(), SettingsFileError> {
    write_atomic(&paths.settings(), &settings.to_json()?)
}

// Written beside the target and renamed over it, so a reader (Zenkai's own watcher or
// an agent) never sees half a file.
fn write_atomic(path: &Path, text: &str) -> Result<(), SettingsFileError> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let written = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path));
    if written.is_err()
        && temp.exists()
        && let Err(error) = std::fs::remove_file(&temp)
    {
        tracing::debug!(%error, "could not remove a settings temp file");
    }
    written.map_err(|source| SettingsFileError::Write {
        path: path.to_path_buf(),
        source,
    })
}

// Dropping the watcher stops it; the debounce thread ends with it.
pub struct SettingsWatcher {
    _watcher: notify::RecommendedWatcher,
}

// The folder is watched, not the file: editors and agents save by writing a temporary
// file and renaming it over settings.json, which would end a watch on the file itself.
pub fn watch(
    paths: &SettingsPaths,
    on_change: impl Fn(Result<Settings, SettingsError>) + Send + 'static,
) -> Result<SettingsWatcher, SettingsFileError> {
    let (changed, changes) = mpsc::channel::<()>();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) if touches_settings(&event) => {
                if changed.send(()).is_err() {
                    tracing::debug!("settings debounce thread already stopped");
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "settings watcher error"),
        })
        .map_err(|source| SettingsFileError::Watch {
            path: paths.folder.clone(),
            source,
        })?;
    watcher
        .watch(&paths.folder, RecursiveMode::NonRecursive)
        .map_err(|source| SettingsFileError::Watch {
            path: paths.folder.clone(),
            source,
        })?;
    let file = paths.settings();
    std::thread::Builder::new()
        .name("settings-watch".to_string())
        .spawn(move || {
            while changes.recv().is_ok() {
                while changes.recv_timeout(DEBOUNCE).is_ok() {}
                on_change(load(&file));
            }
        })
        .map_err(SettingsFileError::Thread)?;
    Ok(SettingsWatcher { _watcher: watcher })
}

fn touches_settings(event: &notify::Event) -> bool {
    !event.kind.is_access()
        && event
            .paths
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PermissionMode;

    fn paths(dir: &tempfile::TempDir) -> SettingsPaths {
        SettingsPaths {
            folder: dir.path().join("Zenkai"),
        }
    }

    #[test]
    fn first_start_writes_defaults_and_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        let settings = prepare(&paths).unwrap();
        assert_eq!(settings, Settings::default());
        let schema = std::fs::read_to_string(paths.schema()).unwrap();
        assert_eq!(schema, Settings::schema_json().unwrap());
        assert_eq!(load(&paths.settings()).unwrap(), Settings::default());
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        std::fs::create_dir_all(&paths.folder).unwrap();
        std::fs::write(paths.settings(), "{ broken").unwrap();
        assert!(prepare(&paths).is_err());
        assert_eq!(
            std::fs::read_to_string(paths.settings()).unwrap(),
            "{ broken"
        );
    }

    #[test]
    fn a_missing_file_reads_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("none.json")).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn saved_settings_load_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        prepare(&paths).unwrap();
        let mut settings = Settings::default();
        settings.agents.permission = PermissionMode::ReadOnly;
        save(&paths, &settings).unwrap();
        assert_eq!(load(&paths.settings()).unwrap(), settings);
        let leftovers = std::fs::read_dir(&paths.folder)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn an_edit_on_disk_is_reported_once_settled() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        prepare(&paths).unwrap();
        let (sender, received) = mpsc::channel();
        let _watcher = watch(&paths, move |loaded| sender.send(loaded).unwrap()).unwrap();
        std::fs::write(paths.folder.join("other.json"), "{ \"not\": \"settings\" }").unwrap();
        std::fs::write(
            paths.settings(),
            "{ \"agents\": { \"permission\": \"automatic\" } }",
        )
        .unwrap();
        let loaded = received
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.agents.permission, PermissionMode::Automatic);
        std::fs::write(paths.settings(), "{ \"agents\": ").unwrap();
        let broken = loop {
            let next = received.recv_timeout(Duration::from_secs(10)).unwrap();
            if next.is_err() {
                break next;
            }
        };
        assert!(matches!(broken, Err(SettingsError::Parse { .. })));
    }
}
