use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::version::Version;

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const MAX_FILE_BYTES: u64 = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub last_check: Option<u64>,
    pub announced: Option<Version>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    last_check: Option<u64>,
    #[serde(default)]
    announced: Option<String>,
}

impl Record {
    // A last check in the future (clock change, edited file) counts as expired.
    pub fn is_due(&self, now: u64) -> bool {
        match self.last_check {
            None => true,
            Some(last) => last > now || now - last >= CHECK_INTERVAL_SECS,
        }
    }

    pub fn should_announce(&self, found: Version, silenced_this_session: bool) -> bool {
        !silenced_this_session && self.announced.is_none_or(|announced| found > announced)
    }

    pub fn parse(bytes: &[u8]) -> Record {
        let Ok(stored) = serde_json::from_slice::<Stored>(bytes) else {
            return Record::default();
        };
        Record {
            last_check: stored.last_check,
            announced: stored.announced.as_deref().and_then(Version::parse_bare),
        }
    }

    fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&Stored {
            last_check: self.last_check,
            announced: self.announced.map(|version| version.to_string()),
        })
    }

    pub fn load() -> Record {
        let Some(file) = file() else {
            return Record::default();
        };
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&file)
            .and_then(|file| file.take(MAX_FILE_BYTES).read_to_end(&mut bytes));
        match read {
            Ok(_) => Record::parse(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Record::default(),
            Err(error) => {
                tracing::warn!(%error, "could not read the update check record");
                Record::default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(file) = file() else {
            return Ok(());
        };
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = self.to_json().map_err(std::io::Error::other)?;
        static SAVING: Mutex<()> = Mutex::new(());
        let _saving = SAVING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp = file.with_extension("tmp");
        std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, &file))
    }
}

fn file() -> Option<PathBuf> {
    Some(crate::recovery::directory()?.parent()?.join("updates.json"))
}

#[cfg(test)]
mod tests {
    use super::{CHECK_INTERVAL_SECS, Record};
    use crate::updates::version::Version;

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
        let record = Record::default();
        assert!(record.is_due(1_000));
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
        assert!(record.should_announce(version("0.1.1"), false));
        record.announced = Some(version("0.1.1"));
        assert!(!record.should_announce(version("0.1.1"), false));
        assert!(record.should_announce(version("0.1.2"), false));
    }

    #[test]
    fn an_older_found_version_is_not_announced_again() {
        let record = Record {
            announced: Some(version("0.2.0")),
            ..Record::default()
        };
        assert!(!record.should_announce(version("0.1.5"), false));
    }

    #[test]
    fn later_silences_the_session() {
        assert!(!Record::default().should_announce(version("0.1.1"), true));
    }

    #[test]
    fn the_record_survives_a_round_trip() {
        let record = Record {
            last_check: Some(1_700_000_000),
            announced: Some(version("1.2.3")),
        };
        let json = record.to_json().unwrap();
        assert_eq!(Record::parse(json.as_bytes()), record);
    }

    #[test]
    fn a_corrupt_record_is_the_default_and_due() {
        for bytes in [
            &b""[..],
            b"not json",
            b"[]",
            b"{\"last_check\":-1}",
            b"\xff",
        ] {
            let record = Record::parse(bytes);
            assert_eq!(record, Record::default());
            assert!(record.is_due(0));
        }
    }

    #[test]
    fn an_announced_value_that_is_not_a_version_is_dropped() {
        let record = Record::parse(br#"{"announced":"latest"}"#);
        assert_eq!(record.announced, None);
    }
}
