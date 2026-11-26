use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use zenkai_agent::preferences::XlsxReaderChoice;
use zenkai_agent::protected_view::{FileOrigin, file_origin};
use zenkai_engine::{
    Engine, EngineError, Unsupported, Workbook, XlsxReader, open_xlsx_cancellable,
    run_with_engine_stack,
};
use zenkai_formats::{Delimiter, tsv_delimiter};
use zenkai_i18n::t;
use zenkai_pdf_host::process::host_next_to_exe;
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};
use zenkai_values_host::client::{HOST_FILE, ReadError, ValuesClient};
use zenkai_values_host::wire::FailureKind;

use crate::entry::CsvLoad;
use crate::text_document::DocumentKind;

pub const SPREADSHEET_EXTENSIONS: [&str; 7] = ["xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"];
const REPLACED_EXTENSIONS: [&str; 4] = ["xlsm", "xls", "xlsb", "ods"];

pub fn xlsx_target(chosen: &Path) -> PathBuf {
    let extension = chosen
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("xlsx") => chosen.to_path_buf(),
        Some(ext) if REPLACED_EXTENSIONS.contains(&ext) => chosen.with_extension("xlsx"),
        _ => {
            let mut name = chosen.as_os_str().to_owned();
            name.push(".xlsx");
            PathBuf::from(name)
        }
    }
}

const COPY_MARKER: &str = " (Zenkai)";

// The name Save As suggests for a copy; a copy of a copy, even renamed, keeps one marker.
pub fn copy_stem(stem: &str) -> String {
    if stem.contains(COPY_MARKER) {
        stem.to_string()
    } else {
        format!("{stem}{COPY_MARKER}")
    }
}

#[derive(Clone, Copy)]
pub enum Unresolved {
    ComparePaths,
    // For callers that must not act on a file unless they are sure it is another one.
    AssumeSame,
}

pub fn same_file(a: &Path, b: &Path, unresolved: Unresolved) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => match unresolved {
            Unresolved::ComparePaths => a == b,
            Unresolved::AssumeSame => true,
        },
    }
}

pub struct TargetProbe {
    pub same_as_source: bool,
    pub exists: bool,
}

// Filesystem questions about a Save As target; blocks on a slow share, so call it off the UI thread.
pub fn probe_target(source: Option<&Path>, target: &Path) -> TargetProbe {
    let same_as_source =
        source.is_some_and(|source| same_file(source, target, Unresolved::ComparePaths));
    TargetProbe {
        same_as_source,
        exists: !same_as_source && target.exists(),
    }
}

pub fn is_remote_or_device(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(
            prefix.kind(),
            std::path::Prefix::UNC(..)
                | std::path::Prefix::VerbatimUNC(..)
                | std::path::Prefix::DeviceNS(_)
                | std::path::Prefix::Verbatim(_)
        )
    )
}

// Case, separators, `.` and `..` do not make two spellings different files. Purely lexical:
// it never reads the disk, so it is safe on the UI thread.
pub fn same_path(a: &Path, b: &Path) -> bool {
    normalized(a) == normalized(b)
}

fn normalized(path: &Path) -> Vec<(bool, String)> {
    let fold = |text: &std::ffi::OsStr| {
        let text = text.to_string_lossy();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text.into_owned()
        }
    };
    let mut parts: Vec<(bool, String)> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => parts.push((false, fold(prefix.as_os_str()))),
            Component::RootDir => parts.push((false, "/".to_string())),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|(is_name, _)| *is_name) {
                    parts.pop();
                } else {
                    parts.push((false, "..".to_string()));
                }
            }
            Component::Normal(name) => parts.push((true, fold(name))),
        }
    }
    parts
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

pub fn is_openable(path: &Path) -> bool {
    is_delimited_text(path)
        || DocumentKind::of(path).is_text()
        || DocumentKind::of(path).is_pdf()
        || extension(path)
            .is_some_and(|extension| SPREADSHEET_EXTENSIONS.contains(&extension.as_str()))
}

pub fn is_delimited_text(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("csv" | "tsv" | "txt"))
}

// Only csv and tsv open as themselves; a txt is imported into a new workbook.
pub fn opens_as_itself(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("csv" | "tsv"))
}

pub fn export_delimiter(path: &Path) -> Delimiter {
    tsv_delimiter(path).unwrap_or(Delimiter::Comma)
}

pub struct FileLoad {
    pub workbook: Workbook,
    pub unsupported: Vec<Unsupported>,
    pub read_only: bool,
    pub origin: FileOrigin,
    pub reader_fallback: Option<String>,
    pub csv: Option<CsvLoad>,
}

