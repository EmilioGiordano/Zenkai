use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use zenkai_engine::{EngineError, write_atomic};
use zenkai_i18n::t;

pub const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Format {
    pub line_ending: LineEnding,
    pub bom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StampRecord {
    modified_nanos: Option<u64>,
    len: u64,
}

impl Stamp {
    pub fn record(self) -> StampRecord {
        StampRecord {
            modified_nanos: self
                .modified
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|since| u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)),
            len: self.len,
        }
    }

    pub fn from_record(record: StampRecord) -> Stamp {
        Stamp {
            modified: record
                .modified_nanos
                .map(|nanos| UNIX_EPOCH + Duration::from_nanos(nanos)),
            len: record.len,
        }
    }

    // Equal to no file on disk, so a save guarded by it always sees a change.
    fn unknown() -> Stamp {
        Stamp {
            modified: None,
            len: u64::MAX,
        }
    }
}

// The stamp a text restored from a recovery copy is saved over, and whether the file moved
// on since the edits began. Without a recorded base nothing proves the file is unchanged.
pub fn recovered_base(base: Option<StampRecord>, disk: Option<Stamp>) -> (Option<Stamp>, bool) {
    match (base, disk) {
        (Some(base), Some(disk)) if disk.record() == base => (Some(disk), false),
        (Some(base), _) => (Some(Stamp::from_record(base)), true),
        (None, _) => (Some(Stamp::unknown()), true),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub format: Format,
    pub mixed_endings: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("{}", t!("open.not_found"))]
    Missing,
    #[error("{}", t!("text.too_large", limit = MAX_TEXT_BYTES / (1024 * 1024)))]
    TooLarge,
    #[error("{}", t!("text.not_utf8"))]
    NotUtf8,
    #[error("{0}")]
    Io(String),
}

// The editor holds text with `\n` only; the file's own ending and BOM are put back on save.
// A file that mixes endings is saved with the dominant one, which the caller reports.
pub fn decode(bytes: &[u8]) -> Result<Decoded, ReadError> {
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(body) => (true, body),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|_| ReadError::NotUtf8)?;
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let line_ending = if crlf > lf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };
    let text = if crlf > 0 {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    };
    Ok(Decoded {
        text,
        format: Format { line_ending, bom },
        mixed_endings: crlf > 0 && lf > 0,
    })
}

pub fn encode(text: &str, format: Format) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + BOM.len());
    if format.bom {
        bytes.extend_from_slice(BOM);
    }
    match format.line_ending {
        LineEnding::Lf => bytes.extend_from_slice(text.as_bytes()),
        LineEnding::CrLf => bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    bytes
}

pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

// Runs off the UI thread. The size is checked before reading and again on the bytes read,
// since the file may grow in between.
pub fn read(path: &Path) -> Result<(Decoded, Option<Stamp>), ReadError> {
    let file = std::fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ReadError::Missing,
        _ => ReadError::Io(t!("file.read_failed", path = path.display(), error = error)),
    })?;
    let stamp = stamp_of(path);
    if stamp.is_some_and(|stamp| stamp.len > MAX_TEXT_BYTES) {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ReadError::Io(t!("file.read_failed", path = path.display(), error = error))
        })?;
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err(ReadError::TooLarge);
    }
    Ok((decode(&bytes)?, stamp))
}

