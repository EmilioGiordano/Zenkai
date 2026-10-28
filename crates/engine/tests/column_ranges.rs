// Column ranges from a file or from an insert must save as ranges Excel accepts: each
// within 1..=16384 with min <= max, and the valid widths around them kept.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read, Write};
use std::path::Path;

use zenkai_engine::{Engine, SaveTarget, XlsxReader, open_xlsx_with, save_xlsx};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

// Zenkai rewrites the saved <cols> only when a column is outlined.
const OUTLINED_G: &str = r#"<col min="7" max="7" width="20" customWidth="1" outlineLevel="1"/>"#;

fn with_columns(path: &Path, cols: &str) {
    let mut source = rust_xlsxwriter::Workbook::new();
    source.add_worksheet().write_number(0, 0, 1.0).unwrap();
    let bytes = source.save_to_buffer().unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut body = String::new();
        entry.read_to_string(&mut body).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            body = body.replacen("<sheetData", &format!("<cols>{cols}</cols><sheetData"), 1);
        }
        out.start_file(name, options).unwrap();
        out.write_all(body.as_bytes()).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

fn saved_columns(xlsx: &[u8]) -> Vec<(u32, u32, String)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(xlsx)).unwrap();
    let mut sheet = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet)
        .unwrap();
    let document = roxmltree::Document::parse(&sheet).unwrap();
    document
        .descendants()
        .filter(|n| n.has_tag_name("col"))
        .map(|col| {
            let bound = |name| col.attribute(name).unwrap().parse::<u32>().unwrap();
            let width = col.attribute("width").unwrap().to_string();
            (bound("min"), bound("max"), width)
        })
        .collect()
}

fn assert_valid(columns: &[(u32, u32, String)]) {
    for (min, max, _) in columns {
        assert!(1 <= *min && min <= max && *max <= 16_384, "{columns:?}");
    }
}

#[test]
fn invalid_column_ranges_from_a_file_save_without_them() {
    let hostile = [
        r#"<col min="5" max="3" width="9" customWidth="1"/>"#,
        r#"<col min="16385" max="16390" width="9" customWidth="1"/>"#,
        r#"<col min="-7" max="0" width="9" customWidth="1"/>"#,
    ];
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        for col in hostile {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cols.xlsx");
            with_columns(&path, &format!("{OUTLINED_G}{col}"));
            let book = open_xlsx_with(&path, reader).unwrap().workbook;
            let columns = saved_columns(&book.to_xlsx().unwrap());
            assert_eq!(columns, [(7, 7, "20".to_string())], "{reader:?} {col}");
        }
    }
}

#[test]
fn column_ranges_reaching_past_the_sheet_keep_their_part_inside_it() {
    let partial = [
        (
            r#"<col min="10" max="20000" width="30" customWidth="1"/>"#,
            (10, 16_384),
        ),
        (
            r#"<col min="0" max="4" width="30" customWidth="1"/>"#,
            (1, 4),
        ),
    ];
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        for (col, (min, max)) in partial {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cols.xlsx");
            with_columns(&path, &format!("{OUTLINED_G}{col}"));
            let book = open_xlsx_with(&path, reader).unwrap().workbook;
            let columns = saved_columns(&book.to_xlsx().unwrap());
            assert_valid(&columns);
            assert!(
                columns.contains(&(min, max, "30".to_string())),
                "{reader:?} {col} {columns:?}"
            );
            assert_eq!(columns.len(), 2, "{reader:?} {col} {columns:?}");
        }
    }
}

#[test]
fn inserting_columns_saves_ranges_inside_the_sheet_and_undo_restores_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cols.xlsx");
    with_columns(
        &path,
        r#"<col min="2" max="16383" width="20" customWidth="1"/><col min="16384" max="16384" width="30" customWidth="1"/>"#,
    );
    let mut book = open_xlsx_with(&path, XlsxReader::Fast).unwrap().workbook;
    let before = book.sizes(SheetId(0));
    book.insert_columns(SheetId(0), ColIdx::new(0).unwrap(), 2)
        .unwrap();
    let columns = saved_columns(&book.to_xlsx().unwrap());
    assert_eq!(columns, [(4, 16_384, "20".to_string())]);
    book.undo().unwrap();
    assert_eq!(book.sizes(SheetId(0)), before);
    book.redo().unwrap();
    assert_eq!(saved_columns(&book.to_xlsx().unwrap()), columns);
}

