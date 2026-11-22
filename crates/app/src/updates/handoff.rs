use std::fmt;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, Prefix};
use std::process::{Command, Stdio};

use super::prepare::{self, PendingError};
use super::version::Version;
use crate::installation::{LAUNCHER, UPDATE};

// The launcher's argument (crates/launcher/src/arguments.rs); the version follows it as written.
const APPLY: &str = "--apply=";
const WRITE_PROBE: &str = ".zenkai-write-check-";
const GENERIC_WRITE: u32 = 0x4000_0000;
const DELETE: u32 = 0x0001_0000;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const ERROR_WRITE_PROTECT: i32 = 19;

// Why this install sends the user to Releases instead of updating itself: the app never asks
// for elevation and never updates on a network path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    NetworkPath,
    NoWriteAccess,
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Blocked::NetworkPath => "it runs from a network path",
            Blocked::NoWriteAccess => "this user cannot write its folder",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("this install cannot update itself: {0}")]
    Blocked(Blocked),
    #[error("the update did not verify again: {0}")]
    NotVerified(PendingError),
    #[error("could not prepare the update files: {0}")]
    Disk(#[source] io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drive {
    Letter(u8),
    Share,
    Unplaced,
}

fn drive_of(path: &Path) -> Drive {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Drive::Letter(letter),
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => Drive::Share,
            Prefix::Verbatim(_) | Prefix::DeviceNS(_) => Drive::Unplaced,
        },
        _ => Drive::Unplaced,
    }
}

// A path that cannot be placed on a local drive counts as a network one: updating never runs
// where it is unsure.
fn on_network(root: &Path) -> bool {
    match drive_of(root) {
        Drive::Letter(letter) => {
            let drive = format!("{}:\\", char::from(letter));
            winsafe::GetDriveType(Some(&drive)) == winsafe::co::DRIVE::REMOTE
        }
        Drive::Share | Drive::Unplaced => true,
    }
}

// The launcher renames `app` inside the root, so the root itself must be writable. The probe is
// deleted by Windows when its handle closes, so it never stays behind, even after a crash.
fn writable(root: &Path) -> io::Result<bool> {
    let probe = root.join(format!("{WRITE_PROBE}{}", std::process::id()));
    let opened = OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(GENERIC_WRITE | DELETE)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(probe);
    match opened {
        Ok(_) => Ok(true),
        Err(error)
            if error.kind() == io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(ERROR_WRITE_PROTECT) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

// Blocks on the disk: run it in the background.
pub fn blocked(root: &Path) -> io::Result<Option<Blocked>> {
    if on_network(root) {
        return Ok(Some(Blocked::NetworkPath));
    }
    Ok((!writable(root)?).then_some(Blocked::NoWriteAccess))
}

// Blocks: run it in the background. Right before the handoff the install is checked again and
// the zip verified again and extracted from scratch: nothing found at download time is trusted.
pub fn stage(root: &Path, version: Version, installed: Version) -> Result<(), StageError> {
    if let Some(blocked) = blocked(root).map_err(StageError::Disk)? {
        return Err(StageError::Blocked(blocked));
    }
    let update = root.join(UPDATE);
    prepare::plain_folder(&update).map_err(StageError::Disk)?;
    match prepare::verify_and_extract(&update, version, installed) {
        Ok(()) => Ok(()),
        Err(PendingError::Disk(error)) => Err(StageError::Disk(error)),
        Err(refused) => {
            prepare::discard(&update, version);
            Err(StageError::NotVerified(refused))
        }
    }
}

// The launcher waits for this process to end before it swaps the folders, and passes the restart
// flag on to the app it opens, so the session comes back as after any restart.
pub fn launch(root: &Path, version: Version) -> io::Result<()> {
    let mut command = Command::new(root.join(LAUNCHER));
    command
        .args(apply_arguments(version))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(crate::restart::DETACHED_NEW_GROUP);
    command.spawn().map(drop)
}

fn apply_arguments(version: Version) -> [String; 2] {
    [
        format!("{APPLY}{version}"),
        crate::restart::RESTARTED_FLAG.to_owned(),
    ]
}

#[cfg(test)]
#[path = "handoff/tests.rs"]
mod tests;
