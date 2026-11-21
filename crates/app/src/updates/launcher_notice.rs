use std::path::Path;

use super::store::{self, ReadError};
use super::version::Version;

const FILE: &str = "notice";
const MAX_BYTES: u64 = 64;

// What the launcher left in `.update\notice` about the update it just handled. The words are
// the ones the launcher writes, in crates/launcher/src/notice.rs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherNotice {
    Restored(Option<Version>),
    Interrupted,
    NotApplied(Version),
}

impl LauncherNotice {
    pub fn parse(bytes: &[u8]) -> Option<LauncherNotice> {
        let text = std::str::from_utf8(bytes).ok()?;
        match text.split_once(' ') {
            None if text == "restored" => Some(LauncherNotice::Restored(None)),
            None if text == "interrupted" => Some(LauncherNotice::Interrupted),
            None => None,
            Some(("restored", version)) => Some(LauncherNotice::Restored(Some(
                Version::parse_bare(version)?,
            ))),
            Some(("not-applied", version)) => {
                Some(LauncherNotice::NotApplied(Version::parse_bare(version)?))
            }
            Some(_) => None,
        }
    }
}

// The file goes once read, so a notice shows once even when it cannot be understood.
pub fn take(update_folder: &Path) -> Option<LauncherNotice> {
    let file = update_folder.join(FILE);
    let bytes = match store::read_capped(&file, MAX_BYTES) {
        Ok(bytes) => Some(bytes),
        Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, "could not read the launcher's notice");
            None
        }
    };
    if let Err(error) = std::fs::remove_file(&file) {
        tracing::warn!(%error, "could not delete the launcher's notice; it may show again");
    }
    let notice = LauncherNotice::parse(&bytes?);
    if notice.is_none() {
        tracing::warn!("the launcher left a notice this version does not understand");
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::{LauncherNotice, take};
    use crate::updates::Version;

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    #[test]
    fn the_words_the_launcher_writes_parse() {
        let parse = |text: &str| LauncherNotice::parse(text.as_bytes());
        assert_eq!(
            parse("restored 0.2.0"),
            Some(LauncherNotice::Restored(Some(version("0.2.0"))))
        );
        assert_eq!(parse("restored"), Some(LauncherNotice::Restored(None)));
        assert_eq!(parse("interrupted"), Some(LauncherNotice::Interrupted));
        assert_eq!(
            parse("not-applied 0.2.0"),
            Some(LauncherNotice::NotApplied(version("0.2.0")))
        );
    }

    #[test]
    fn anything_else_is_not_a_notice() {
        for text in [
            "",
            "restored ",
            "restored 0.2",
            "restored 00.2.0",
            "restored 0.2.0 extra",
            "not-applied",
            "not-applied x",
            "interrupted 0.2.0",
            "Restored",
            "restored\n",
            "upgraded 0.2.0",
        ] {
            assert_eq!(LauncherNotice::parse(text.as_bytes()), None, "{text:?}");
        }
        assert_eq!(LauncherNotice::parse(b"\xff\xfe"), None);
    }

    #[test]
    fn a_notice_is_taken_once() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("notice"), "interrupted").unwrap();
        assert_eq!(take(folder.path()), Some(LauncherNotice::Interrupted));
        assert!(!folder.path().join("notice").exists());
        assert_eq!(take(folder.path()), None);
    }

    #[test]
    fn an_oversized_or_unknown_notice_is_dropped() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("notice");
        std::fs::write(&file, "restored ".repeat(100)).unwrap();
        assert_eq!(take(folder.path()), None);
        assert!(!file.exists());
        std::fs::write(&file, "upgraded").unwrap();
        assert_eq!(take(folder.path()), None);
        assert!(!file.exists());
    }
}
