use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::Path;

use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

// The files of `app\` in the install layout (packaging/windows/package.ps1). Anything else in the
// zip is left out; the launcher and the relay at its root are never replaced.
pub const APP_FILES: [&str; 5] = [
    "zenkai.exe",
    "zenkai-pdf-host.exe",
    "zenkai-values-host.exe",
    "pdfium.dll",
    "pdfium-LICENSE.txt",
];
const PROGRAM: &str = "zenkai.exe";
const APP_FOLDER: &str = "app";
pub const MAX_ENTRIES: usize = 256;
pub const MAX_NAME_BYTES: usize = 255;
const NOT_IN_NAMES: &[u8] = b"\\:*?\"<>|";
const DEVICE_NAMES: [&str; 26] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
];
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const WRITE_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub total_bytes: u64,
    pub ratio: u64,
}

pub const LIMITS: Limits = Limits {
    total_bytes: 1024 * 1024 * 1024,
    ratio: 100,
};

// Devices, pipes and sockets count as links: none of them is a plain file or folder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Plain,
    Link,
}

// A name ending in `/` is a folder, as the zip format has it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    File,
    Folder,
}

pub struct Entry<'a> {
    pub name: &'a [u8],
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chosen {
    pub index: usize,
    pub file: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("the zip cannot be read: {0}")]
    Zip(#[from] ZipError),
    #[error("the zip has more than {} entries", MAX_ENTRIES)]
    TooManyEntries,
    #[error("the zip has an entry with an unsafe name: {0:?}")]
    UnsafeName(String),
    #[error("the zip has a link or a special file: {0:?}")]
    Link(String),
    #[error("the zip has two entries named {0:?} apart from case")]
    Duplicate(String),
    #[error("the zip lists the same name twice")]
    RepeatedName,
    #[error("the zip has a file that another entry treats as a folder: {0:?}")]
    FileAsFolder(String),
    #[error("the zip does not keep everything under one top-level folder")]
    NotOneTopFolder,
    #[error("the zip has an encrypted entry")]
    Encrypted,
    #[error("the zip has an entry compressed with an unsupported method")]
    Method,
    #[error("the zip has no app\\{}", PROGRAM)]
    NoProgram,
    #[error("an entry expands past its compression limit")]
    Ratio,
    #[error("the zip expands past {0} bytes")]
    TooLarge(u64),
    #[error("an entry is not the size its header declares")]
    SizeMismatch,
    #[error("an entry is damaged: {0}")]
    Damaged(#[source] io::Error),
    #[error("could not write the extracted files: {0}")]
    Write(#[source] io::Error),
}

fn shown(name: &[u8]) -> String {
    String::from_utf8_lossy(&name[..name.len().min(MAX_NAME_BYTES)]).into_owned()
}

// Every entry is checked, also those that are not extracted: one hostile name refuses the
// whole zip. Names are printable ASCII and plain on Windows, so lowercase compares them fully.
pub fn select(entries: &[Entry<'_>]) -> Result<Vec<Chosen>, ArchiveError> {
    if entries.len() > MAX_ENTRIES {
        return Err(ArchiveError::TooManyEntries);
    }
    let mut top = None;
    let mut seen = HashSet::new();
    let mut files = HashSet::new();
    let mut chosen = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.kind == Kind::Link {
            return Err(ArchiveError::Link(shown(entry.name)));
        }
        let (components, shape) = checked_components(entry.name)?;
        match top {
            None => top = Some(components[0]),
            Some(first) if first == components[0] => {}
            Some(_) => return Err(ArchiveError::NotOneTopFolder),
        }
        if shape == Shape::File && components.len() == 1 {
            return Err(ArchiveError::NotOneTopFolder);
        }
        let key = components.join("/").to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            return Err(ArchiveError::Duplicate(key));
        }
        if shape == Shape::File {
            files.insert(key);
            if let [_, APP_FOLDER, name] = components.as_slice()
                && let Some(file) = APP_FILES.iter().copied().find(|file| file == name)
            {
                chosen.push(Chosen { index, file });
            }
        }
    }
    for key in &seen {
        let inside_a_file = key
            .match_indices('/')
            .any(|(end, _)| files.contains(&key[..end]));
        if inside_a_file {
            return Err(ArchiveError::FileAsFolder(key.clone()));
        }
    }
    if !chosen.iter().any(|chosen| chosen.file == PROGRAM) {
        return Err(ArchiveError::NoProgram);
    }
    Ok(chosen)
}

fn checked_components(name: &[u8]) -> Result<(Vec<&str>, Shape), ArchiveError> {
    let unsafe_name = || ArchiveError::UnsafeName(shown(name));
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(unsafe_name());
    }
    let printable = name
        .iter()
        .all(|byte| (b' '..=b'~').contains(byte) && !NOT_IN_NAMES.contains(byte));
    if !printable {
        return Err(unsafe_name());
    }
    let text = std::str::from_utf8(name).map_err(|_| unsafe_name())?;
    let (path, shape) = match text.strip_suffix('/') {
        Some(path) => (path, Shape::Folder),
        None => (text, Shape::File),
    };
    let components: Vec<&str> = path.split('/').collect();
    if components
        .iter()
        .all(|component| plain_component(component))
    {
        Ok((components, shape))
    } else {
        Err(unsafe_name())
    }
}

