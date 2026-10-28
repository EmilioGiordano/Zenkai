// Formula cells written without a cached value, as openpyxl writes them, compute on open.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read, Write};

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, SaveTarget, XlsxReader, open_xlsx_with, save_xlsx};
use zenkai_types::{CellPos, SheetId};

const SHEET: SheetId = SheetId(0);

fn at(text: &str) -> CellPos {
    CellPos::parse_a1(text).unwrap()
}

fn source_without_cached_values(path: &std::path::Path) {
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 2.0).unwrap();
    sheet.write_formula(0, 1, "=A1*3").unwrap();
    sheet.write_formula(0, 2, "=B1+1").unwrap();
    sheet.write_formula(1, 0, "=SUM(A1:C1)").unwrap();
    book.save(path).unwrap();

    let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            let xml = String::from_utf8(bytes).unwrap();
            let stripped = xml.replace("</f><v>0</v>", "</f>");
            assert_eq!(stripped.matches("</f></c>").count(), 3, "{xml}");
            bytes = stripped.into_bytes();
        }
        out.start_file(name, zip::write::FileOptions::default())
            .unwrap();
        out.write_all(&bytes).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

fn assert_computed(book: &impl Engine) {
    for (cell, expected) in [("B1", "6"), ("C1", "7"), ("A2", "15")] {
        assert_eq!(book.cell(SHEET, at(cell)).text, expected, "{cell}");
    }
}

fn formulas_compute_on_open(reader: XlsxReader) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated.xlsx");
    source_without_cached_values(&path);

    let opened = open_xlsx_with(&path, reader).unwrap();
    assert_eq!(opened.fallback, None);
    assert_computed(&opened.workbook);

    let saved_path = dir.path().join("saved.xlsx");
    save_xlsx(
        opened
            .workbook
            .save_snapshot(&saved_path, SaveTarget::Document)
            .unwrap(),
    )
    .unwrap();
    assert_computed(&open_xlsx_with(&saved_path, reader).unwrap().workbook);
}

#[test]
fn ironcalc_reader_computes_formulas_without_cached_values() {
    formulas_compute_on_open(XlsxReader::IronCalc);
}

#[test]
fn fast_reader_computes_formulas_without_cached_values() {
    formulas_compute_on_open(XlsxReader::Fast);
}