// ZENKAI_XLSX_READER=fast, shadow or standard overrides the setting, for testing.
pub fn xlsx_reader(choice: XlsxReaderChoice) -> XlsxReader {
    match std::env::var("ZENKAI_XLSX_READER").as_deref() {
        Ok("fast") => XlsxReader::Fast,
        Ok("shadow") => XlsxReader::Shadow,
        Ok("standard") => XlsxReader::IronCalc,
        _ => match choice {
            XlsxReaderChoice::Standard => XlsxReader::IronCalc,
            XlsxReaderChoice::Fast => XlsxReader::Fast,
        },
    }
}

#[derive(Debug)]
pub enum LoadFailure {
    Missing,
    Engine(EngineError),
    Unreadable { reason: String, fallback: String },
}

pub fn load_workbook(
    path: &Path,
    reader: XlsxReader,
    cancel: &Arc<AtomicBool>,
) -> Result<FileLoad, LoadFailure> {
    let started = Instant::now();
    let loaded = tracing::info_span!("open_workbook").in_scope(|| load_any(path, reader, cancel));
    let elapsed_ms = started.elapsed().as_millis();
    match &loaded {
        Ok(load) => tracing::info!(
            ?path,
            elapsed_ms,
            read_only = load.read_only,
            "workbook opened"
        ),
        Err(LoadFailure::Engine(EngineError::Cancelled)) => {
            tracing::info!(?path, elapsed_ms, "workbook open cancelled")
        }
        Err(LoadFailure::Missing) => {
            tracing::warn!(?path, elapsed_ms, "workbook to open is missing")
        }
        Err(LoadFailure::Engine(error)) => {
            tracing::warn!(?path, elapsed_ms, %error, "workbook could not be opened")
        }
        Err(LoadFailure::Unreadable { reason, fallback }) => {
            tracing::warn!(?path, elapsed_ms, %reason, %fallback, "workbook is unreadable")
        }
    }
    loaded
}

fn load_any(
    path: &Path,
    reader: XlsxReader,
    cancel: &Arc<AtomicBool>,
) -> Result<FileLoad, LoadFailure> {
    match open_xlsx_cancellable(path, reader, cancel) {
        Ok(opened) => Ok(FileLoad {
            workbook: opened.workbook,
            unsupported: opened.unsupported,
            read_only: false,
            origin: file_origin(path),
            reader_fallback: opened.fallback,
            csv: None,
        }),
        Err(EngineError::InvalidFile(_)) if cancel.load(Ordering::Relaxed) => {
            Err(LoadFailure::Engine(EngineError::Cancelled))
        }
        Err(EngineError::InvalidFile(reason)) => match open_values(path, cancel) {
            Ok(workbook) => Ok(FileLoad {
                workbook,
                unsupported: Vec::new(),
                read_only: true,
                origin: file_origin(path),
                reader_fallback: None,
                csv: None,
            }),
            Err(_) if cancel.load(Ordering::Relaxed) => {
                Err(LoadFailure::Engine(EngineError::Cancelled))
            }
            Err(fallback) => Err(LoadFailure::Unreadable { reason, fallback }),
        },
        Err(EngineError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(LoadFailure::Missing)
        }
        Err(error) => Err(LoadFailure::Engine(error)),
    }
}

const MAX_VALUES_FILE_BYTES: u64 = 512 * 1024 * 1024;

fn values_error_text(error: &ReadError) -> String {
    match error {
        ReadError::TimedOut => t!("file.values_timed_out").to_string(),
        ReadError::HostMissing | ReadError::StartFailed(_) => {
            t!("file.values_host_unavailable").to_string()
        }
        ReadError::TooLarge => t!("file.values_too_many_cells").to_string(),
        ReadError::Failed(failure) if failure.kind == FailureKind::TooLarge => {
            t!("file.values_too_many_cells").to_string()
        }
        ReadError::Cancelled | ReadError::Failed(_) | ReadError::HostBroken(_) => {
            t!("file.values_unreadable").to_string()
        }
    }
}

