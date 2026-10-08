use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use crate::error::EngineError;
use crate::preflight;
use crate::workbook::{Engine, Workbook};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unsupported {
    Charts,
    Images,
    PivotTables,
    Macros,
    Comments,
    Tables,
    Hyperlinks,
    DataValidation,
    ExternalLinks,
    AutoFilter,
    SheetProtection,
}

impl Unsupported {
    pub fn label(self) -> &'static str {
        match self {
            Unsupported::Charts => "charts",
            Unsupported::Images => "images and shapes",
            Unsupported::PivotTables => "pivot tables",
            Unsupported::Macros => "macros (VBA)",
            Unsupported::Comments => "comments",
            Unsupported::Tables => "Excel tables",
            Unsupported::Hyperlinks => "hyperlinks",
            Unsupported::DataValidation => "data validation",
            Unsupported::ExternalLinks => "links to other workbooks",
            Unsupported::AutoFilter => "filters (AutoFilter)",
            Unsupported::SheetProtection => "sheet protection",
        }
    }
}

pub struct Opened {
    pub workbook: Workbook,
    pub unsupported: Vec<Unsupported>,
}

pub fn open_xlsx(path: &Path) -> Result<Opened, EngineError> {
    let read_error = |source| EngineError::Read {
        path: path.to_path_buf(),
        source,
    };
    let size = fs::metadata(path).map_err(read_error)?.len();
    if size > preflight::MAX_FILE_BYTES {
        return Err(EngineError::Unsafe(format!(
            "the file is {} MB, larger than the {} MB supported",
            size / 1024 / 1024,
            preflight::MAX_FILE_BYTES / 1024 / 1024
        )));
    }
    let bytes = fs::read(path).map_err(read_error)?;
    let unsupported = scan_unsupported(&bytes)?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Book".to_string());
    let workbook = load_guarded(&bytes, &name)?;
    Ok(Opened {
        workbook,
        unsupported,
    })
}

// The engine's importer can panic on malformed parts; a broken file must
// become an error message, never take the application down.
fn load_guarded(bytes: &[u8], name: &str) -> Result<Workbook, EngineError> {
    preflight::run_with_engine_stack(|| Workbook::from_xlsx_bytes(bytes, name))
}

