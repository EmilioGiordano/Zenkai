use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use crate::error::EngineError;
use crate::workbook::{Engine, Workbook};

const MAX_ENTRIES: usize = 20_000;
const MAX_SCANNED_XML: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unsupported {
    Charts,
    Images,
    PivotTables,
    Macros,
    Comments,
    ConditionalFormatting,
    DataValidation,
    ExternalLinks,
}

impl Unsupported {
    pub fn label(self) -> &'static str {
        match self {
            Unsupported::Charts => "charts",
            Unsupported::Images => "images and shapes",
            Unsupported::PivotTables => "pivot tables",
            Unsupported::Macros => "macros (VBA)",
            Unsupported::Comments => "comments",
            Unsupported::ConditionalFormatting => "conditional formatting",
            Unsupported::DataValidation => "data validation",
            Unsupported::ExternalLinks => "links to other workbooks",
        }
    }
}

pub struct Opened {
    pub workbook: Workbook,
    pub unsupported: Vec<Unsupported>,
}

pub fn open_xlsx(path: &Path) -> Result<Opened, EngineError> {
    let bytes = fs::read(path).map_err(|source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let unsupported = scan_unsupported(&bytes)?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Book".to_string());
    let workbook = Workbook::from_xlsx_bytes(&bytes, &name)?;
    Ok(Opened {
        workbook,
        unsupported,
    })
}

pub fn scan_unsupported(bytes: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| EngineError::InvalidFile(e.to_string()))?;
    if archive.len() > MAX_ENTRIES {
        return Err(EngineError::InvalidFile(format!(
            "{} entries in the archive",
            archive.len()
        )));
    }
    let mut found = Vec::new();
    let mut scanned = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| EngineError::InvalidFile(e.to_string()))?;
        let name = entry.name().to_ascii_lowercase();
        let by_name = [
            ("xl/charts/", Unsupported::Charts),
            ("xl/media/", Unsupported::Images),
            ("xl/drawings/", Unsupported::Images),
            ("xl/pivottables/", Unsupported::PivotTables),
            ("xl/vbaproject", Unsupported::Macros),
            ("xl/comments", Unsupported::Comments),
            ("xl/threadedcomments/", Unsupported::Comments),
            ("xl/externallinks/", Unsupported::ExternalLinks),
        ];
        for (prefix, kind) in by_name {
            if name.starts_with(prefix) {
                found.push(kind);
            }
        }
        if name.starts_with("xl/worksheets/sheet") && scanned < MAX_SCANNED_XML {
            let budget = MAX_SCANNED_XML - scanned;
            let mut xml = Vec::new();
            (&mut entry)
                .take(budget)
                .read_to_end(&mut xml)
                .map_err(|e| EngineError::InvalidFile(e.to_string()))?;
            scanned += xml.len() as u64;
            if contains(&xml, b"<conditionalFormatting") {
                found.push(Unsupported::ConditionalFormatting);
            }
            if contains(&xml, b"<dataValidations") {
                found.push(Unsupported::DataValidation);
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// Write to a sibling temp file, prove it reopens, then replace: the original is
// never truncated and survives any failure before the final rename.
pub fn save_xlsx_atomic(workbook: &Workbook, path: &Path) -> Result<(), EngineError> {
    let bytes = workbook.to_xlsx()?;
    Workbook::from_xlsx_bytes(&bytes, "verify")
        .map_err(|e| EngineError::VerifyFailed(e.to_string()))?;
    let temp = temp_path(path);
    fs::write(&temp, &bytes).map_err(|source| EngineError::Write {
        path: temp.clone(),
        source,
    })?;
    if let Err(source) = fs::rename(&temp, path) {
        if let Err(cleanup) = fs::remove_file(&temp) {
            tracing::warn!(?temp, %cleanup, "could not remove temp file after failed save");
        }
        return Err(EngineError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "book.xlsx".to_string());
    path.with_file_name(format!("~${name}.{}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_types::{CellPos, SheetId};

    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("book.xlsx");
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_number(0, 0, 2.0).unwrap();
        sheet
            .write_formula(1, 0, rust_xlsxwriter::Formula::new("=A1*21").set_result(""))
            .unwrap();
        book.save(&path).unwrap();
        path
    }

    #[test]
    fn open_edit_save_reopen_keeps_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(dir.path());
        let mut opened = open_xlsx(&path).unwrap();
        assert!(opened.unsupported.is_empty());
        let a2 = CellPos::parse_a1("A2").unwrap();
        assert_eq!(opened.workbook.cell(SheetId(0), a2).text, "42");

        opened
            .workbook
            .set_input(SheetId(0), CellPos::parse_a1("A1").unwrap(), "3")
            .unwrap();
        save_xlsx_atomic(&opened.workbook, &path).unwrap();

        let reopened = open_xlsx(&path).unwrap();
        assert_eq!(reopened.workbook.cell(SheetId(0), a2).text, "63");
        assert_eq!(reopened.workbook.input(SheetId(0), a2), "=A1*21");
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "temp file left behind");
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("empty.xlsx", Vec::new()),
            ("text.xlsx", b"not a zip at all".to_vec()),
            ("truncated.xlsx", {
                let full = fs::read(fixture(dir.path())).unwrap();
                full[..full.len() / 2].to_vec()
            }),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            assert!(open_xlsx(&path).is_err(), "{name}");
        }
    }

    #[test]
    fn undo_restores_previous_value() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::parse_a1("A1").unwrap();
        book.set_input(SheetId(0), a1, "1").unwrap();
        book.set_input(SheetId(0), a1, "2").unwrap();
        book.undo().unwrap();
        assert_eq!(book.cell(SheetId(0), a1).text, "1");
        book.redo().unwrap();
        assert_eq!(book.cell(SheetId(0), a1).text, "2");
    }
}
