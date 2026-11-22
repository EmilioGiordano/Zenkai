use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

const FLAG: &str = "--version";

pub fn requested(mut arguments: impl Iterator<Item = OsString>) -> bool {
    arguments.next().is_some_and(|argument| argument == FLAG)
}

fn line() -> String {
    format!("zenkai {}", env!("CARGO_PKG_VERSION"))
}

// The Windows release build has no console, so this reaches a terminal only when the
// output is redirected.
pub fn print() -> ExitCode {
    match writeln!(std::io::stdout().lock(), "{}", line()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{line, requested};

    fn arguments(values: &[&str]) -> impl Iterator<Item = OsString> {
        values
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_flag_alone_asks_for_the_version() {
        assert!(requested(arguments(&["--version"])));
    }

    #[test]
    fn a_file_path_or_no_argument_does_not() {
        assert!(!requested(arguments(&[])));
        assert!(!requested(arguments(&["book.xlsx", "--version"])));
    }

    #[test]
    fn the_line_names_the_app_and_the_workspace_version() {
        assert_eq!(line(), concat!("zenkai ", env!("CARGO_PKG_VERSION")));
    }
}
