use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::arguments::Version;
use crate::folders::{Folders, kept_by_cleanup};

pub const ERROR_SHARING_VIOLATION: i32 = 32;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const MAX_MARKER_BYTES: u64 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    Yes,
    No,
    Unknown,
}

pub fn presence(path: &Path) -> Presence {
    match fs::symlink_metadata(path) {
        Ok(_) => Presence::Present,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Presence::Absent,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not tell whether it exists");
            Presence::Unknown
        }
    }
}

// Windows refuses to open the image of a running program for writing, with a sharing violation.
// Any other failure says nothing either way.
pub fn running(program: &Path) -> Running {
    match OpenOptions::new().write(true).open(program) {
        Ok(_) => Running::No,
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Running::Yes,
        Err(error) => {
            tracing::warn!(%error, "could not tell whether the app is running");
            Running::Unknown
        }
    }
}

// A marker that is not a version, or is too long to be one, does not count.
pub fn marker(folders: &Folders) -> Presence {
    match read_marker(folders) {
        Ok(Some(text)) if text_version(&text).is_some() => Presence::Present,
        Ok(_) => Presence::Absent,
        Err(error) => {
            tracing::warn!(%error, "could not read the confirmation marker");
            Presence::Unknown
        }
    }
}

pub fn confirms(folders: &Folders, version: &Version) -> bool {
    match read_marker(folders) {
        Ok(text) => text.as_deref() == Some(version.as_bytes()),
        Err(error) => {
            tracing::debug!(%error, "could not read the confirmation marker");
            false
        }
    }
}

fn text_version(bytes: &[u8]) -> Option<Version> {
    Version::parse(std::str::from_utf8(bytes).ok()?)
}

fn read_marker(folders: &Folders) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(folders.confirmed()) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_MARKER_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

pub fn remove_marker(folders: &Folders) -> io::Result<()> {
    match fs::remove_file(folders.confirmed()) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

#[derive(Debug)]
pub enum Unusable {
    NotAPlainFolder,
    NoPlainProgram,
    Unreadable(io::Error),
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unusable::NotAPlainFolder => f.write_str("it is not a plain folder"),
            Unusable::NoPlainProgram => f.write_str("it has no plain zenkai.exe"),
            Unusable::Unreadable(error) => write!(f, "it cannot be read: {error}"),
        }
    }
}

// The extractor decides the exact list of files; the launcher only refuses to swap in something
// that is not a real folder holding a real program, such as a link to somewhere else.
pub fn check_new_app(folders: &Folders, version: &Version) -> Result<(), Unusable> {
    let folder = fs::symlink_metadata(folders.new_app(version)).map_err(Unusable::Unreadable)?;
    if !folder.is_dir() || is_reparse_point(&folder) {
        return Err(Unusable::NotAPlainFolder);
    }
    match fs::symlink_metadata(folders.new_program(version)) {
        Ok(program) if program.is_file() && !is_reparse_point(&program) => Ok(()),
        Ok(_) => Err(Unusable::NoPlainProgram),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Unusable::NoPlainProgram),
        Err(error) => Err(Unusable::Unreadable(error)),
    }
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

// A rename fails while anything inside the folder is open, so it is retried until `limit`.
pub fn rename_retrying(from: &Path, to: &Path, limit: Duration, poll: Duration) -> io::Result<()> {
    let deadline = Instant::now() + limit;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => sleep(poll),
        }
    }
}

pub fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

// `app.old` must be gone before the marker goes: both folders without a marker would make the
// next start take the new version for one that never started.
pub fn finish_update(folders: &Folders) -> io::Result<()> {
    remove_tree(&folders.old())?;
    let mut first_failure = None;
    for entry in fs::read_dir(folders.update())? {
        let entry = entry?;
        if kept_by_cleanup(&entry.file_name()) {
            continue;
        }
        let kind = entry.file_type()?;
        let removed = if kind.is_dir() || kind.is_symlink_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        if let Err(error) = removed {
            tracing::warn!(%error, path = %entry.path().display(), "could not clean up after the update");
            first_failure.get_or_insert(error);
        }
    }
    remove_marker(folders)?;
    first_failure.map_or(Ok(()), Err)
}
