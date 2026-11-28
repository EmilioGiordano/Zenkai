// Gridlines and Headings are per-sheet flags saved in the file: a toggle must survive a
// save and reopen, leave the other sheets and the print options alone, and undo.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read, Write};
use std::path::Path;

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, Workbook, XlsxReader, open_xlsx_with};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

const FIRST: SheetId = SheetId(0);
const SECOND: SheetId = SheetId(1);
const READERS: [XlsxReader; 2] = [XlsxReader::IronCalc, XlsxReader::Fast];

// The first sheet is what Excel writes for View, Gridlines off, with the print option on.
fn source(path: &Path) {
    let mut book = Source::new();
    let first = book.add_worksheet();
    first.write_string(0, 0, "first").unwrap();
    first.set_screen_gridlines(false);
    first.set_print_gridlines(true);
    book.add_worksheet().write_string(0, 0, "second").unwrap();
    book.save(path).unwrap();
}

// Excel writes showRowColHeaders="0" for View, Headings off; the schema's boolean also
// allows "false".
fn hide_headings_of_the_first_sheet(path: &Path, spelling: &str) {
    let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            let attribute = format!("<sheetView showRowColHeaders=\"{spelling}\" ");
            data = String::from_utf8(data)
                .unwrap()
                .replacen("<sheetView ", &attribute, 1)
                .into_bytes();
        }
        out.start_file(name, zip::write::FileOptions::default())
            .unwrap();
        out.write_all(&data).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

fn part(xlsx: &[u8], name: &str) -> String {
    let mut archive = zip::ZipArchive::new(Cursor::new(xlsx)).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn open(path: &Path, reader: XlsxReader) -> Workbook {
    let opened = open_xlsx_with(path, reader).unwrap();
    assert_eq!(opened.fallback, None, "the reader under test was not used");
    opened.workbook
}

#[test]
fn a_file_with_gridlines_off_opens_without_gridlines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.xlsx");
    source(&path);
    for reader in READERS {
        let book = open(&path, reader);
        assert!(!book.gridlines_shown(FIRST));
        assert!(book.gridlines_shown(SECOND));
    }
}

#[test]
fn a_file_that_says_nothing_shows_headings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.xlsx");
    source(&path);
    for reader in READERS {
        let book = open(&path, reader);
        assert!(book.headings_shown(FIRST));
        assert!(book.headings_shown(SECOND));
    }
}

#[test]
fn a_file_with_headings_off_opens_without_headings() {
    for spelling in ["0", "false"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.xlsx");
        source(&path);
        hide_headings_of_the_first_sheet(&path, spelling);
        for reader in READERS {
            let book = open(&path, reader);
            assert!(!book.headings_shown(FIRST), "{spelling}");
            assert!(book.headings_shown(SECOND), "{spelling}");
            assert!(!book.gridlines_shown(FIRST), "{spelling}");
        }
    }
}

#[test]
fn toggles_round_trip_through_save_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.xlsx");
    source(&path);
    for reader in READERS {
        let mut book = open(&path, reader);
        book.set_gridlines_shown(FIRST, true).unwrap();
        book.set_gridlines_shown(SECOND, false).unwrap();
        book.set_headings_shown(SECOND, false).unwrap();
        let saved = book.to_xlsx().unwrap();

        let saved_path = dir.path().join("saved.xlsx");
        std::fs::write(&saved_path, &saved).unwrap();
        for reader in READERS {
            let reopened = open(&saved_path, reader);
            assert!(reopened.gridlines_shown(FIRST));
            assert!(reopened.headings_shown(FIRST));
            assert!(!reopened.gridlines_shown(SECOND));
            assert!(!reopened.headings_shown(SECOND));
        }

        let first = part(&saved, "xl/worksheets/sheet1.xml");
        assert!(!first.contains("showGridLines"), "{first}");
        assert!(!first.contains("showRowColHeaders"), "{first}");
        let second = part(&saved, "xl/worksheets/sheet2.xml");
        assert!(second.contains(r#"showGridLines="0""#), "{second}");
        assert!(second.contains(r#"showRowColHeaders="0""#), "{second}");
    }
}

#[test]
fn a_toggle_leaves_the_print_options_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.xlsx");
    source(&path);
    for reader in READERS {
        let mut book = open(&path, reader);
        book.set_gridlines_shown(FIRST, true).unwrap();
        book.set_headings_shown(FIRST, false).unwrap();
        let first = part(&book.to_xlsx().unwrap(), "xl/worksheets/sheet1.xml");
        assert!(first.contains("<printOptions"), "{first}");
        assert!(first.contains(r#"gridLines="1""#), "{first}");
        assert!(!first.contains("headings="), "{first}");
    }
}

#[test]
fn a_heading_flag_the_file_already_had_is_written_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.xlsx");
    source(&path);
    hide_headings_of_the_first_sheet(&path, "0");
    for reader in READERS {
        let mut book = open(&path, reader);
        book.set_gridlines_shown(FIRST, true).unwrap();
        let first = part(&book.to_xlsx().unwrap(), "xl/worksheets/sheet1.xml");
        assert_eq!(first.matches("showRowColHeaders").count(), 1, "{first}");
        roxmltree::Document::parse(&first).unwrap();
    }
}

#[test]
fn a_new_sheet_shows_both_and_a_duplicate_copies_them() {
    let mut book = Workbook::new_empty().unwrap();
    book.set_gridlines_shown(FIRST, false).unwrap();
    book.set_headings_shown(FIRST, false).unwrap();

    let added = book.add_sheet().unwrap();
    assert!(book.gridlines_shown(added));
    assert!(book.headings_shown(added));

    book.duplicate_sheet(FIRST).unwrap();
    let copy = book
        .sheets()
        .iter()
        .find(|sheet| sheet.name.contains('('))
        .unwrap()
        .id;
    assert!(!book.gridlines_shown(copy));
    assert!(!book.headings_shown(copy));
}

#[test]
fn each_toggle_is_its_own_undo_step_on_its_own_sheet() {
    let mut book = Workbook::new_empty().unwrap();
    let second = book.add_sheet().unwrap();
    book.set_gridlines_shown(FIRST, false).unwrap();
    book.set_headings_shown(FIRST, false).unwrap();
    assert!(book.gridlines_shown(second) && book.headings_shown(second));

    book.undo().unwrap();
    assert!(book.headings_shown(FIRST));
    assert!(!book.gridlines_shown(FIRST));
    book.undo().unwrap();
    assert!(book.gridlines_shown(FIRST));

    book.redo().unwrap();
    assert!(!book.gridlines_shown(FIRST));
    assert!(book.headings_shown(FIRST));
    book.redo().unwrap();
    assert!(!book.headings_shown(FIRST));
    assert!(book.gridlines_shown(second) && book.headings_shown(second));
}

#[test]
fn a_toggle_does_not_recalculate_volatile_formulas() {
    let mut book = Workbook::new_empty().unwrap();
    let at = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(0));
    book.set_input(FIRST, at, "=RAND()").unwrap();
    let before = book.cell(FIRST, at).text;
    book.set_gridlines_shown(FIRST, false).unwrap();
    book.set_headings_shown(FIRST, false).unwrap();
    assert_eq!(book.cell(FIRST, at).text, before);
}

#[test]
fn a_workbook_that_was_never_toggled_reads_as_shown() {
    let book = Workbook::new_empty().unwrap();
    assert!(book.gridlines_shown(FIRST));
    assert!(book.headings_shown(FIRST));
    assert!(book.gridlines_shown(SheetId(9)));
    assert!(book.headings_shown(SheetId(9)));
}
