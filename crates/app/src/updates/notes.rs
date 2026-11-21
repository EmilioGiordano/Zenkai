use std::fmt;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::store::{self, ReadError, SaveError};
use super::version::Version;

pub const MAX_NOTE_BYTES: usize = 64 * 1024;
pub const MAX_NOTES: usize = 20;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Day {
    year: u16,
    month: u8,
    day: u8,
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl Day {
    // GitHub sends `2026-12-04T10:00:00Z`; only the date is kept.
    pub fn parse(text: &str) -> Option<Day> {
        let date = text.get(..10).filter(|date| date.is_ascii())?;
        if !matches!(text.as_bytes().get(10), None | Some(b'T')) {
            return None;
        }
        if date.as_bytes()[4] != b'-' || date.as_bytes()[7] != b'-' {
            return None;
        }
        let year = digits(&date[..4])?;
        let month = u8::try_from(digits(&date[5..7])?).ok()?;
        let day = u8::try_from(digits(&date[8..])?).ok()?;
        let valid =
            (1970..=9999).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day);
        valid.then_some(Day { year, month, day })
    }
}

fn digits(text: &str) -> Option<u16> {
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

// `body` is None when the release has no text or it is past the size limit: the version is
// still listed, so a note that is too big never hides an update.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Note {
    pub version: Version,
    pub published: Option<Day>,
    pub body: Option<String>,
}

// What the What's new tab shows: at most MAX_NOTES versions, newest first, one per version.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Notes {
    entries: Vec<Note>,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("the notes file is not valid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the notes file is over a limit")]
    OverLimit,
}

#[derive(Deserialize)]
struct Stored {
    notes: Vec<StoredNote>,
}

#[derive(Deserialize)]
struct StoredNote {
    version: String,
    #[serde(default)]
    published: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Serialize)]
struct Written<'a> {
    notes: &'a [WrittenNote<'a>],
}

#[derive(Serialize)]
struct WrittenNote<'a> {
    version: String,
    published: Option<String>,
    body: Option<&'a str>,
}

impl Notes {
    pub fn from_releases(mut found: Vec<Note>) -> Notes {
        found.sort_by(|a, b| b.version.cmp(&a.version));
        found.dedup_by_key(|note| note.version);
        found.truncate(MAX_NOTES);
        Notes { entries: found }
    }

    // The installed version moves up after an update, so the list is cut at read time, not
    // when it was fetched.
    pub fn newer_than(&self, installed: Version) -> impl Iterator<Item = &Note> {
        self.entries
            .iter()
            .filter(move |note| note.version > installed)
    }

    // Every limit is checked and a file that breaks one is refused whole: the text is written
    // by whoever controls the release, and this file can be edited by anyone on the machine.
    pub fn parse(bytes: &[u8]) -> Result<Notes, CacheError> {
        let stored: Stored = serde_json::from_slice(bytes)?;
        if stored.notes.len() > MAX_NOTES {
            return Err(CacheError::OverLimit);
        }
        let mut entries: Vec<Note> = Vec::with_capacity(stored.notes.len());
        for note in stored.notes {
            let version = Version::parse_bare(&note.version).ok_or(CacheError::OverLimit)?;
            let published = note
                .published
                .as_deref()
                .map(|text| Day::parse(text).ok_or(CacheError::OverLimit))
                .transpose()?;
            let in_order = entries
                .last()
                .is_none_or(|previous| previous.version > version);
            let in_size = note
                .body
                .as_ref()
                .is_none_or(|body| body.len() <= MAX_NOTE_BYTES);
            if !in_order || !in_size {
                return Err(CacheError::OverLimit);
            }
            entries.push(Note {
                version,
                published,
                body: note.body,
            });
        }
        Ok(Notes { entries })
    }