#[test]
fn an_outlined_range_reaching_past_the_sheet_keeps_its_outline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cols.xlsx");
    with_columns(
        &path,
        r#"<col min="10" max="20000" width="30" customWidth="1" outlineLevel="2"/>"#,
    );
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let book = open_xlsx_with(&path, reader).unwrap().workbook;
        let saved = book.to_xlsx().unwrap();
        let mut sheet = String::new();
        zip::ZipArchive::new(Cursor::new(saved))
            .unwrap()
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        assert!(
            sheet.contains(
                r#"<col min="10" max="16384" width="30" customWidth="1" outlineLevel="2"/>"#
            ),
            "{reader:?} {sheet}"
        );
    }
}

#[test]
fn invalid_column_ranges_from_a_file_save_without_them_after_an_edit() {
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cols.xlsx");
        with_columns(
            &path,
            &format!(r#"{OUTLINED_G}<col min="5" max="3" width="9" customWidth="1"/>"#),
        );
        let mut book = open_xlsx_with(&path, reader).unwrap().workbook;
        book.set_input(SheetId(0), CellPos::parse_a1("B2").unwrap(), "new text")
            .unwrap();
        save_xlsx(book.save_snapshot(&path, SaveTarget::Document).unwrap()).unwrap();
        let columns = saved_columns(&std::fs::read(&path).unwrap());
        assert_eq!(columns, [(7, 7, "20".to_string())], "{reader:?}");
    }
}

#[test]
fn columns_pushed_past_the_sheet_by_an_insert_are_not_shown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cols.xlsx");
    with_columns(
        &path,
        r#"<col min="2" max="16383" width="20" customWidth="1"/><col min="16384" max="16384" width="30" customWidth="1"/>"#,
    );
    let mut book = open_xlsx_with(&path, XlsxReader::Fast).unwrap().workbook;
    book.insert_columns(SheetId(0), ColIdx::new(0).unwrap(), 2)
        .unwrap();
    let saved = dir.path().join("saved.xlsx");
    std::fs::write(&saved, book.to_xlsx().unwrap()).unwrap();
    let reopened = open_xlsx_with(&saved, XlsxReader::Fast).unwrap().workbook;
    assert_eq!(book.sizes(SheetId(0)), reopened.sizes(SheetId(0)));
}

#[test]
fn a_file_repeating_full_width_columns_is_refused_not_expanded() {
    let cols =
        r#"<col min="1" max="16384" width="9" customWidth="1" outlineLevel="1"/>"#.repeat(20_000);
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cols.xlsx");
        with_columns(&path, &cols);
        assert!(open_xlsx_with(&path, reader).is_err(), "{reader:?}");
    }
}

fn sheet_xml(xlsx: Vec<u8>) -> String {
    let mut sheet = String::new();
    zip::ZipArchive::new(Cursor::new(xlsx))
        .unwrap()
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet)
        .unwrap();
    sheet
}

// Hiding and showing a default-width column drops the model's entry for it; the outline
// is Zenkai's and must still be written, and survive a reopen.
#[test]
fn hiding_and_showing_an_outlined_default_width_column_keeps_its_outline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cols.xlsx");
    with_columns(
        &path,
        r#"<col min="3" max="3" width="10" outlineLevel="1" collapsed="1"/>"#,
    );
    let outlined = r#"outlineLevel="1" collapsed="1"/>"#;
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let mut book = open_xlsx_with(&path, reader).unwrap().workbook;
        let column = Range::new(
            CellPos::parse_a1("C1").unwrap(),
            CellPos::new(RowIdx::clamped(i64::MAX), ColIdx::new(2).unwrap()),
        );
        book.set_columns_hidden(SheetId(0), column, true).unwrap();
        book.set_columns_hidden(SheetId(0), column, false).unwrap();
        let saved = book.to_xlsx().unwrap();
        let sheet = sheet_xml(saved.clone());
        assert!(
            sheet.contains(r#"<col min="3" max="3""#),
            "{reader:?} {sheet}"
        );
        assert!(sheet.contains(outlined), "{reader:?} {sheet}");
        let again = dir.path().join("saved.xlsx");
        std::fs::write(&again, saved).unwrap();
        let reopened = open_xlsx_with(&again, reader).unwrap().workbook;
        let sheet = sheet_xml(reopened.to_xlsx().unwrap());
        assert!(sheet.contains(outlined), "{reader:?} {sheet}");
    }
}
