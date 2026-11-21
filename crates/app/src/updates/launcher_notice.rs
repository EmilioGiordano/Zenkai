use std::path::Path;

use super::store::{self, ReadError};
use super::version::Version;

const FILE: &str = "notice";
const MAX_BYTES: u64 = 256;
const MAX_NOTICES: usize = 4;

// What the launcher left in `.update\notice` about the update it just handled, one per line. The
// words are the ones the launcher writes, in crates/launcher/src/notice.rs.
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

// The file goes once read, so its notices show once. A line this version does not understand is
// skipped; a file past the limit is dropped whole.
pub fn take(update_folder: &Path) -> Vec<LauncherNotice> {
    let file = update_folder.join(FILE);
    let read = store::read_capped(&file, MAX_BYTES);
    if let Err(ReadError::Io(error)) = &read
        && error.kind() == std::io::ErrorKind::NotFound
    {
        return Vec::new();
    }
    if let Err(error) = std::fs::remove_file(&file) {
        tracing::warn!(%error, "could not delete the launcher's notice; it may show again");
    }
    let bytes = match read {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, "could not read the launcher's notice");
            return Vec::new();
        }
    };
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let notice = LauncherNotice::parse(line);
            if notice.is_none() {
                tracing::warn!("the launcher left a notice this version does not understand");
            }
            notice
        })
        .take(MAX_NOTICES)
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{LauncherNotice, take};
    use crate::updates::Version;

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    proptest! {
        #[test]
        fn only_what_the_launcher_writes_is_ever_a_notice(
            bytes in proptest::collection::vec(any::<u8>(), 0..80)
        ) {
            if LauncherNotice::parse(&bytes).is_some() {
                prop_assert!(
                    bytes == b"restored"
                        || bytes == b"interrupted"
                        || bytes.starts_with(b"restored ")
                        || bytes.starts_with(b"not-applied ")
                );
            }
        }
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
        assert_eq!(take(folder.path()), [LauncherNotice::Interrupted]);
        assert!(!folder.path().join("notice").exists());
        assert!(take(folder.path()).is_empty());
    }

    #[test]
    fn an_oversized_or_unknown_notice_is_dropped() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("notice");
        std::fs::write(&file, "restored ".repeat(100)).unwrap();
        assert!(take(folder.path()).is_empty());
        assert!(!file.exists());
        std::fs::write(&file, "upgraded").unwrap();
        assert!(take(folder.path()).is_empty());
        assert!(!file.exists());
    }

    #[test]
    fn each_line_is_a_notice_and_an_unknown_line_is_skipped() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("notice");
        std::fs::write(&file, "interrupted\nupgraded 9.9.9\nrestored 0.2.0").unwrap();
        assert_eq!(
            take(folder.path()),
            [
                LauncherNotice::Interrupted,
                LauncherNotice::Restored(Some(version("0.2.0")))
            ]
        );
    }
}
