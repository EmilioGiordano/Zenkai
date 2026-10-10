use std::path::{Component, Path, PathBuf};

use zenkai_agent::preferences::XlsxReaderChoice;
use zenkai_agent::protected_view::{FileOrigin, file_origin};
use zenkai_engine::{
    Engine, EngineError, Unsupported, Workbook, XlsxReader, open_xlsx_with, run_with_engine_stack,
    write_atomic,
};
use zenkai_formats::{Delimiter, ParsedCsv, parse_csv, read_values, write_csv};
use zenkai_i18n::t;
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

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

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
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

pub fn load_workbook(path: &Path, reader: XlsxReader) -> Result<FileLoad, LoadFailure> {
    match open_xlsx_with(path, reader) {
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

pub fn sheet_rows(workbook: &Workbook, sheet: SheetId) -> Vec<Vec<String>> {
    let end = workbook.used_end(sheet);
    (0..=end.row.get())
        .map(|row| {
            (0..=end.col.get())
                .map(|col| {
                    let pos = CellPos::new(
                        RowIdx::clamped(i64::from(row)),
                        ColIdx::clamped(i64::from(col)),
                    );
                    workbook.cell(sheet, pos).text
                })
                .collect()
        })
        .collect()
}

pub fn write_csv_file(path: &Path, rows: &[Vec<String>]) -> Result<(), String> {
    let delimiter = if extension(path).as_deref() == Some("tsv") {
        Delimiter::Tab
    } else {
        Delimiter::Comma
    };
    let bytes = write_csv(rows, delimiter).map_err(|e| e.to_string())?;
    write_atomic(path, &bytes, |written| {
        parse_csv(written, Some(delimiter))
            .map(|_| ())
            .map_err(|e| EngineError::VerifyFailed(e.to_string()))
    })
    .map_err(|e| t!("file.write_failed", path = path.display(), error = e))
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
    fn csv_import_then_export_keeps_values() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("in.csv");
        std::fs::write(&source, "name;qty\nAna;3\nLuis;4\n").unwrap();
        let (_, parsed) = read_csv(&source).unwrap();
        assert_eq!(parsed.delimiter, Delimiter::Semicolon);
        let workbook = workbook_from_rows(parsed.rows).unwrap();
        let rows = sheet_rows(&workbook, SheetId(0));
        assert_eq!(
            rows,
            vec![vec!["name", "qty"], vec!["Ana", "3"], vec!["Luis", "4"]]
        );
        let target = dir.path().join("out.csv");
        write_csv_file(&target, &rows).unwrap();
        let written = std::fs::read(&target).unwrap();
        assert_eq!(&written[3..], b"name,qty\r\nAna,3\r\nLuis,4\r\n");
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
    fn network_and_device_paths_are_not_probed() {
        assert!(is_remote_or_device(Path::new(r"\\host\share\a.xlsx")));
        assert!(is_remote_or_device(Path::new(r"\\.\pipe\x")));
        assert!(!is_remote_or_device(Path::new(r"C:\data\a.xlsx")));
        assert!(!is_remote_or_device(Path::new("relative/a.xlsx")));
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
        assert!(same_file(&file, &other));
        assert!(!same_file(&file, &dir.path().join("Other.xlsx")));
    }
}