    // Escaping can make a file larger than the text in it, so the oldest versions are left
    // out until it fits what `parse` accepts.
    pub(crate) fn to_json(&self) -> Result<String, serde_json::Error> {
        let written: Vec<WrittenNote> = self
            .entries
            .iter()
            .map(|note| WrittenNote {
                version: note.version.to_string(),
                published: note.published.map(|day| day.to_string()),
                body: note.body.as_deref(),
            })
            .collect();
        for keep in (1..=written.len()).rev() {
            let json = serde_json::to_string(&Written {
                notes: &written[..keep],
            })?;
            if json.len() as u64 <= MAX_FILE_BYTES {
                return Ok(json);
            }
        }
        serde_json::to_string(&Written { notes: &[] })
    }

    // A missing or damaged cache is no notes: the next check fills it again.
    pub fn load() -> Notes {
        let Some(file) = file() else {
            return Notes::default();
        };
        match store::read_capped(&file, MAX_FILE_BYTES) {
            Ok(bytes) => Notes::parse(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the release notes cache is damaged; ignoring it");
                Notes::default()
            }),
            Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Notes::default()
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the release notes cache");
                Notes::default()
            }
        }
    }

    pub fn save(&self, order: u64) -> Result<(), SaveError> {
        static LAST_WRITTEN: Mutex<u64> = Mutex::new(0);
        let file = file().ok_or(SaveError::NoDirectory)?;
        self.write_to(&file, order, &LAST_WRITTEN)
    }

    fn write_to(
        &self,
        file: &std::path::Path,
        order: u64,
        last_written: &Mutex<u64>,
    ) -> Result<(), SaveError> {
        store::write_ordered(file, &self.to_json()?, order, last_written)
    }
}

