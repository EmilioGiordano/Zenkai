use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use zenkai_agent::preferences::XlsxReaderChoice;
use zenkai_agent::protected_view::{FileOrigin, file_origin};
use zenkai_engine::{
    Engine, EngineError, ShownCells, Unsupported, Workbook, XlsxReader, open_xlsx_cancellable,
    run_with_engine_stack, write_atomic_streaming,
};
use zenkai_formats::{CsvError, CsvWriter, Delimiter, ParsedCsv, parse_csv, read_values};
use zenkai_i18n::t;
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

use crate::document::read_shared;
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

pub fn read_csv(path: &Path) -> Result<(Vec<u8>, ParsedCsv), String> {
    let bytes = std::fs::read(path)
        .map_err(|e| t!("file.read_failed", path = path.display(), error = e))?;
    let hint = (extension(path).as_deref() == Some("tsv")).then_some(Delimiter::Tab);
    let parsed = parse_csv(&bytes, hint).map_err(|e| e.to_string())?;
    Ok((bytes, parsed))
}

pub fn workbook_from_rows(rows: Vec<Vec<String>>) -> Result<Workbook, String> {
    run_with_engine_stack(move || {
        let mut workbook = Workbook::new_empty()?;
        workbook.set_inputs(SheetId(0), CellPos::default(), &rows)?;
        workbook.warm_used_areas();
        Ok(workbook)
    })
    .map_err(|e| e.to_string())
}

pub struct FileLoad {
    pub workbook: Workbook,
    pub unsupported: Vec<Unsupported>,
    pub read_only: bool,
    pub origin: FileOrigin,
    pub reader_fallback: Option<String>,
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
        }),
        Err(EngineError::InvalidFile(reason)) => match open_values(path) {
            Ok(workbook) => Ok(FileLoad {
                workbook,
                unsupported: Vec::new(),
                read_only: true,
                origin: file_origin(path),
                reader_fallback: None,
            }),
            Err(fallback) => Err(LoadFailure::Unreadable { reason, fallback }),
        },
        Err(EngineError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(LoadFailure::Missing)
        }
        Err(error) => Err(LoadFailure::Engine(error)),
    }
}

const MAX_VALUES_FILE_BYTES: u64 = 512 * 1024 * 1024;

