// Test helpers outside #[test] functions; clippy only exempts the functions themselves.
#![allow(clippy::unwrap_used)]

use std::io::Write;

use zenkai_engine::{EngineError, ListedSheet, sheet_names_of};

fn workbook(sheets: &[(&str, &str)]) -> Vec<u8> {
    let mut entries = String::new();
    let mut rels = String::new();
    for (index, (name, state)) in sheets.iter().enumerate() {
        let id = index + 1;
        entries.push_str(&format!(
            r#"<sheet name="{name}" sheetId="{id}" {state} r:id="rId{id}"/>"#
        ));
        rels.push_str(&format!(
            r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{id}.xml"/>"#
        ));
    }
    let workbook = format!(
        r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{entries}</sheets></workbook>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for (name, body) in [
        ("xl/workbook.xml", workbook.as_str()),
        ("xl/_rels/workbook.xml.rels", rels.as_str()),
        ("xl/worksheets/sheet1.xml", "<<< corrupt"),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn lists_visible_and_hidden_sheets_without_reading_worksheets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsx");
    std::fs::write(
        &path,
        workbook(&[
            ("Resumen", ""),
            ("Año", ""),
            ("Oculta", r#"state="hidden""#),
        ]),
    )
    .unwrap();
    let expected = |name: &str, hidden| ListedSheet {
        name: name.to_string(),
        hidden,
    };
    assert_eq!(
        sheet_names_of(&path).unwrap(),
        [
            expected("Resumen", false),
            expected("Año", false),
            expected("Oculta", true)
        ]
    );
}

#[test]
fn a_file_that_is_not_xlsx_is_an_invalid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.xlsx");
    std::fs::write(&path, b"plain text").unwrap();
    assert!(matches!(
        sheet_names_of(&path),
        Err(EngineError::InvalidFile(_))
    ));
}

#[test]
fn a_missing_file_is_a_read_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        sheet_names_of(&dir.path().join("gone.xlsx")),
        Err(EngineError::Read { .. })
    ));
}