// Plan B when the engine cannot open a file (or it is .xls/.ods): only the values,
// read by calamine in a child process, in a workbook that is never saved over the original.
pub fn open_values(path: &Path, cancel: &AtomicBool) -> Result<Workbook, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| t!("file.read_failed", path = path.display(), error = e))?
        .len();
    if size > MAX_VALUES_FILE_BYTES {
        return Err(t!("file.too_large", path = path.display()));
    }
    let sheets = ValuesClient::new(host_next_to_exe(HOST_FILE))
        .read(path, cancel)
        .map_err(|error| {
            tracing::warn!(%error, "values host read failed");
            values_error_text(&error)
        })?;
    run_with_engine_stack(move || {
        let mut workbook = Workbook::new_empty()?;
        for (index, sheet) in (0u32..).zip(&sheets) {
            if index > 0 {
                workbook.add_sheet()?;
            }
            if let Err(error) = workbook.rename_sheet(SheetId(index), &sheet.name) {
                tracing::warn!(%error, name = %sheet.name, "kept the default sheet name");
            }
            for block in &sheet.blocks {
                let origin = CellPos::new(
                    RowIdx::clamped(i64::from(block.first_row)),
                    ColIdx::clamped(i64::from(block.first_col)),
                );
                workbook.set_inputs(SheetId(index), origin, &block.rows)?;
            }
        }
        workbook.warm_used_areas();
        Ok(workbook)
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_csv_and_tsv_open_as_themselves_and_a_txt_is_still_imported() {
        for name in ["a.csv", "a.TSV"] {
            assert!(opens_as_itself(Path::new(name)), "{name}");
        }
        assert!(!opens_as_itself(Path::new("a.txt")));
        assert!(is_delimited_text(Path::new("a.txt")));
        assert!(is_openable(Path::new("a.txt")));
    }

    #[test]
    fn a_cancelled_open_of_an_unreadable_file_does_not_fall_back_to_a_second_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.xlsx");
        std::fs::write(&path, b"not a zip").unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let loaded = load_any(&path, XlsxReader::Fast, &cancel);
        assert!(matches!(
            loaded,
            Err(LoadFailure::Engine(EngineError::Cancelled))
        ));
    }

    #[test]
    fn keeps_xlsx_and_never_drops_a_dotted_name() {
        assert_eq!(
            xlsx_target(Path::new("a/book.xlsx")),
            Path::new("a/book.xlsx")
        );
        assert_eq!(
            xlsx_target(Path::new("a/book.XLSM")),
            Path::new("a/book.xlsx")
        );
        assert_eq!(
            xlsx_target(Path::new("a/report.v2")),
            Path::new("a/report.v2.xlsx")
        );
        assert_eq!(xlsx_target(Path::new("a/book")), Path::new("a/book.xlsx"));
    }

    #[test]
    fn a_copy_name_carries_the_marker_once() {
        assert_eq!(copy_stem("Report"), "Report (Zenkai)");
        assert_eq!(copy_stem("Report (Zenkai)"), "Report (Zenkai)");
        assert_eq!(copy_stem("Report (Zenkai) v2"), "Report (Zenkai) v2");
    }

    #[test]
    fn local_paths_are_probed() {
        assert!(!is_remote_or_device(Path::new("/data/a.xlsx")));
        assert!(!is_remote_or_device(Path::new("relative/a.xlsx")));
    }

    #[cfg(windows)]
    #[test]
    fn network_and_device_paths_are_not_probed() {
        assert!(is_remote_or_device(Path::new(r"\\host\share\a.xlsx")));
        assert!(is_remote_or_device(Path::new(r"\\.\pipe\x")));
        assert!(!is_remote_or_device(Path::new(r"C:\data\a.xlsx")));
    }

    #[test]
    fn same_path_ignores_separators_dots_and_on_windows_case() {
        assert!(same_path(
            Path::new("a/b/../c.xlsx"),
            Path::new("./a/c.xlsx")
        ));
        assert!(!same_path(Path::new("a/c.xlsx"), Path::new("a/d.xlsx")));
        assert!(!same_path(Path::new("../c.xlsx"), Path::new("c.xlsx")));
        if cfg!(windows) {
            assert!(same_path(
                Path::new("C:/Data/Sales.xlsx"),
                Path::new(r"c:\data\sales.XLSX")
            ));
        }
    }

    #[test]
    fn same_file_sees_through_different_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.xlsx");
        std::fs::write(&file, b"x").unwrap();
        let other = dir.path().join(".").join("Book.xlsx");
        assert!(same_file(&file, &other, Unresolved::ComparePaths));
        assert!(!same_file(
            &file,
            &dir.path().join("Other.xlsx"),
            Unresolved::ComparePaths
        ));
    }

    #[test]
    fn probing_a_target_tells_the_source_from_another_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Book.xlsx");
        let other = dir.path().join("Other.xlsx");
        std::fs::write(&source, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let spelled = dir.path().join(".").join("Book.xlsx");

        let same = probe_target(Some(&source), &spelled);
        assert!(same.same_as_source && !same.exists);
        let existing = probe_target(Some(&source), &other);
        assert!(!existing.same_as_source && existing.exists);
        let fresh = probe_target(Some(&source), &dir.path().join("New.xlsx"));
        assert!(!fresh.same_as_source && !fresh.exists);
        let unsaved = probe_target(None, &other);
        assert!(!unsaved.same_as_source && unsaved.exists);
    }
}