pub fn scan_unsupported(bytes: &[u8]) -> Result<Vec<Unsupported>, EngineError> {
    let invalid = |e: zip::result::ZipError| EngineError::InvalidFile(e.to_string());
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    if archive.len() > preflight::MAX_ENTRIES {
        return Err(EngineError::Unsafe(format!(
            "{} entries in the archive",
            archive.len()
        )));
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(invalid)?;
        if entry.size() > preflight::MAX_ENTRY_BYTES {
            return Err(EngineError::Unsafe(format!(
                "part {} expands to {} MB",
                entry.name(),
                entry.size() / 1024 / 1024
            )));
        }
        total = total.saturating_add(entry.size());
    }
    if total > preflight::MAX_TOTAL_BYTES {
        return Err(EngineError::Unsafe(format!(
            "the workbook expands to {} MB",
            total / 1024 / 1024
        )));
    }
    let mut found = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(invalid)?;
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
            ("xl/tables/", Unsupported::Tables),
        ];
        for (prefix, kind) in by_name {
            if name.starts_with(prefix) {
                found.push(kind);
            }
        }
        if entry.is_dir() {
            continue;
        }
        let mut part = Vec::new();
        (&mut entry)
            .take(preflight::MAX_ENTRY_BYTES)
            .read_to_end(&mut part)
            .map_err(|e| EngineError::InvalidFile(e.to_string()))?;
        let features = preflight::check_part(&part)?;
        if features.hyperlinks {
            found.push(Unsupported::Hyperlinks);
        }
        if features.data_validation {
            found.push(Unsupported::DataValidation);
        }
        if features.auto_filter {
            found.push(Unsupported::AutoFilter);
        }
        if features.protection {
            found.push(Unsupported::SheetProtection);
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

// Write to a sibling temp file, prove it reopens, then replace: the original is
// never truncated and survives any failure before the final rename.
pub fn save_xlsx_atomic(workbook: &Workbook, path: &Path) -> Result<(), EngineError> {
    let bytes = preflight::run_with_engine_stack(|| workbook.to_xlsx())?;
    write_atomic(path, &bytes, |written| {
        load_guarded(written, "verify").map(|_| ())
    })
}

// Every write over a user file goes through here: sibling temp file, read back
// and verified, then renamed over the target.
pub fn write_atomic(
    path: &Path,
    bytes: &[u8],
    verify: impl Fn(&[u8]) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let temp = temp_path(path);
    if let Err(source) = write_new(&temp, bytes) {
        if source.kind() != std::io::ErrorKind::AlreadyExists {
            remove_temp(&temp);
        }
        return Err(EngineError::Write { path: temp, source });
    }
    let verified = fs::read(&temp)
        .map_err(|e| EngineError::VerifyFailed(e.to_string()))
        .and_then(|written| {
            if written != bytes {
                return Err(EngineError::VerifyFailed(
                    "the file on disk differs from what was written".to_string(),
                ));
            }
            verify(&written).map_err(|e| EngineError::VerifyFailed(e.to_string()))
        });
    if let Err(error) = verified {
        remove_temp(&temp);
        return Err(error);
    }
    if let Err(source) = fs::rename(&temp, path) {
        remove_temp(&temp);
        return Err(EngineError::Write {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn remove_temp(temp: &Path) {
    if let Err(error) = fs::remove_file(temp) {
        tracing::warn!(?temp, %error, "could not remove temp file after failed save");
    }
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
    fn inputs_that_do_not_fit_are_rejected_not_clamped() {
        let mut book = Workbook::new_empty().unwrap();
        let near_end = CellPos::new(zenkai_types::RowIdx::LAST, zenkai_types::ColIdx::default());
        let rows = vec![vec!["1".to_string()], vec!["2".to_string()]];
        assert!(book.set_inputs(SheetId(0), near_end, &rows).is_err());
    }

    #[test]
    fn oversized_typed_formula_is_rejected() {
        let mut book = Workbook::new_empty().unwrap();
        let formula = format!("={}1", "1+".repeat(5_000));
        assert!(
            book.set_input(SheetId(0), CellPos::default(), &formula)
                .is_err()
        );
        let rows = vec![vec![formula]];
        assert!(
            book.set_inputs(SheetId(0), CellPos::default(), &rows)
                .is_err()
        );
    }

    #[test]
    fn paste_inside_zenkai_shifts_relative_references() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![
            vec!["1".to_string(), "=A1*2".to_string()],
            vec!["5".to_string(), String::new()],
        ];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        let copied = book.copy(sheet, zenkai_types::Range::single(b1)).unwrap();
        assert_eq!(copied.text.trim(), "2");
        let b2 = CellPos::parse_a1("B2").unwrap();
        book.paste(sheet, b2, &copied, false).unwrap();
        assert_eq!(book.input(sheet, b2), "=A2*2");
        assert_eq!(book.cell(sheet, b2).text, "10");
    }

    #[test]
    fn inserting_a_row_shifts_formulas_like_excel() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![vec!["2".to_string()], vec!["=A1*3".to_string()]];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        book.insert_rows(sheet, zenkai_types::RowIdx::default(), 1)
            .unwrap();
        let a3 = CellPos::parse_a1("A3").unwrap();
        assert_eq!(book.input(sheet, a3), "=A2*3");
        assert_eq!(book.cell(sheet, a3).text, "6");
        book.delete_columns(sheet, zenkai_types::ColIdx::default(), 1)
            .unwrap();
        assert_eq!(book.cell(sheet, a3).text, "");
        book.set_frozen(sheet, 1, 2).unwrap();
        assert_eq!(book.frozen(sheet), (1, 2));
    }

    #[test]
    fn fill_down_and_right_copy_with_shifted_references() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![
            vec!["1".to_string(), "=A1*10".to_string()],
            vec!["2".to_string(), String::new()],
            vec!["3".to_string(), String::new()],
        ];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        let b1_b3 = zenkai_types::Range::parse_a1("B1:B3").unwrap();
        book.fill(sheet, b1_b3, true).unwrap();
        let b3 = CellPos::parse_a1("B3").unwrap();
        assert_eq!(book.input(sheet, b3), "=A3*10");
        assert_eq!(book.cell(sheet, b3).text, "30");
        let c1 = zenkai_types::Range::parse_a1("C1").unwrap();
        book.fill(sheet, c1, false).unwrap();
        assert_eq!(
            book.input(sheet, CellPos::parse_a1("C1").unwrap()),
            "=B1*10"
        );
        book.undo().unwrap();
        book.undo().unwrap();
        assert_eq!(
            book.input(sheet, b3),
            "",
            "the whole fill undoes in one step"
        );
    }

    #[test]
    fn set_inputs_keeps_quotes_tabs_newlines_and_blanks() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let tricky = vec![
            vec!["say \"hi\"".to_string(), "a	b".to_string()],
            vec![
                String::new(),
                "line 1
line 2"
                    .to_string(),
            ],
            vec!["last".to_string()],
        ];
        book.set_inputs(sheet, CellPos::default(), &tricky).unwrap();
        let at = |a1: &str| book.input(sheet, CellPos::parse_a1(a1).unwrap());
        assert_eq!(at("A1"), "say \"hi\"");
        assert_eq!(at("B1"), "a	b");
        assert_eq!(at("A2"), "");
        assert_eq!(
            at("B2"),
            "line 1
line 2"
        );
        assert_eq!(at("A3"), "last");
    }

    #[test]
    fn column_width_round_trips_in_pixels() {
        let mut book = Workbook::new_empty().unwrap();
        let col = zenkai_types::ColIdx::new(2).unwrap();
        book.set_column_width(SheetId(0), col, 145.0).unwrap();
        let sizes = book.sizes(SheetId(0));
        let span = sizes
            .columns
            .iter()
            .find(|s| s.first <= col && col <= s.last)
            .unwrap();
        assert!((span.width - 145.0).abs() < 1.0, "{}", span.width);
    }

    #[test]
    fn filled_cells_lists_populated_cells_in_order() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = vec![vec!["1".to_string(), String::new(), "x".to_string()]];
        book.set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let cells: Vec<String> = book
            .filled_cells(SheetId(0))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(cells, ["A1", "C1"]);
    }

    #[test]
    fn plus_and_minus_formulas_get_the_formula_limits() {
        let mut book = Workbook::new_empty().unwrap();
        let deep = format!("+{}1{}", "(".repeat(300), ")".repeat(300));
        assert!(
            book.set_input(SheetId(0), CellPos::default(), &deep)
                .is_err()
        );
        let rows = vec![vec![deep.replacen('+', "-", 1)]];
        assert!(
            book.set_inputs(SheetId(0), CellPos::default(), &rows)
                .is_err()
        );
        book.set_input(SheetId(0), CellPos::default(), "-5")
            .unwrap();
        assert_eq!(book.number(SheetId(0), CellPos::default()), Some(-5.0));
    }

    #[test]
    fn empty_hidden_row_on_a_later_sheet_survives_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("two.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        source.add_worksheet().write_string(0, 0, "first").unwrap();
        let second = source.add_worksheet();
        second.write_string(0, 0, "second").unwrap();
        second.set_row_hidden(4).unwrap();
        second.set_row_height(6, 33).unwrap();
        source.save(&path).unwrap();
        let opened = open_xlsx(&path).unwrap();
        let saved = opened.workbook.to_xlsx().unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(saved.clone())).unwrap();
        let mut xml = String::new();
        archive
            .by_name("xl/worksheets/sheet2.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(
            xml.contains(r#"<row r="5""#) && xml.contains(r#"hidden="1""#),
            "{xml}"
        );
        let reopened = Workbook::from_xlsx_bytes(&saved, "two").unwrap();
        let sizes = reopened.sizes(SheetId(1));
        let row = |r| zenkai_types::RowIdx::new(r).unwrap();
        assert!(sizes.rows.iter().any(|(r, _)| *r == row(4)));
        assert!(sizes.rows.iter().any(|(r, h)| *r == row(6) && *h == 44.0));
        assert_eq!(reopened.input(SheetId(1), CellPos::default()), "second");
    }

    #[test]
    fn row_heights_survive_save_even_on_empty_rows() {
        let mut book = Workbook::new_empty().unwrap();
        // Row 4 stays empty: IronCalc alone drops the height of rows without cells.
        let empty = zenkai_types::RowIdx::new(3).unwrap();
        let filled = zenkai_types::RowIdx::new(9).unwrap();
        book.set_row_height(SheetId(0), empty, 48.0).unwrap();
        book.set_row_height(SheetId(0), filled, 30.0).unwrap();
        let a = |row| CellPos::new(row, zenkai_types::ColIdx::clamped(0));
        book.set_input(SheetId(0), a(filled), "x").unwrap();
        book.set_input(SheetId(0), CellPos::default(), "top")
            .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "Book1").unwrap();
        let sizes = reopened.sizes(SheetId(0));
        let height = |row| sizes.rows.iter().find(|(r, _)| *r == row).map(|(_, h)| *h);
        assert_eq!(height(empty), Some(48.0));
        assert_eq!(height(filled), Some(30.0));
        assert_eq!(reopened.input(SheetId(0), a(filled)), "x");
        assert_eq!(reopened.input(SheetId(0), CellPos::default()), "top");
    }

    #[test]
    fn font_and_fill_colours_apply_and_survive_save() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        book.set_input(SheetId(0), a1, "x").unwrap();
        let range = zenkai_types::Range::single(a1);
        let red = zenkai_types::Rgb(0xFF_00_00);
        let yellow = zenkai_types::Rgb(0xFF_FF_00);
        book.apply_style(
            SheetId(0),
            range,
            zenkai_types::StyleChange::FontColor(Some(red)),
        )
        .unwrap();
        book.apply_style(
            SheetId(0),
            range,
            zenkai_types::StyleChange::Fill(Some(yellow)),
        )
        .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "b").unwrap();
        let style = reopened.cell(SheetId(0), a1).style;
        assert_eq!((style.font_color, style.fill), (Some(red), Some(yellow)));
        book.apply_style(SheetId(0), range, zenkai_types::StyleChange::Fill(None))
            .unwrap();
        assert_eq!(book.cell(SheetId(0), a1).style.fill, None);
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
