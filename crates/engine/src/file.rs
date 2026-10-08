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
    fn extend_continues_number_trends_repeats_the_rest_and_undoes_once() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let rows = vec![
            vec!["1".to_string(), "=A1*10".to_string()],
            vec!["2".to_string(), "=A2*10".to_string()],
        ];
        book.set_inputs(SheetId(0), at(0, 0), &rows).unwrap();
        let source = zenkai_types::Range::new(at(0, 0), at(1, 1));
        let target = zenkai_types::Range::new(at(0, 0), at(4, 1));
        book.extend(SheetId(0), source, target).unwrap();
        let column = |c| {
            (0..5)
                .map(|r| book.input(SheetId(0), at(r, c)))
                .collect::<Vec<_>>()
        };
        assert_eq!(column(0), ["1", "2", "3", "4", "5"]);
        assert_eq!(
            column(1),
            ["=A1*10", "=A2*10", "=A3*10", "=A4*10", "=A5*10"]
        );
        book.undo().unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 0)), "");
        let right = zenkai_types::Range::new(at(0, 0), at(1, 3));
        book.extend(SheetId(0), source, right).unwrap();
        assert_eq!(book.input(SheetId(0), at(0, 2)), "1");
        assert_eq!(book.input(SheetId(0), at(1, 3)), "=C2*10");
        book.undo().unwrap();
        let tenths = vec![
            vec!["0.1".to_string()],
            vec!["0.2".to_string()],
            vec!["x".to_string()],
        ];
        book.set_inputs(SheetId(0), at(0, 5), &tenths).unwrap();
        let numbers = zenkai_types::Range::new(at(0, 5), at(1, 5));
        book.extend(
            SheetId(0),
            numbers,
            zenkai_types::Range::new(at(0, 5), at(2, 5)),
        )
        .unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 5)), "0.3");
        let mixed = vec![vec!["a".to_string()], vec!["1".to_string()]];
        book.set_inputs(SheetId(0), at(0, 7), &mixed).unwrap();
        let source = zenkai_types::Range::new(at(0, 7), at(1, 7));
        let target = zenkai_types::Range::new(at(0, 7), at(3, 7));
        book.extend(SheetId(0), source, target).unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 7)), "a");
        assert_eq!(book.input(SheetId(0), at(3, 7)), "1");
        let words = vec![vec!["inf".to_string()], vec!["nan".to_string()]];
        book.set_inputs(SheetId(0), at(0, 9), &words).unwrap();
        let source = zenkai_types::Range::new(at(0, 9), at(1, 9));
        let target = zenkai_types::Range::new(at(0, 9), at(2, 9));
        book.extend(SheetId(0), source, target).unwrap();
        assert_eq!(book.input(SheetId(0), at(2, 9)), "inf");
    }

    #[test]
    fn strike_and_font_size_apply() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        book.set_input(SheetId(0), a1, "x").unwrap();
        let range = zenkai_types::Range::single(a1);
        book.apply_style(SheetId(0), range, zenkai_types::StyleChange::Strike(true))
            .unwrap();
        book.apply_style(SheetId(0), range, zenkai_types::StyleChange::FontSize(14))
            .unwrap();
        let style = book.cell(SheetId(0), a1).style;
        assert!(style.strike);
        assert_eq!(style.font_size, Some(14.0));
    }

    #[test]
    fn border_presets_apply_survive_save_and_clear() {
        use zenkai_types::{BorderPreset, StyleChange};
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let block = zenkai_types::Range::new(at(0, 0), at(1, 1));
        book.apply_style(SheetId(0), block, StyleChange::Borders(BorderPreset::All))
            .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "b").unwrap();
        for pos in [at(0, 0), at(1, 1)] {
            let style = reopened.cell(SheetId(0), pos).style;
            assert!(
                style.border_top && style.border_left && style.border_bottom && style.border_right
            );
        }
        book.apply_style(SheetId(0), block, StyleChange::Borders(BorderPreset::None))
            .unwrap();
        let style = book.cell(SheetId(0), at(1, 1)).style;
        assert!(!style.border_top && !style.border_bottom);
    }

    #[test]
    fn autofill_counts_up_dates_and_numbered_text() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let rows = vec![vec![
            "2026-12-30".to_string(),
            "Item 9".to_string(),
            "Q01".to_string(),
            "hola".to_string(),
        ]];
        book.set_inputs(SheetId(0), at(0, 0), &rows).unwrap();
        let source = zenkai_types::Range::new(at(0, 0), at(0, 3));
        let target = zenkai_types::Range::new(at(0, 0), at(3, 3));
        book.extend(SheetId(0), source, target).unwrap();
        let row = |r| {
            (0..4)
                .map(|c| book.cell(SheetId(0), at(r, c)).text)
                .collect::<Vec<_>>()
        };
        assert_eq!(row(1), ["2026-12-31", "Item 10", "Q02", "hola"]);
        assert_eq!(row(3), ["2027-01-02", "Item 12", "Q04", "hola"]);
    }

    #[test]
    fn sort_orders_rows_like_excel_and_moves_formulas() {
        let mut book = Workbook::new_empty().unwrap();
        let at = |r: i64, c: i64| {
            CellPos::new(
                zenkai_types::RowIdx::clamped(r),
                zenkai_types::ColIdx::clamped(c),
            )
        };
        let rows: Vec<Vec<String>> = [
            ["banana", "=B1"],
            ["", "x"],
            ["10", "y"],
            ["Apple", "z"],
            ["2", "w"],
        ]
        .iter()
        .map(|r| r.iter().map(ToString::to_string).collect())
        .collect();
        book.set_inputs(SheetId(0), at(0, 0), &rows).unwrap();
        let range = zenkai_types::Range::new(at(0, 0), at(4, 1));
        book.sort(SheetId(0), range, zenkai_types::ColIdx::clamped(0), false)
            .unwrap();
        let column = |book: &Workbook, c| {
            (0..5)
                .map(|r| book.input(SheetId(0), at(r, c)))
                .collect::<Vec<_>>()
        };
        assert_eq!(column(&book, 0), ["2", "10", "Apple", "banana", ""]);
        assert_eq!(column(&book, 1), ["w", "y", "z", "=B4", "x"]);
        book.undo().unwrap();
        assert_eq!(book.input(SheetId(0), at(0, 0)), "banana");
        book.sort(SheetId(0), range, zenkai_types::ColIdx::clamped(0), true)
            .unwrap();
        assert_eq!(column(&book, 0), ["banana", "Apple", "10", "2", ""]);
    }

    #[test]
    fn sort_refuses_ranges_with_merged_cells() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        let sheet = source.add_worksheet();
        sheet.write_string(0, 0, "b").unwrap();
        sheet.write_string(1, 0, "a").unwrap();
        sheet
            .merge_range(2, 0, 2, 1, "merged", &rust_xlsxwriter::Format::new())
            .unwrap();
        source.save(&path).unwrap();
        let mut book = open_xlsx(&path).unwrap().workbook;
        let range = zenkai_types::Range::parse_a1("A1:B3").unwrap();
        let sorted = book.sort(SheetId(0), range, zenkai_types::ColIdx::clamped(0), false);
        assert!(sorted.is_err());
        assert_eq!(book.input(SheetId(0), CellPos::default()), "b");
    }

    #[test]
    fn wrap_and_vertical_alignment_load_and_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.xlsx");
        let mut source = rust_xlsxwriter::Workbook::new();
        let format = rust_xlsxwriter::Format::new()
            .set_text_wrap()
            .set_align(rust_xlsxwriter::FormatAlign::Top);
        source
            .add_worksheet()
            .write_string_with_format(0, 0, "long header", &format)
            .unwrap();
        source.save(&path).unwrap();
        let mut book = open_xlsx(&path).unwrap().workbook;
        let style = book.cell(SheetId(0), CellPos::default()).style;
        assert!(style.wrap);
        assert_eq!(style.valign, zenkai_types::VAlign::Top);
        let a1 = zenkai_types::Range::single(CellPos::default());
        book.apply_style(SheetId(0), a1, zenkai_types::StyleChange::Wrap(false))
            .unwrap();
        assert!(!book.cell(SheetId(0), CellPos::default()).style.wrap);
    }

    #[test]
    fn hidden_rows_and_columns_have_no_size_and_unhide() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = zenkai_types::Range::parse_a1("A2:A3").unwrap();
        let cols = zenkai_types::Range::parse_a1("C1:D1").unwrap();
        book.set_rows_hidden(SheetId(0), rows, true).unwrap();
        book.set_columns_hidden(SheetId(0), cols, true).unwrap();
        let sizes = book.sizes(SheetId(0));
        assert!(sizes.rows.iter().any(|(r, h)| r.get() == 1 && *h == 0.0));
        assert!(
            sizes
                .columns
                .iter()
                .any(|s| s.first.get() <= 2 && 2 <= s.last.get() && s.width == 0.0)
        );
        book.set_rows_hidden(SheetId(0), rows, false).unwrap();
        assert!(!book.sizes(SheetId(0)).rows.iter().any(|(_, h)| *h == 0.0));
        book.undo().unwrap();
        assert!(book.sizes(SheetId(0)).rows.iter().any(|(_, h)| *h == 0.0));
    }

    #[test]
    fn scattered_inputs_write_cells_and_recalculate_once_done() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::parse_a1("A1").unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        let c3 = CellPos::parse_a1("C3").unwrap();
        book.set_input(SheetId(0), a1, "1").unwrap();
        book.set_input(SheetId(0), b1, "=A1*2").unwrap();
        let changes = vec![(a1, "5".to_string()), (c3, "hola".to_string())];
        book.set_scattered_inputs(SheetId(0), &changes).unwrap();
        assert_eq!(book.number(SheetId(0), b1), Some(10.0));
        assert_eq!(book.input(SheetId(0), c3), "hola");
        let deep = vec![(c3, format!("={}1{}", "(".repeat(300), ")".repeat(300)))];
        assert!(book.set_scattered_inputs(SheetId(0), &deep).is_err());
    }

    #[test]
    fn format_preview_matches_excel_codes() {
        use crate::workbook::format_preview;
        assert_eq!(format_preview(1234.5, "#,##0.00").unwrap(), "1,234.50");
        assert_eq!(format_preview(0.256, "0.0%").unwrap(), "25.6%");
        assert!(format_preview(1.0, &"0".repeat(256)).is_err());
    }

    #[test]
    fn duplicate_sheet_copies_cells_next_to_the_source() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), CellPos::default(), "hola")
            .unwrap();
        book.add_sheet().unwrap();
        book.duplicate_sheet(SheetId(0)).unwrap();
        let names: Vec<String> = book.sheets().into_iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 3);
        assert!(names[1].starts_with("Sheet1 ("), "{names:?}");
        assert_eq!(book.input(SheetId(1), CellPos::default()), "hola");
        book.undo().unwrap();
        assert_eq!(book.sheets().len(), 2);
        book.redo().unwrap();
        let row = zenkai_types::RowIdx::new(4).unwrap();
        book.set_row_height(SheetId(1), row, 40.0).unwrap();
        book.set_input(
            SheetId(1),
            CellPos::new(row, zenkai_types::ColIdx::clamped(1)),
            "=A1",
        )
        .unwrap();
        let reopened = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "b").unwrap();
        assert_eq!(reopened.sheets().len(), 3);
        assert_eq!(reopened.input(SheetId(1), CellPos::default()), "hola");
        let sizes = reopened.sizes(SheetId(1));
        assert!(sizes.rows.iter().any(|(r, h)| *r == row && *h == 40.0));
    }

    #[test]
    fn clear_formats_keeps_values_and_clear_all_empties() {
        use zenkai_types::{Range, StyleChange};
        let mut book = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        book.set_input(SheetId(0), a1, "7").unwrap();
        book.apply_style(SheetId(0), Range::single(a1), StyleChange::Bold(true))
            .unwrap();
        book.clear_formats(SheetId(0), Range::single(a1)).unwrap();
        assert!(!book.cell(SheetId(0), a1).style.bold);
        assert_eq!(book.input(SheetId(0), a1), "7");
        book.clear_all(SheetId(0), Range::single(a1)).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        book.set_input(SheetId(0), a1, "8").unwrap();
        let sheet = Range::parse_a1("A1:XFD1048576").unwrap();
        book.clear(SheetId(0), sheet).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        book.set_input(SheetId(0), a1, "9").unwrap();
        book.clear_all(SheetId(0), sheet).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        // Clearing far past the data must not create cells that grow the used area.
        book.set_input(SheetId(0), a1, "1").unwrap();
        let before = book.used_end(SheetId(0));
        let far = Range::parse_a1("XFD1048576").unwrap();
        book.clear(SheetId(0), far).unwrap();
        book.clear_all(SheetId(0), far).unwrap();
        assert_eq!(book.used_end(SheetId(0)), before);
        // A formatted far cell makes the used area the whole sheet; Delete over it must
        // still only touch the contents.
        book.apply_style(SheetId(0), far, StyleChange::Bold(true))
            .unwrap();
        book.set_input(SheetId(0), CellPos::parse_a1("XFD1048576").unwrap(), "far")
            .unwrap();
        book.clear(SheetId(0), sheet).unwrap();
        assert_eq!(book.input(SheetId(0), a1), "");
        assert_eq!(
            book.input(SheetId(0), CellPos::parse_a1("XFD1048576").unwrap()),
            ""
        );
        let huge = Range::parse_a1("B2:XFD1048576").unwrap();
        assert!(
            book.apply_style(SheetId(0), huge, StyleChange::Bold(true))
                .is_err()
        );
    }

    #[test]
    fn fill_with_puts_one_entry_in_every_cell_shifting_formulas() {
        let mut book = Workbook::new_empty().unwrap();
        let range = zenkai_types::Range::parse_a1("B1:C2").unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        book.fill_with(SheetId(0), b1, "=A1*2", range).unwrap();
        let column = zenkai_types::Range::parse_a1("D1:D1048576").unwrap();
        assert!(
            book.fill_with(SheetId(0), CellPos::parse_a1("D1").unwrap(), "x", column)
                .is_err()
        );
        assert_eq!(book.input(SheetId(0), CellPos::parse_a1("D1").unwrap()), "");
        let at = |a1: &str| book.input(SheetId(0), CellPos::parse_a1(a1).unwrap());
        assert_eq!(
            (at("B2"), at("C1"), at("C2")),
            ("=A2*2".into(), "=B1*2".into(), "=B2*2".into())
        );
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
