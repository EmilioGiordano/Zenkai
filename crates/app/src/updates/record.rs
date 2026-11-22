use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::store::{self, ReadError, SaveError};
use super::version::Version;

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const MAX_FILE_BYTES: u64 = 4096;

// `last_check` is the last attempt and decides when the next automatic check is due.
// `restored` is the version the launcher had to roll back from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub last_check: Option<u64>,
    pub last_success: Option<u64>,
    pub newest: Option<Version>,
    pub announced: Option<Version>,
    pub restored: Option<Version>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    last_check: Option<u64>,
    #[serde(default)]
    last_success: Option<u64>,
    #[serde(default)]
    newest: Option<String>,
    #[serde(default)]
    announced: Option<String>,
    #[serde(default)]
    restored: Option<String>,
}

impl Record {
    // A last check in the future (clock change, edited file) counts as expired.
    pub fn is_due(&self, now: u64) -> bool {
        match self.last_check {
            None => true,
            Some(last) => last > now || now - last >= CHECK_INTERVAL_SECS,
        }
    }

    // Different, not greater: a bogus high tag announced once must not mute the real versions
    // that come after it. A version that had to be restored from is never offered on its own.
    pub fn should_announce(&self, found: Version) -> bool {
        self.announced != Some(found) && self.restored != Some(found)
    }

    pub fn parse(bytes: &[u8]) -> Result<Record, serde_json::Error> {
        let stored: Stored = serde_json::from_slice(bytes)?;
        let version = |text: Option<String>| text.as_deref().and_then(Version::parse_bare);
        Ok(Record {
            last_check: stored.last_check,
            last_success: stored.last_success,
            newest: version(stored.newest),
            announced: version(stored.announced),
            restored: version(stored.restored),
        })
    }

    fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&Stored {
            last_check: self.last_check,
            last_success: self.last_success,
            newest: self.newest.map(|version| version.to_string()),
            announced: self.announced.map(|version| version.to_string()),
            restored: self.restored.map(|version| version.to_string()),
        })
    }

    // A missing or damaged record is the default, which is due: the next check rebuilds it.
    pub fn load() -> Record {
        let Some(file) = file() else {
            return Record::default();
        };
        match store::read_capped(&file, MAX_FILE_BYTES) {
            Ok(bytes) => Record::parse(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the update check record is damaged; starting a new one");
                Record::default()
            }),
            Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Record::default()
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the update check record");
                Record::default()
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
    Some(crate::recovery::session_directory()?.join("updates.json"))
}

#[cfg(test)]
mod tests {
    use super::super::version::Version;
    use std::sync::Mutex;

    use super::{CHECK_INTERVAL_SECS, Record, SaveError};

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn checked_at(last: u64) -> Record {
        Record {
            last_check: Some(last),
            ..Record::default()
        }
    }

    #[test]
    fn a_new_install_is_due() {
        assert!(Record::default().is_due(1_000));
    }

    #[test]
    fn a_check_is_not_due_before_a_day_has_passed() {
        let record = checked_at(1_000);
        assert!(!record.is_due(1_000));
        assert!(!record.is_due(1_000 + CHECK_INTERVAL_SECS - 1));
        assert!(record.is_due(1_000 + CHECK_INTERVAL_SECS));
    }

    #[test]
    fn a_last_check_in_the_future_is_expired() {
        assert!(checked_at(5_000).is_due(4_999));
        assert!(checked_at(u64::MAX).is_due(0));
    }

    #[test]
    fn each_version_is_announced_once() {
        let mut record = Record::default();
        assert!(record.should_announce(version("0.1.1")));
        record.announced = Some(version("0.1.1"));
        assert!(!record.should_announce(version("0.1.1")));
        assert!(record.should_announce(version("0.1.2")));
    }

    #[test]
    fn a_restored_version_is_not_announced_again() {
        let record = Record {
            restored: Some(version("0.2.0")),
            ..Record::default()
        };
        assert!(!record.should_announce(version("0.2.0")));
        assert!(record.should_announce(version("0.2.1")));
    }

    #[test]
    fn a_bogus_high_tag_does_not_mute_the_real_versions_after_it() {
        let record = Record {
            announced: Some(version("9.9.9")),
            ..Record::default()
        };
        assert!(record.should_announce(version("0.1.1")));
    }

    #[test]
    fn the_record_survives_a_round_trip() {
        let record = Record {
            last_check: Some(1_700_000_100),
            last_success: Some(1_700_000_000),
            newest: Some(version("1.2.4")),
            announced: Some(version("1.2.3")),
            restored: Some(version("1.2.4")),
        };
        let json = record.to_json().unwrap();
        assert_eq!(Record::parse(json.as_bytes()).unwrap(), record);
    }

    #[test]
    fn a_damaged_record_is_an_error_and_the_default_is_due() {
        for bytes in [&b""[..], b"not json", b"{\"last_check\":-1}", b"\xff"] {
            assert!(Record::parse(bytes).is_err(), "{bytes:?}");
        }
        assert!(Record::default().is_due(0));
    }

    #[test]
    fn a_version_field_that_is_not_a_version_is_dropped() {
        let record = Record::parse(br#"{"announced":"latest","newest":"v1.0.0"}"#).unwrap();
        assert_eq!(record.announced, None);
        assert_eq!(record.newest, None);
    }

    #[test]
    fn a_saved_record_is_read_back_from_a_new_folder() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("session").join("updates.json");
        let record = Record {
            last_check: Some(7),
            announced: Some(version("1.2.3")),
            ..Record::default()
        };
        record.write_to(&file, 1, &Mutex::new(0)).unwrap();
        assert_eq!(
            Record::parse(&std::fs::read(&file).unwrap()).unwrap(),
            record
        );
    }

    #[test]
    fn a_record_that_cannot_be_written_is_an_error() {
        let folder = tempfile::tempdir().unwrap();
        let blocker = folder.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let result = Record::default().write_to(&blocker.join("updates.json"), 1, &Mutex::new(0));
        assert!(matches!(result, Err(SaveError::Write(_))));
    }

    #[test]
    fn an_old_record_with_a_switch_still_loads() {
        let record = Record::parse(br#"{"check_on_open":false,"last_check":5}"#).unwrap();
        assert_eq!(record.last_check, Some(5));
    }
}
