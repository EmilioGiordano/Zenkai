use std::path::{Path, PathBuf};

use zenkai_engine::{Engine, EngineError, Workbook, run_with_engine_stack, write_atomic};
use zenkai_formats::{Delimiter, parse_csv, read_values, write_csv};
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

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

pub fn is_delimited_text(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("csv" | "tsv" | "txt"))
}

pub fn import_csv(path: &Path) -> Result<(Workbook, String), String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let hint = (extension(path).as_deref() == Some("tsv")).then_some(Delimiter::Tab);
    let parsed = parse_csv(&bytes, hint).map_err(|e| e.to_string())?;
    let rows = parsed.rows;
    let workbook = run_with_engine_stack(move || {
        let mut workbook = Workbook::new_empty()?;
        workbook.set_inputs(SheetId(0), CellPos::default(), &rows)?;
        Ok(workbook)
    })
    .map_err(|e| e.to_string())?;
    let summary = format!(
        "Imported {} ({}, {}). Save to keep it as .xlsx.",
        path.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        parsed.delimiter.label(),
        parsed.encoding.label()
    );
    Ok((workbook, summary))
}

const MAX_VALUES_FILE_BYTES: u64 = 512 * 1024 * 1024;

// Plan B when the engine cannot open a file (or it is .xls/.ods): only the values,
// read by calamine, in a workbook that is never saved over the original.
pub fn open_values(path: &Path) -> Result<Workbook, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?
        .len();
    if size > MAX_VALUES_FILE_BYTES {
        return Err(format!("{} is larger than 512 MB", path.display()));
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
            let origin = CellPos::new(
                RowIdx::clamped(i64::from(sheet.first_row)),
                ColIdx::clamped(i64::from(sheet.first_col)),
            );
            workbook.set_inputs(SheetId(index), origin, &sheet.rows)?;
        }
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
    .map_err(|e| format!("Could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (workbook, summary) = import_csv(&source).unwrap();
        assert!(summary.contains("semicolon"), "{summary}");
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
    fn same_file_sees_through_different_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.xlsx");
        std::fs::write(&file, b"x").unwrap();
        let other = dir.path().join(".").join("Book.xlsx");
        assert!(same_file(&file, &other));
        assert!(!same_file(&file, &dir.path().join("Other.xlsx")));
    }
}
