use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};

use serde::{Deserialize, Serialize};
use zenkai_i18n::t;

use crate::files::is_openable;

const MAX_PATH_UNITS: usize = 32_767;

// The file a second launch was given, as UTF-16 code units so no Windows path is lost.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    open: Option<Vec<u16>>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("not a plain drive or share path")]
    NotPlainPath,
    #[error("the path is too long")]
    TooLong,
    #[error("the path names an alternate data stream")]
    AlternateStream,
    #[error("the path climbs out of its folder")]
    RelativeSegment,
    #[error("Zenkai does not open this kind of file")]
    UnsupportedExtension,
}

impl Refusal {
    pub fn message(&self) -> &'static str {
        match self {
            Refusal::NotPlainPath => t!("tray.refusal.not_plain"),
            Refusal::TooLong => t!("tray.refusal.too_long"),
            Refusal::AlternateStream => t!("tray.refusal.stream"),
            Refusal::RelativeSegment => t!("tray.refusal.relative"),
            Refusal::UnsupportedExtension => t!("tray.refusal.extension"),
        }
    }
}

impl Handoff {
    pub fn of(open: Option<&Path>) -> Handoff {
        Handoff {
            open: open.map(|path| path.as_os_str().encode_wide().collect()),
        }
    }

    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn decode(line: &str) -> Option<Handoff> {
        serde_json::from_str(line.trim()).ok()
    }

    pub fn requested(&self) -> Option<Result<PathBuf, Refusal>> {
        self.open
            .as_deref()
            .map(|units| accept_path(&OsString::from_wide(units)))
    }
}

fn has_colon(name: &std::ffi::OsStr) -> bool {
    name.encode_wide().any(|unit| unit == u16::from(b':'))
}

pub fn accept_path(raw: &OsString) -> Result<PathBuf, Refusal> {
    if raw.encode_wide().count() > MAX_PATH_UNITS {
        return Err(Refusal::TooLong);
    }
    let path = Path::new(raw);
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(Refusal::NotPlainPath);
    };
    match prefix.kind() {
        Prefix::Disk(_) => {}
        Prefix::UNC(server, share) if !server.is_empty() && !share.is_empty() => {
            if has_colon(server) || has_colon(share) {
                return Err(Refusal::AlternateStream);
            }
        }
        _ => return Err(Refusal::NotPlainPath),
    }
    if components.next() != Some(Component::RootDir) {
        return Err(Refusal::NotPlainPath);
    }
    for component in components {
        match component {
            Component::Normal(name) if has_colon(name) => return Err(Refusal::AlternateStream),
            Component::Normal(_) => {}
            _ => return Err(Refusal::RelativeSegment),
        }
    }
    if !is_openable(path) {
        return Err(Refusal::UnsupportedExtension);
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(text: &str) -> Result<PathBuf, Refusal> {
        accept_path(&OsString::from(text))
    }

    #[test]
    fn a_handoff_round_trips_with_and_without_a_file() {
        let none = Handoff::of(None);
        assert_eq!(Handoff::decode(&none.encode().unwrap()), Some(none));
        let some = Handoff::of(Some(Path::new("C:\\Datos\\Ñandú planilla.xlsx")));
        assert_eq!(Handoff::decode(&some.encode().unwrap()), Some(some));
    }

    #[test]
    fn a_path_that_is_not_utf8_survives_the_trip() {
        let odd = OsString::from_wide(&[0x43, 0x3a, 0x5c, 0xd800, 0x2e, 0x63, 0x73, 0x76]);
        let handoff = Handoff::of(Some(Path::new(&odd)));
        let decoded = Handoff::decode(&handoff.encode().unwrap()).unwrap();
        assert_eq!(decoded.open.unwrap(), odd.encode_wide().collect::<Vec<_>>());
    }

    #[test]
    fn a_malformed_message_is_not_a_handoff() {
        assert_eq!(Handoff::decode(""), None);
        assert_eq!(Handoff::decode("not json"), None);
        assert_eq!(Handoff::decode("{\"open\":5}"), None);
        assert_eq!(Handoff::decode("{\"open\":\"C:\\\\a.xlsx\"}"), None);
    }

    #[test]
    fn the_newline_that_ends_a_message_is_ignored() {
        assert_eq!(
            Handoff::decode("{\"open\":null}\r\n"),
            Some(Handoff { open: None })
        );
    }

    #[test]
    fn plain_drive_paths_to_openable_files_are_accepted() {
        assert!(accepted("C:\\Datos\\planilla.xlsx").is_ok());
        assert!(accepted("d:\\a b\\x.CSV").is_ok());
        assert!(accepted("C:\\a\\notes.txt").is_ok());
    }

    #[test]
    fn plain_network_shares_are_accepted_with_the_same_rules() {
        assert!(accepted(r"\\server\share\dir\a.xlsx").is_ok());
        assert_eq!(
            accepted(r"\\server\share\a.xlsx:s"),
            Err(Refusal::AlternateStream)
        );
        assert_eq!(
            accepted(r"\\server\share\a.exe"),
            Err(Refusal::UnsupportedExtension)
        );
        assert_eq!(
            accepted(r"\\server\share\..\a.xlsx"),
            Err(Refusal::RelativeSegment)
        );
        assert!(accepted(r"\\server\a.xlsx").is_err());
    }

    #[test]
    fn network_device_and_namespace_paths_are_refused() {
        for text in [
            "\\\\.\\C:\\a.xlsx",
            "\\\\?\\C:\\a.xlsx",
            "\\\\?\\UNC\\host\\share\\a.xlsx",
            "\\\\.\\pipe\\x.xlsx",
        ] {
            assert_eq!(accepted(text), Err(Refusal::NotPlainPath), "{text}");
        }
    }

    #[test]
    fn relative_and_drive_relative_paths_are_refused() {
        for text in ["a.xlsx", "C:a.xlsx", "\\a.xlsx", ".\\a.xlsx"] {
            assert_eq!(accepted(text), Err(Refusal::NotPlainPath), "{text}");
        }
        assert_eq!(accepted("C:\\a\\..\\b.xlsx"), Err(Refusal::RelativeSegment));
    }

    #[test]
    fn alternate_data_streams_are_refused() {
        assert_eq!(
            accepted("C:\\a.xlsx:secret.csv"),
            Err(Refusal::AlternateStream)
        );
        assert_eq!(
            accepted("C:\\dir:stream\\a.xlsx"),
            Err(Refusal::AlternateStream)
        );
    }

    #[test]
    fn files_the_app_cannot_open_are_refused() {
        assert_eq!(
            accepted("C:\\a\\run.exe"),
            Err(Refusal::UnsupportedExtension)
        );
        assert_eq!(accepted("C:\\a\\noext"), Err(Refusal::UnsupportedExtension));
    }

    #[test]
    fn an_oversized_path_is_refused() {
        let long = format!("C:\\{}.xlsx", "a".repeat(MAX_PATH_UNITS));
        assert_eq!(accepted(&long), Err(Refusal::TooLong));
    }
}