// Windows drops a trailing dot or space, so `a.` and `a` would land on the same file; `.` and
// `..` end in a dot too. A device name stays a device with any extension.
fn plain_component(component: &str) -> bool {
    if component.is_empty() || component.ends_with('.') || component.ends_with(' ') {
        return false;
    }
    let base = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ');
    !DEVICE_NAMES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(base))
}

fn kind_of(mode: Option<u32>, attributes: u32, symlink: bool) -> Kind {
    let special = mode.is_some_and(|mode| {
        let file_type = mode & S_IFMT;
        file_type != 0 && file_type != S_IFREG && file_type != S_IFDIR
    });
    if symlink || special || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Kind::Link
    } else {
        Kind::Plain
    }
}

// `destination` must not exist: it is created here and removed again if anything fails, so a
// half-written folder never stays behind for the launcher.
pub fn extract(
    zip: impl Read + Seek,
    destination: &Path,
    limits: Limits,
) -> Result<(), ArchiveError> {
    let mut archive = ZipArchive::new(zip)?;
    if archive.len() > MAX_ENTRIES {
        return Err(ArchiveError::TooManyEntries);
    }
    // The reader keeps one entry per exact name, so a repeated one never reaches `select`.
    if archive.has_duplicate_names() {
        return Err(ArchiveError::RepeatedName);
    }
    let mut listed = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index_data(index)?;
        if entry.encrypted() {
            return Err(ArchiveError::Encrypted);
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ArchiveError::Method);
        }
        let kind = kind_of(
            entry.unix_mode(),
            entry.external_attributes(),
            entry.is_symlink(),
        );
        listed.push((entry.name_raw().to_vec(), kind));
    }
    let entries: Vec<Entry<'_>> = listed
        .iter()
        .map(|(name, kind)| Entry { name, kind: *kind })
        .collect();
    let chosen = select(&entries)?;
    fs::create_dir(destination).map_err(ArchiveError::Write)?;
    let written = write_chosen(&mut archive, &chosen, destination, limits);
    if written.is_err()
        && let Err(error) = fs::remove_dir_all(destination)
    {
        tracing::warn!(%error, "could not remove a half-extracted update");
    }
    written
}

fn write_chosen(
    archive: &mut ZipArchive<impl Read + Seek>,
    chosen: &[Chosen],
    destination: &Path,
    limits: Limits,
) -> Result<(), ArchiveError> {
    let mut buffer = vec![0; WRITE_CHUNK];
    let mut total = 0u64;
    for chosen in chosen {
        let mut entry = archive.by_index(chosen.index)?;
        let declared = entry.size();
        let allowed = entry.compressed_size().saturating_mul(limits.ratio);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join(chosen.file))
            .map_err(ArchiveError::Write)?;
        let mut written = 0u64;
        loop {
            let read = match entry.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(ArchiveError::Damaged(error)),
            };
            written += read as u64;
            total += read as u64;
            if written > declared {
                return Err(ArchiveError::SizeMismatch);
            }
            if written > allowed {
                return Err(ArchiveError::Ratio);
            }
            if total > limits.total_bytes {
                return Err(ArchiveError::TooLarge(limits.total_bytes));
            }
            file.write_all(&buffer[..read])
                .map_err(ArchiveError::Write)?;
        }
        if written != declared {
            return Err(ArchiveError::SizeMismatch);
        }
        file.sync_all().map_err(ArchiveError::Write)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive/tests.rs"]
mod tests;
