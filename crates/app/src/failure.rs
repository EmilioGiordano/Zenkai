use std::io::{self, ErrorKind};
use std::path::Path;

use zenkai_engine::EngineError;
use zenkai_i18n::t;

use crate::files::LoadFailure;

const WINDOWS_SHARING_VIOLATION: i32 = 32;
const WINDOWS_LOCK_VIOLATION: i32 = 33;

// What the user sees, and what Copy and the Messages panel keep: the full path and the
// operating system's own words, which may be in another language.
pub struct Failure {
    pub text: String,
    pub detail: String,
}

#[derive(Clone, Copy)]
enum Access {
    Read,
    Write,
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn sentence(text: String) -> String {
    if text.ends_with(['.', '!', '?']) {
        text
    } else {
        format!("{text}.")
    }
}

fn io_reason(kind: ErrorKind, os_code: Option<i32>, access: Access) -> String {
    let locked = cfg!(windows)
        && matches!(
            os_code,
            Some(WINDOWS_SHARING_VIOLATION | WINDOWS_LOCK_VIOLATION)
        );
    if locked {
        return t!("io.in_use").to_string();
    }
    match (kind, access) {
        (ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem, Access::Write) => {
            t!("io.write_denied").to_string()
        }
        (ErrorKind::PermissionDenied, Access::Read) => t!("io.read_denied").to_string(),
        (ErrorKind::NotFound, _) => t!("io.not_found").to_string(),
        (ErrorKind::StorageFull, _) => t!("io.storage_full").to_string(),
        (_, Access::Write) => t!("io.write_other").to_string(),
        (_, Access::Read) => t!("io.read_other").to_string(),
    }
}

fn io_error_reason(error: &io::Error, access: Access) -> String {
    io_reason(error.kind(), error.raw_os_error(), access)
}

fn engine_reason(error: &EngineError) -> String {
    match error {
        EngineError::Read { source, .. } => io_error_reason(source, Access::Read),
        EngineError::Write { source, .. } => io_error_reason(source, Access::Write),
        other => other.to_string(),
    }
}

pub fn open_failure(path: &Path, failure: &LoadFailure) -> Failure {
    let name = file_name(path);
    let full = path.display();
    match failure {
        LoadFailure::Missing => Failure {
            text: t!("open.path_not_found", name = name),
            detail: t!("open.path_not_found", name = full),
        },
        LoadFailure::Engine(error) => Failure {
            text: t!(
                "open.failed",
                name = name,
                reason = sentence(engine_reason(error))
            ),
            detail: t!("open.failed", name = full, reason = error),
        },
        LoadFailure::Unreadable { reason, fallback } => Failure {
            text: t!(
                "open.unreadable",
                name = name,
                reason = reason,
                fallback = fallback
            ),
            detail: t!(
                "open.unreadable",
                name = full,
                reason = reason,
                fallback = fallback
            ),
        },
    }
}

pub fn save_failure(path: &Path, error: &EngineError) -> Failure {
    Failure {
        text: t!(
            "notice.save_failed",
            name = file_name(path),
            reason = sentence(engine_reason(error))
        ),
        detail: t!("notice.save_failed", name = path.display(), reason = error),
    }
}

pub fn offer_another_location(failure: Failure) -> Failure {
    Failure {
        text: t!("notice.save_failed_pick_another", error = failure.text),
        detail: failure.detail,
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    use zenkai_engine::EngineError;

    use super::{
        Access, file_name, io_reason, offer_another_location, open_failure, save_failure, sentence,
    };
    use crate::files::LoadFailure;

    fn write_error(kind: ErrorKind, path: &str) -> EngineError {
        EngineError::Write {
            path: PathBuf::from(path),
            source: std::io::Error::new(kind, "Acceso denegado. (os error 5)"),
        }
    }

    #[test]
    fn the_file_name_drops_the_folders() {
        assert_eq!(file_name(Path::new("data/report.xlsx")), "report.xlsx");
        assert_eq!(file_name(Path::new("report.xlsx")), "report.xlsx");
    }

    #[test]
    fn a_sentence_gets_one_final_period() {
        assert_eq!(
            sentence("Could not find EOCD".into()),
            "Could not find EOCD."
        );
        assert_eq!(sentence("Done.".into()), "Done.");
        assert_eq!(sentence("Really?".into()), "Really?");
    }

    #[test]
    fn io_kinds_map_to_plain_messages() {
        let reason = |kind, access| io_reason(kind, None, access);
        assert_eq!(
            reason(ErrorKind::PermissionDenied, Access::Write),
            "Zenkai can't write to this location (read-only or no permission)."
        );
        assert_eq!(
            reason(ErrorKind::ReadOnlyFilesystem, Access::Write),
            reason(ErrorKind::PermissionDenied, Access::Write)
        );
        assert_ne!(
            reason(ErrorKind::PermissionDenied, Access::Read),
            reason(ErrorKind::PermissionDenied, Access::Write)
        );
        assert_ne!(
            reason(ErrorKind::NotFound, Access::Write),
            reason(ErrorKind::StorageFull, Access::Write)
        );
        assert_ne!(
            reason(ErrorKind::StorageFull, Access::Write),
            reason(ErrorKind::Interrupted, Access::Write)
        );
    }

    #[test]
    fn a_save_error_is_capitalised_names_the_file_and_hides_the_os_text() {
        let error = write_error(ErrorKind::PermissionDenied, "data/ro.xlsx");
        let failure = save_failure(Path::new("data/ro.xlsx"), &error);
        assert_eq!(
            failure.text,
            "Could not save ro.xlsx: Zenkai can't write to this location (read-only or no permission)."
        );
        assert!(failure.detail.contains("data/ro.xlsx"));
        assert!(failure.detail.contains("os error 5"));
        assert!(!failure.text.contains("os error"));
    }

    #[test]
    fn offering_another_location_keeps_the_detail() {
        let error = write_error(ErrorKind::StorageFull, "a.xlsx");
        let plain = save_failure(Path::new("a.xlsx"), &error);
        let detail = plain.detail.clone();
        let offered = offer_another_location(plain);
        assert!(offered.text.starts_with("Could not save a.xlsx:"));
        assert!(
            offered
                .text
                .ends_with("Pick another location to keep your changes.")
        );
        assert_eq!(offered.detail, detail);
    }

    #[test]
    fn an_open_error_names_the_file_not_the_folder() {
        let failure = open_failure(
            Path::new("data/report.xlsx"),
            &LoadFailure::Engine(EngineError::InvalidFile("Could not find EOCD".into())),
        );
        assert!(failure.text.starts_with("Could not open report.xlsx: "));
        assert!(failure.text.contains("Could not find EOCD"));
        assert!(!failure.text.contains("data/"));
        assert!(
            failure
                .detail
                .starts_with("Could not open data/report.xlsx: ")
        );
    }

    #[test]
    fn a_missing_file_is_named_by_its_file_name() {
        let failure = open_failure(Path::new("data/gone.xlsx"), &LoadFailure::Missing);
        assert_eq!(failure.text, "gone.xlsx was not found.");
        assert_eq!(failure.detail, "data/gone.xlsx was not found.");
    }

    #[test]
    fn an_unreadable_file_reports_both_reasons_under_its_name() {
        let failure = open_failure(
            Path::new("data/bad.xlsx"),
            &LoadFailure::Unreadable {
                reason: "Could not find EOCD".into(),
                fallback: "no sheets".into(),
            },
        );
        assert!(
            failure
                .text
                .starts_with("Could not open bad.xlsx: Could not find EOCD.")
        );
        assert!(failure.text.ends_with("no sheets"));
    }
}
