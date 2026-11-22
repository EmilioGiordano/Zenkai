use std::ffi::OsString;
use std::fmt;

const APPLY: &str = "--apply=";
const MAX_DIGITS: usize = 5;

// Kept exactly as written: the app confirms its start with this same text, byte for byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Version {
    // `^(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})$`: the app's own grammar,
    // no leading zero, so the text the app confirms with is this same text; and safe in a path.
    pub fn parse(text: &str) -> Option<Version> {
        let parts: Vec<&str> = text.split('.').collect();
        let valid = parts.len() == 3
            && parts.iter().all(|part| {
                (1..=MAX_DIGITS).contains(&part.len())
                    && part.bytes().all(|b| b.is_ascii_digit())
                    && (*part == "0" || !part.starts_with('0'))
            });
        valid.then(|| Version(text.to_owned()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Open,
    Apply(Version),
    Malformed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub request: Request,
    pub passed: Vec<OsString>,
}

// Only a first argument `--apply=<version>` is the launcher's; every other one goes to the app
// unchanged. A malformed one is dropped, so the app never takes it for a file.
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Launch {
    let mut arguments = arguments.into_iter().peekable();
    let apply = arguments
        .peek()
        .and_then(|first| first.to_str())
        .and_then(|first| first.strip_prefix(APPLY))
        .map(Version::parse);
    let request = match apply {
        None => Request::Open,
        Some(Some(version)) => Request::Apply(version),
        Some(None) => Request::Malformed,
    };
    if request != Request::Open {
        arguments.next();
    }
    Launch {
        request,
        passed: arguments.collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{Launch, Request, Version, parse};

    fn launch(arguments: &[&str]) -> Launch {
        parse(arguments.iter().map(OsString::from))
    }

    fn passed(arguments: &[&str]) -> Vec<OsString> {
        arguments.iter().map(OsString::from).collect()
    }

    #[test]
    fn versions_within_the_pattern_parse() {
        for text in [
            "0.0.0",
            "0.1.0",
            "1.2.3",
            "0.10.0",
            "10.0.0",
            "99999.99999.99999",
        ] {
            assert_eq!(
                Version::parse(text).map(|v| v.to_string()),
                Some(text.to_owned())
            );
        }
    }

    #[test]
    fn versions_outside_the_pattern_do_not_parse() {
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "00.01.002",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "00.0.0",
            "1..3",
            ".1.2",
            "1.2.",
            "123456.0.0",
            "0.0.123456",
            "v1.2.3",
            "1.2.3-rc1",
            "1.2.3 ",
            " 1.2.3",
            "1.2.3\n",
            "1.2.\u{0663}",
            "1.2.+3",
            "-1.2.3",
            "1.2.3/..",
            r"..\..\x",
            "1.2.3\0",
        ] {
            assert_eq!(Version::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn without_apply_everything_goes_to_the_app() {
        assert_eq!(
            launch(&[r"C:\book.xlsx", "--restarted"]),
            Launch {
                request: Request::Open,
                passed: passed(&[r"C:\book.xlsx", "--restarted"]),
            }
        );
        assert_eq!(launch(&[]).request, Request::Open);
    }

    #[test]
    fn apply_takes_the_first_argument_and_passes_the_rest() {
        let parsed = launch(&["--apply=0.2.0", "--restarted", "--apply=9.9.9"]);
        assert_eq!(
            parsed.request,
            Request::Apply(Version::parse("0.2.0").unwrap())
        );
        assert_eq!(parsed.passed, passed(&["--restarted", "--apply=9.9.9"]));
    }

    #[test]
    fn an_apply_after_the_first_argument_belongs_to_the_app() {
        let parsed = launch(&["book.xlsx", "--apply=0.2.0"]);
        assert_eq!(parsed.request, Request::Open);
        assert_eq!(parsed.passed, passed(&["book.xlsx", "--apply=0.2.0"]));
    }

    #[test]
    fn a_malformed_apply_is_dropped() {
        for first in [
            "--apply=",
            r"--apply=..\evil",
            "--apply=1.2",
            r"--apply=C:\x",
        ] {
            let parsed = launch(&[first, "book.xlsx"]);
            assert_eq!(parsed.request, Request::Malformed, "{first}");
            assert_eq!(parsed.passed, passed(&["book.xlsx"]));
        }
    }
}
