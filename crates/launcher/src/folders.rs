use std::io;
use std::path::PathBuf;

use crate::arguments::Version;

const APP: &str = "app";
const OLD: &str = "app.old";
const UPDATE: &str = ".update";
const PROGRAM: &str = "zenkai.exe";
const LOCK: &str = "lock";
const CONFIRMED: &str = "confirmed";
const NOTICE: &str = "notice";
const JOURNAL: &str = "launcher.log";

// Every path the launcher touches comes from its own folder, never from an argument.
pub struct Folders {
    root: PathBuf,
}

impl Folders {
    pub fn of_this_launcher() -> io::Result<Folders> {
        let exe = std::env::current_exe()?;
        let root = exe
            .parent()
            .ok_or_else(|| io::Error::other("the launcher is not inside a folder"))?;
        Ok(Folders {
            root: root.to_path_buf(),
        })
    }

    #[cfg(test)]
    pub fn at(root: &std::path::Path) -> Folders {
        Folders {
            root: root.to_path_buf(),
        }
    }

    pub fn app(&self) -> PathBuf {
        self.root.join(APP)
    }

    pub fn program(&self) -> PathBuf {
        self.app().join(PROGRAM)
    }

    pub fn old(&self) -> PathBuf {
        self.root.join(OLD)
    }

    pub fn update(&self) -> PathBuf {
        self.root.join(UPDATE)
    }

    pub fn lock(&self) -> PathBuf {
        self.update().join(LOCK)
    }

    pub fn confirmed(&self) -> PathBuf {
        self.update().join(CONFIRMED)
    }

    pub fn notice(&self) -> PathBuf {
        self.update().join(NOTICE)
    }

    pub fn journal(&self) -> PathBuf {
        self.update().join(JOURNAL)
    }

    pub fn new_app(&self, version: &Version) -> PathBuf {
        self.update().join(format!("new-{version}"))
    }

    pub fn new_program(&self, version: &Version) -> PathBuf {
        self.new_app(version).join(PROGRAM)
    }

    pub fn rejected(&self) -> PathBuf {
        self.update()
            .join(format!("rejected-{}", std::process::id()))
    }
}