fn file() -> Option<PathBuf> {
    Some(crate::recovery::session_directory()?.join("whats-new.json"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::version::Version;
    use super::{CacheError, Day, MAX_NOTE_BYTES, MAX_NOTES, Note, Notes};

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn note(text: &str, body: Option<&str>) -> Note {
        Note {
            version: version(text),
            published: Day::parse("2026-12-04"),
            body: body.map(str::to_owned),
        }
    }

    fn versions(notes: &Notes) -> Vec<String> {
        notes
            .newer_than(version("0.0.0"))
            .map(|note| note.version.to_string())
            .collect()
    }

    #[test]
    fn a_date_is_read_from_the_start_of_a_timestamp() {
        assert_eq!(Day::parse("2026-12-04").unwrap().to_string(), "2026-12-04");
        assert_eq!(
            Day::parse("2026-12-04T10:00:00Z").unwrap().to_string(),
            "2026-12-04"
        );
    }

    #[test]
    fn anything_else_is_not_a_date() {
        for text in [
            "",
            "2026",
            "2026-12-0",
            "2026-12-04x",
            "2026-12-04 10:00",
            "2026/12/04",
            "2026-13-04",
            "2026-00-04",
            "2026-12-00",
            "2026-12-32",
            "1969-12-04",
            "+026-12-04",
            "2026-1\u{e9}-04",
            "\u{202e}026-12-04",
            "2026-12-04\n",
        ] {
            assert_eq!(Day::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn notes_are_newest_first_one_per_version_and_at_most_twenty() {
        let mut found: Vec<Note> = (1..=30)
            .map(|patch| note(&format!("0.1.{patch}"), Some("text")))
            .collect();
        found.push(note("0.1.30", Some("again")));
        found.reverse();
        let notes = Notes::from_releases(found);
        let listed = versions(&notes);
        assert_eq!(listed.len(), MAX_NOTES);
        assert_eq!(listed.first().map(String::as_str), Some("0.1.30"));
        assert_eq!(listed.last().map(String::as_str), Some("0.1.11"));
    }

    #[test]
    fn only_versions_above_the_installed_one_are_listed() {
        let notes = Notes::from_releases(vec![
            note("0.1.0", Some("a")),
            note("0.1.1", Some("b")),
            note("0.2.0", Some("c")),
        ]);
        let newer: Vec<String> = notes
            .newer_than(version("0.1.0"))
            .map(|note| note.version.to_string())
            .collect();
        assert_eq!(newer, ["0.2.0", "0.1.1"]);
        assert_eq!(notes.newer_than(version("0.2.0")).count(), 0);
    }

    #[test]
    fn the_cache_survives_a_round_trip() {
        let notes = Notes::from_releases(vec![
            note("0.2.0", Some("# New\n\n- one \u{202e}\"quoted\"\n")),
            note("0.1.1", None),
            Note {
                version: version("0.1.0"),
                published: None,
                body: Some(String::new()),
            },
        ]);
        let json = notes.to_json().unwrap();
        assert_eq!(Notes::parse(json.as_bytes()).unwrap(), notes);
    }

    #[test]
    fn a_cache_that_breaks_a_limit_is_refused_whole() {
        let too_big = "x".repeat(MAX_NOTE_BYTES + 1);
        let many: Vec<String> = (0..=MAX_NOTES)
            .rev()
            .map(|patch| format!(r#"{{"version":"0.1.{patch}"}}"#))
            .collect();
        let cases = [
            format!(r#"{{"notes":[{{"version":"0.1.1","body":"{too_big}"}}]}}"#),
            format!(r#"{{"notes":[{}]}}"#, many.join(",")),
            r#"{"notes":[{"version":"latest"}]}"#.to_owned(),
            r#"{"notes":[{"version":"v0.1.1"}]}"#.to_owned(),
            r#"{"notes":[{"version":"0.1.1","published":"yesterday"}]}"#.to_owned(),
            r#"{"notes":[{"version":"0.1.1"},{"version":"0.1.2"}]}"#.to_owned(),
            r#"{"notes":[{"version":"0.1.1"},{"version":"0.1.1"}]}"#.to_owned(),
        ];
        for text in cases {
            assert!(
                matches!(Notes::parse(text.as_bytes()), Err(CacheError::OverLimit)),
                "{}",
                &text[..text.len().min(80)]
            );
        }
    }

    #[test]
    fn a_body_at_the_limit_is_accepted() {
        let exact = "x".repeat(MAX_NOTE_BYTES);
        let text = format!(r#"{{"notes":[{{"version":"0.1.1","body":"{exact}"}}]}}"#);
        assert!(Notes::parse(text.as_bytes()).is_ok());
    }

    #[test]
    fn a_file_that_is_not_notes_is_an_error() {
        for bytes in [&b""[..], b"[]", b"{}", b"null", b"{\"notes\":3}", b"\xff"] {
            assert!(Notes::parse(bytes).is_err(), "{bytes:?}");
        }
        assert_eq!(Notes::parse(br#"{"notes":[]}"#).unwrap(), Notes::default());
    }

    #[test]
    fn a_write_too_big_for_the_cache_leaves_out_the_oldest_versions() {
        let heavy = "\u{1}".repeat(MAX_NOTE_BYTES);
        let found: Vec<Note> = (1..=MAX_NOTES)
            .map(|patch| note(&format!("0.1.{patch}"), Some(&heavy)))
            .collect();
        let notes = Notes::from_releases(found);
        let json = notes.to_json().unwrap();
        assert!(json.len() as u64 <= super::MAX_FILE_BYTES);
        let kept = Notes::parse(json.as_bytes()).unwrap();
        let listed = versions(&kept);
        assert!(!listed.is_empty() && listed.len() < MAX_NOTES);
        assert_eq!(listed.first().map(String::as_str), Some("0.1.20"));
    }

    #[test]
    fn a_saved_cache_is_read_back_from_a_new_folder() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("session").join("whats-new.json");
        let notes = Notes::from_releases(vec![note("0.1.1", Some("fixed"))]);
        notes.write_to(&file, 1, &Mutex::new(0)).unwrap();
        assert_eq!(Notes::parse(&std::fs::read(&file).unwrap()).unwrap(), notes);
    }
}
