use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;

use tempfile::TempDir;

use crate::folders::Folders;

const FILE_SHARE_READ: u32 = 1;

// A fake install on disk. Each version is a folder whose `zenkai.exe` holds its name as text.
pub struct Install {
    pub root: TempDir,
}

impl Install {
    pub fn new() -> Install {
        let install = Install {
            root: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(install.folders().update()).unwrap();
        install
    }

    pub fn folders(&self) -> Folders {
        Folders::at(self.root.path())
    }

    pub fn path(&self, relative: &str) -> std::path::PathBuf {
        self.root.path().join(relative)
    }

    pub fn put_version(&self, folder: &str, name: &str) {
        let folder = self.path(folder);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("zenkai.exe"), name).unwrap();
        fs::write(folder.join("pdfium.dll"), name).unwrap();
    }

    pub fn version_in(&self, folder: &str) -> Option<String> {
        fs::read_to_string(self.path(folder).join("zenkai.exe")).ok()
    }

    pub fn write(&self, relative: &str, text: &str) {
        fs::write(self.path(relative), text).unwrap();
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.path(relative).exists()
    }

    // Held open without write or delete sharing: the program cannot be opened for writing, as
    // when it runs, and the folder around it cannot be renamed.
    pub fn hold(&self, relative: &str) -> File {
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(self.path(relative))
            .unwrap()
    }

    pub fn rejected(&self) -> std::path::PathBuf {
        self.folders().rejected()
    }
}