// Plan B when the engine cannot open a file (or it is .xls/.ods): only the values,
// read by calamine, in a workbook that is never saved over the original.
pub fn open_values(path: &Path) -> Result<Workbook, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| t!("file.read_failed", path = path.display(), error = e))?
        .len();
    if size > MAX_VALUES_FILE_BYTES {
        return Err(t!("file.too_large", path = path.display()));
    }
    let path = path.to_path_buf();
    run_with_engine_stack(move || {
        let sheets = read_values(&path).map_err(|e| EngineError::InvalidFile(e.to_string()))?;
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

// The read lock is held only while the shown cells are gathered; the write can take far
// longer. Memory follows the cells, never the area up to the sheet's end.
pub fn gather_export(shared: &RwLock<Workbook>, sheet: SheetId) -> Result<ShownCells, EngineError> {
    read_shared(shared).shown_cells(sheet)
}

pub fn write_csv_file(
    path: &Path,
    shown: &ShownCells,
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    let delimiter = if extension(path).as_deref() == Some("tsv") {
        Delimiter::Tab
    } else {
        Delimiter::Comma
    };
    write_atomic_streaming(path, |out| {
        write_shown_rows(out, shown, delimiter, cancel).map_err(|error| match error {
            ExportStop::Cancelled => EngineError::Cancelled,
            ExportStop::Csv(CsvError::Write(source)) => EngineError::Write {
                path: path.to_path_buf(),
                source,
            },
            ExportStop::Csv(other) => EngineError::Rejected(other.to_string()),
        })
    })
}

enum ExportStop {
    Cancelled,
    Csv(CsvError),
}

impl From<CsvError> for ExportStop {
    fn from(error: CsvError) -> Self {
        ExportStop::Csv(error)
    }
}

// Every row up to the sheet's end has as many fields as the sheet has columns, as when
// the whole rectangle was written; gaps are written without building a string each.
fn write_shown_rows(
    out: &mut dyn Write,
    shown: &ShownCells,
    delimiter: Delimiter,
    cancel: &AtomicBool,
) -> Result<(), ExportStop> {
    let width = usize::from(shown.end.col.get()) + 1;
    let mut writer = CsvWriter::new(out, delimiter)?;
    let mut cells = shown.cells.iter().peekable();
    for row in 0..=shown.end.row.get() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportStop::Cancelled);
        }
        let mut next_col = 0;
        while let Some((pos, text)) = cells.next_if(|(pos, _)| pos.row.get() == row) {
            let col = usize::from(pos.col.get());
            writer.write_empty_fields(col.saturating_sub(next_col))?;
            writer.write_field(text)?;
            next_col = col + 1;
        }
        writer.write_empty_fields(width.saturating_sub(next_col))?;
        writer.end_row()?;
    }
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_types::{ColIdx, Contents, RowIdx};

    fn number_at(workbook: &Workbook, col: u16) -> Option<f64> {
        let pos = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(i64::from(col)));
        match workbook.contents(SheetId(0)).unwrap()(pos) {
            Contents::Number(n) => Some(n),
            _ => None,
        }
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
    fn csv_import_then_export_keeps_values() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("in.csv");
        std::fs::write(&source, "name;qty\nAna;3\nLuis;4\n").unwrap();
        let (_, parsed) = read_csv(&source).unwrap();
        assert_eq!(parsed.delimiter, Delimiter::Semicolon);
        let workbook = workbook_from_rows(parsed.rows).unwrap();
        let target = dir.path().join("out.csv");
        let shown = workbook.shown_cells(SheetId(0)).unwrap();
        write_csv_file(&target, &shown, &AtomicBool::new(false)).unwrap();
        let written = std::fs::read(&target).unwrap();
        assert_eq!(&written[3..], b"name,qty\r\nAna,3\r\nLuis,4\r\n");
    }

    fn at(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    fn sheet_with(cells: &[(CellPos, &str)]) -> Workbook {
        let mut workbook = Workbook::new_empty().unwrap();
        for (pos, text) in cells {
            workbook.set_input(SheetId(0), *pos, text).unwrap();
        }
        workbook
    }

    // The export before it streamed: one string per cell of the used rectangle.
    fn whole_rectangle_export(workbook: &Workbook, delimiter: Delimiter) -> Vec<u8> {
        let end = workbook.used_end(SheetId(0));
        let rows: Vec<Vec<String>> = (0..=i64::from(end.row.get()))
            .map(|row| {
                (0..=i64::from(end.col.get()))
                    .map(|col| workbook.cell(SheetId(0), at(row, col)).text)
                    .collect()
            })
            .collect();
        zenkai_formats::write_csv(&rows, delimiter).unwrap()
    }

    fn exported(workbook: Workbook, name: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(name);
        let shown = gather_export(&RwLock::new(workbook), SheetId(0)).unwrap();
        write_csv_file(&target, &shown, &AtomicBool::new(false)).unwrap();
        std::fs::read(&target).unwrap()
    }

    #[test]
    fn streamed_export_writes_the_bytes_of_the_whole_rectangle() {
        let sheets: Vec<(&str, Vec<(CellPos, &str)>)> = vec![
            (
                "gaps.csv",
                vec![
                    (at(0, 0), "name"),
                    (at(0, 3), "a,b"),
                    (at(1, 1), "say \"hi\""),
                    (at(3, 0), "@user"),
                    (at(3, 2), "=SEQUENCE(3)"),
                    (at(4, 4), "-x"),
                ],
            ),
            (
                "one column.csv",
                vec![(at(0, 0), "top"), (at(3, 0), "bottom")],
            ),
            ("tabs.tsv", vec![(at(0, 1), "a\tb"), (at(2, 0), "=1/0")]),
            ("empty.csv", vec![]),
        ];
        for (name, cells) in sheets {
            let workbook = sheet_with(&cells);
            let delimiter = if name.ends_with(".tsv") {
                Delimiter::Tab
            } else {
                Delimiter::Comma
            };
            let expected = whole_rectangle_export(&workbook, delimiter);
            assert_eq!(exported(workbook, name), expected, "{name}");
        }
    }

    #[test]
    fn a_cancelled_export_removes_its_temp_and_keeps_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.csv");
        std::fs::write(&target, "old contents").unwrap();
        let workbook = sheet_with(&[(at(0, 0), "a"), (at(5, 3), "b")]);
        let shown = gather_export(&RwLock::new(workbook), SheetId(0)).unwrap();
        let result = write_csv_file(&target, &shown, &AtomicBool::new(true));
        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert_eq!(std::fs::read(&target).unwrap(), b"old contents");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn a_cancelled_export_to_a_new_name_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new.csv");
        let workbook = sheet_with(&[(at(0, 0), "a")]);
        let shown = gather_export(&RwLock::new(workbook), SheetId(0)).unwrap();
        let result = write_csv_file(&target, &shown, &AtomicBool::new(true));
        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn spills_gaps_and_empty_rows_export_as_the_sheet_shows_them() {
        let workbook = sheet_with(&[(at(0, 0), "=SEQUENCE(3)"), (at(1, 2), "x")]);
        assert_eq!(
            &exported(workbook, "spill.csv")[3..],
            b"1,,\r\n2,,x\r\n3,,\r\n"
        );
        let workbook = sheet_with(&[(at(0, 0), "top"), (at(2, 0), "bottom")]);
        assert_eq!(
            &exported(workbook, "column.csv")[3..],
            b"top\r\n\"\"\r\nbottom\r\n"
        );
    }

    #[test]
    fn a_sheet_ending_at_the_last_row_exports_every_row_quickly() {
        let last_row = i64::from(zenkai_types::MAX_ROWS - 1);
        let workbook = sheet_with(&[(at(0, 0), "first"), (at(last_row, 1), "last")]);
        let started = Instant::now();
        let bytes = exported(workbook, "tall.csv");
        assert!(started.elapsed() < std::time::Duration::from_secs(120));
        let lines = bytes.iter().filter(|byte| **byte == b'\n').count();
        assert_eq!(lines, zenkai_types::MAX_ROWS as usize);
        assert!(bytes[3..].starts_with(b"first,\r\n,\r\n"));
        assert!(bytes.ends_with(b",\r\n,last\r\n"));
    }

    #[test]
    fn a_sheet_reaching_the_last_column_exports_full_width_rows() {
        let last_col = i64::from(zenkai_types::MAX_COLS - 1);
        let workbook = sheet_with(&[(at(0, 0), "first"), (at(49, last_col), "last")]);
        let bytes = exported(workbook, "wide.csv");
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        let lines: Vec<&str> = text.split_terminator("\r\n").collect();
        let gap = ",".repeat(usize::from(zenkai_types::MAX_COLS) - 1);
        assert_eq!(lines.len(), 50);
        assert_eq!(lines[0], format!("first{gap}"));
        assert_eq!(lines[1], gap);
        assert_eq!(lines[49], format!("{gap}last"));
    }

    #[test]
    fn decimal_comma_fields_import_as_numbers() {
        let mut rows = vec![vec!["1.234,5".to_string(), "=A1*2".to_string()]];
        zenkai_formats::normalize_decimal_comma(&mut rows);
        let workbook = workbook_from_rows(rows).unwrap();
        let doubled = number_at(&workbook, 1);
        assert_eq!(doubled, Some(2469.0));
    }

    #[test]
    fn day_first_dates_import_as_the_right_day() {
        let mut rows = vec![vec!["08/10/2026".to_string(), "25/10/2026".to_string()]];
        zenkai_formats::normalize_day_first(&mut rows);
        let workbook = workbook_from_rows(rows).unwrap();
        let serial = |col| number_at(&workbook, col);
        // 8 October 2026 and 25 October 2026 as Excel serials.
        assert_eq!(serial(0), Some(46303.0));
        assert_eq!(serial(1), Some(46320.0));
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
