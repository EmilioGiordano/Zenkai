use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek};
use std::iter;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::archive::{self, ArchiveError, FILE_ATTRIBUTE_REPARSE_POINT};
use super::download::{self, DownloadError};
use super::signature::{self, MAX_SIGNATURE_BYTES, MAX_ZIP_BYTES, PLATFORM, SignatureError};
use super::store::{self, ReadError};
use super::version::Version;

pub const MAX_TRIES: usize = 3;
const ASSET_PREFIX: &str = "zenkai-v";
const ZIP_SUFFIX: &str = ".zip";
const SIGNATURE_SUFFIX: &str = ".zip.minisig";
// The launcher swaps in `.update\new-<version>` (crates/launcher/src/folders.rs, `new_app`).
const NEW_APP_PREFIX: &str = "new-";
const NOT_FOUND: u16 = 404;
const FILE_SHARE_READ: u32 = 0x1;

pub fn asset_name(version: Version) -> String {
    format!("{ASSET_PREFIX}{version}-{PLATFORM}{ZIP_SUFFIX}")
}

pub fn signature_name(version: Version) -> String {
    format!("{ASSET_PREFIX}{version}-{PLATFORM}{SIGNATURE_SUFFIX}")
}

pub fn new_app(update: &Path, version: Version) -> PathBuf {
    update.join(format!("{NEW_APP_PREFIX}{version}"))
}

#[derive(Debug, thiserror::Error)]
pub enum PendingError {
    #[error("the signature check refused it: {0}")]
    Signature(SignatureError),
    #[error("the archive was refused: {0}")]
    Archive(ArchiveError),
    #[error("could not read or write the update files: {0}")]
    Disk(#[source] io::Error),
}

impl From<SignatureError> for PendingError {
    fn from(error: SignatureError) -> PendingError {
        match error {
            SignatureError::Read(error) => PendingError::Disk(error),
            refused => PendingError::Signature(refused),
        }
    }
}

impl From<ArchiveError> for PendingError {
    fn from(error: ArchiveError) -> PendingError {
        match error {
            ArchiveError::Write(error) => PendingError::Disk(error),
            refused => PendingError::Archive(refused),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error("no version verified; the newest tried was {0}")]
    NotVerified(Version),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("could not read or write the update files: {0}")]
    Disk(#[source] io::Error),
}

// Blocks: run it on a background thread. `fetch` writes the zip and the signature of one version
// into `update`. What does not verify is deleted and the version before it gets its turn, so
// one bad release cannot block updates; a failed download stops, since the next would fail too.
pub fn prepare(
    update: &Path,
    chosen: Version,
    older: &[Version],
    installed: Version,
    mut fetch: impl FnMut(Version) -> Result<(), DownloadError>,
) -> Result<Version, PrepareError> {
    plain_folder(update).map_err(PrepareError::Disk)?;
    remove_leftovers(update);
    for version in iter::once(chosen)
        .chain(older.iter().copied())
        .take(MAX_TRIES)
    {
        match fetch(version) {
            Ok(()) => {}
            Err(DownloadError::Status(NOT_FOUND)) => {
                tracing::warn!(%version, "the release has no update for this platform");
                discard(update, version);
                continue;
            }
            Err(error) => {
                discard(update, version);
                return Err(error.into());
            }
        }
        match verify_and_extract(update, version, installed) {
            Ok(()) => return Ok(version),
            Err(PendingError::Disk(error)) => {
                discard(update, version);
                return Err(PrepareError::Disk(error));
            }
            Err(refused) => {
                tracing::warn!(%refused, %version, "the update did not verify and was deleted");
                discard(update, version);
            }
        }
    }
    Err(PrepareError::NotVerified(chosen))
}

// Also the check right before the swap: a zip verified earlier is never trusted for that, so
// this reads it again from disk and extracts it from scratch.
pub fn verify_and_extract(
    update: &Path,
    version: Version,
    installed: Version,
) -> Result<(), PendingError> {
    let folder = new_app(update, version);
    remove_tree(&folder).map_err(PendingError::Disk)?;
    let signature =
        match store::read_capped(&update.join(signature_name(version)), MAX_SIGNATURE_BYTES) {
            Ok(signature) => signature,
            Err(ReadError::TooLarge(_)) => {
                return Err(PendingError::Signature(SignatureError::SignatureTooLarge));
            }
            Err(ReadError::Io(error)) => return Err(PendingError::Disk(error)),
        };
    // Nothing else may write or delete the zip between its verification and its extraction.
    let mut zip = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(update.join(asset_name(version)))
        .map_err(PendingError::Disk)?;
    signature::verify(&mut zip, &signature, version, installed)?;
    zip.rewind().map_err(PendingError::Disk)?;
    archive::extract(&mut zip, &folder, archive::LIMITS)?;
    Ok(())
}

// Blocks on the network: the `fetch` that `prepare` runs in the app.
pub fn fetch_release(
    agent: &ureq::Agent,
    update: &Path,
    version: Version,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), DownloadError> {
    let signature = signature_name(version);
    let mut file = fresh_file(&update.join(&signature)).map_err(DownloadError::Write)?;
    let url = download::asset_url(version, &signature)?;
    download::download(
        agent,
        url,
        &mut file,
        MAX_SIGNATURE_BYTES,
        cancel,
        |_, _| {},
    )?;
    let asset = asset_name(version);
    let mut file = fresh_file(&update.join(&asset)).map_err(DownloadError::Write)?;
    let url = download::asset_url(version, &asset)?;
    download::download(agent, url, &mut file, MAX_ZIP_BYTES, cancel, progress)?;
    Ok(())
}

// A name left there could be a link to elsewhere; it is removed and the file made anew.
fn fresh_file(path: &Path) -> io::Result<File> {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn plain_folder(update: &Path) -> io::Result<()> {
    if let Err(error) = fs::create_dir(update)
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(error);
    }
    let metadata = fs::symlink_metadata(update)?;
    if metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        Ok(())
    } else {
        Err(io::Error::other(".update is not a plain folder"))
    }
}

fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

fn discard(update: &Path, version: Version) {
    let removed = [
        remove_file(&update.join(asset_name(version))),
        remove_file(&update.join(signature_name(version))),
        remove_tree(&new_app(update, version)),
    ];
    for error in removed.into_iter().filter_map(Result::err) {
        tracing::warn!(%error, %version, "could not delete an update that did not verify");
    }
}

// Downloads and extractions of earlier attempts; the launcher's own files are left alone.
fn remove_leftovers(update: &Path) {
    let entries = match fs::read_dir(update) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, "could not list earlier update downloads");
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, "could not list earlier update downloads");
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let download = name.starts_with(ASSET_PREFIX)
            && (name.ends_with(ZIP_SUFFIX) || name.ends_with(SIGNATURE_SUFFIX));
        let extracted = name.starts_with(NEW_APP_PREFIX);
        if !download && !extracted {
            continue;
        }
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => remove_tree(&path),
            Ok(_) => remove_file(&path),
            Err(error) => Err(error),
        };
        if let Err(error) = removed {
            tracing::warn!(%error, path = %path.display(), "could not delete an earlier update download");
        }
    }
}

#[cfg(test)]
#[path = "prepare/tests.rs"]
mod tests;