// Runs off the UI thread. Returns the stamp of what was written, so the write is not later
// mistaken for a change made by another program.
pub fn write(path: &Path, text: &str, format: Format) -> Result<Option<Stamp>, EngineError> {
    let bytes = encode(text, format);
    write_atomic(path, &bytes, |written| {
        if decode(written).is_ok() {
            Ok(())
        } else {
            Err(EngineError::VerifyFailed(t!("text.not_utf8").to_string()))
        }
    })?;
    Ok(stamp_of(path))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChange {
    Unchanged,
    Changed,
    Deleted,
}

pub fn disk_change(known: Option<Stamp>, now: Option<Stamp>) -> DiskChange {
    match (known, now) {
        (_, None) => DiskChange::Deleted,
        (Some(known), Some(now)) if known == now => DiskChange::Unchanged,
        (None, Some(_)) => DiskChange::Unchanged,
        (Some(_), Some(_)) => DiskChange::Changed,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaction {
    Nothing,
    ReloadQuietly,
    OfferReload,
    KeepAndMarkDeleted,
}

// Never overwrites local edits: a changed file with edits is offered, not applied.
pub fn react(change: DiskChange, has_local_edits: bool) -> Reaction {
    match (change, has_local_edits) {
        (DiskChange::Unchanged, _) => Reaction::Nothing,
        (DiskChange::Changed, false) => Reaction::ReloadQuietly,
        (DiskChange::Changed, true) => Reaction::OfferReload,
        (DiskChange::Deleted, _) => Reaction::KeepAndMarkDeleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn stamp(len: u64) -> Option<Stamp> {
        Some(Stamp {
            modified: None,
            len,
        })
    }

    #[test]
    fn crlf_is_edited_as_lf_and_written_back_as_crlf() {
        let decoded = decode(b"a\r\nb\r\n").unwrap();
        assert_eq!(decoded.text, "a\nb\n");
        assert_eq!(decoded.format.line_ending, LineEnding::CrLf);
        assert!(!decoded.mixed_endings);
        assert_eq!(encode(&decoded.text, decoded.format), b"a\r\nb\r\n");
    }

    #[test]
    fn the_bom_is_kept_out_of_the_text_and_put_back() {
        let decoded = decode(b"\xEF\xBB\xBFhi\n").unwrap();
        assert_eq!(decoded.text, "hi\n");
        assert!(decoded.format.bom);
        assert_eq!(encode(&decoded.text, decoded.format), b"\xEF\xBB\xBFhi\n");
    }

    #[test]
    fn a_file_without_a_final_newline_stays_without_one() {
        let decoded = decode(b"one\ntwo").unwrap();
        assert_eq!(encode(&decoded.text, decoded.format), b"one\ntwo");
    }

    #[test]
    fn mixed_endings_are_reported_and_take_the_dominant_one() {
        let decoded = decode(b"a\r\nb\r\nc\n").unwrap();
        assert!(decoded.mixed_endings);
        assert_eq!(decoded.format.line_ending, LineEnding::CrLf);
        let decoded = decode(b"a\nb\nc\r\n").unwrap();
        assert!(decoded.mixed_endings);
        assert_eq!(decoded.format.line_ending, LineEnding::Lf);
    }

    #[test]
    fn a_lone_carriage_return_survives() {
        let decoded = decode(b"a\rb\r\nc\r\n").unwrap();
        assert_eq!(encode(&decoded.text, decoded.format), b"a\rb\r\nc\r\n");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_refused_instead_of_decoded_lossily() {
        assert_eq!(decode(b"caf\xE9").unwrap_err(), ReadError::NotUtf8);
        assert_eq!(decode(b"\xFF\xFEa\0").unwrap_err(), ReadError::NotUtf8);
    }

    #[test]
    fn a_file_over_the_cap_is_refused_and_one_at_the_cap_opens() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.md");
        std::fs::write(&big, vec![b'a'; MAX_TEXT_BYTES as usize + 1]).unwrap();
        assert_eq!(read(&big).unwrap_err(), ReadError::TooLarge);
        let limit = dir.path().join("limit.md");
        std::fs::write(&limit, vec![b'a'; MAX_TEXT_BYTES as usize]).unwrap();
        assert!(read(&limit).is_ok());
    }

    #[test]
    fn a_missing_file_is_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read(&dir.path().join("none.md")).unwrap_err(),
            ReadError::Missing
        );
    }

    #[test]
    fn saving_replaces_the_file_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, b"old\r\n").unwrap();
        let (decoded, _) = read(&path).unwrap();
        let text = format!("{}new\n", decoded.text);
        let stamp = write(&path, &text, decoded.format).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old\r\nnew\r\n");
        assert_eq!(stamp, stamp_of(&path));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_disk_change_is_told_from_the_stamp() {
        assert_eq!(disk_change(stamp(1), stamp(1)), DiskChange::Unchanged);
        assert_eq!(disk_change(stamp(1), stamp(2)), DiskChange::Changed);
        assert_eq!(disk_change(stamp(1), None), DiskChange::Deleted);
        assert_eq!(disk_change(None, stamp(1)), DiskChange::Unchanged);
    }

    #[test]
    fn a_stamp_survives_its_record() {
        let path = std::env::temp_dir();
        let stamp = stamp_of(&path).unwrap();
        assert_eq!(Stamp::from_record(stamp.record()), stamp);
        let json = serde_json::to_string(&stamp.record()).unwrap();
        assert_eq!(
            serde_json::from_str::<StampRecord>(&json).unwrap(),
            stamp.record()
        );
    }

    #[test]
    fn a_recovered_text_trusts_the_file_only_if_it_is_the_one_its_edits_began_from() {
        let disk = stamp(5);
        let same = disk.map(Stamp::record);
        assert_eq!(recovered_base(same, disk), (disk, false));
        let (guard, moved) = recovered_base(stamp(4).map(Stamp::record), disk);
        assert!(moved);
        assert_eq!(disk_change(guard, disk), DiskChange::Changed);
        let (guard, moved) = recovered_base(None, disk);
        assert!(moved);
        assert_eq!(disk_change(guard, disk), DiskChange::Changed);
        let (guard, moved) = recovered_base(same, None);
        assert!(moved);
        assert_eq!(disk_change(guard, None), DiskChange::Deleted);
    }

    #[test]
    fn a_clean_document_reloads_quietly_and_one_with_edits_asks() {
        assert_eq!(react(DiskChange::Unchanged, true), Reaction::Nothing);
        assert_eq!(react(DiskChange::Changed, false), Reaction::ReloadQuietly);
        assert_eq!(react(DiskChange::Changed, true), Reaction::OfferReload);
        assert_eq!(
            react(DiskChange::Deleted, false),
            Reaction::KeepAndMarkDeleted
        );
        assert_eq!(
            react(DiskChange::Deleted, true),
            Reaction::KeepAndMarkDeleted
        );
    }

    proptest! {
        #[test]
        fn an_unedited_file_with_uniform_endings_saves_back_to_the_same_bytes(
            lines in proptest::collection::vec("[ -~\u{e9}\u{4e2d}\u{1f600}]{0,20}", 0..8),
            crlf in any::<bool>(),
            bom in any::<bool>(),
            final_newline in any::<bool>(),
        ) {
            let ending = if crlf { "\r\n" } else { "\n" };
            let mut original = Vec::new();
            if bom {
                original.extend_from_slice(BOM);
            }
            original.extend_from_slice(lines.join(ending).as_bytes());
            if final_newline && !lines.is_empty() {
                original.extend_from_slice(ending.as_bytes());
            }
            let decoded = decode(&original).unwrap();
            prop_assert!(!decoded.mixed_endings);
            prop_assert_eq!(encode(&decoded.text, decoded.format), original);
        }
    }
}
