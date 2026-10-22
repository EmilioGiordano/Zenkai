// A cell that carries only a style stays empty through open, edit and save.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read, Write};

use rust_xlsxwriter::{Format, FormatBorder, Workbook as Source};
use zenkai_engine::{Engine, SaveTarget, XlsxReader, open_xlsx_with, save_xlsx};
use zenkai_types::{CellPos, SheetId, ValueKind};

const SHEET: SheetId = SheetId(0);

fn at(text: &str) -> CellPos {
    CellPos::parse_a1(text).unwrap()
}

fn total_row_source(path: &std::path::Path) {
    let border = Format::new()
        .set_border_top(FormatBorder::Thin)
        .set_border_bottom(FormatBorder::Thin);
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    for col in 0..3u16 {
        sheet.write_blank(30, col, &border).unwrap();
    }
    for col in 3..7u16 {
        sheet
            .write_number_with_format(30, col, 5.0, &border)
            .unwrap();
    }
    book.save(path).unwrap();
}

// openpyxl marks a styled blank as a number with no value, as t="n" with or without <v/>.
fn mark_blanks_as_numbers(path: &std::path::Path, tail: &str) {
    let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            let mut xml = String::from_utf8(bytes).unwrap();
            for col in ["A", "B", "C"] {
                let blank = blank_element(&xml, &format!("{col}31"));
                xml = xml.replace(&blank, &format!("{}{tail}", &blank[..blank.len() - 2]));
            }
            bytes = xml.into_bytes();
        }
        out.start_file(name, zip::write::FileOptions::default())
            .unwrap();
        out.write_all(&bytes).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

fn blank_element(xml: &str, cell: &str) -> String {
    let start = xml.find(&format!("<c r=\"{cell}\"")).unwrap();
    let end = start + xml[start..].find("/>").unwrap() + 2;
    xml[start..end].to_string()
}

fn assert_styled_blanks(book: &impl Engine) {
    for name in ["A31", "B31", "C31"] {
        let view = book.cell(SHEET, at(name));
        assert_eq!(view.text, "", "{name}");
        assert_eq!(view.kind, ValueKind::Empty, "{name}");
        assert!(view.style.border_top, "{name} lost its border");
    }
    assert_eq!(book.cell(SHEET, at("D31")).text, "5");
}

fn blanks_stay_empty(reader: XlsxReader, tail: Option<&str>, edited: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("total.xlsx");
    total_row_source(&path);
    if let Some(tail) = tail {
        mark_blanks_as_numbers(&path, tail);
    }
    let opened = open_xlsx_with(&path, reader).unwrap();
    assert_eq!(opened.fallback, None);
    assert_styled_blanks(&opened.workbook);

    let mut book = opened.workbook;
    book.set_input(SHEET, at(edited), "5").unwrap();
    let saved_path = dir.path().join("saved.xlsx");
    save_xlsx(
        book.save_snapshot(&saved_path, SaveTarget::Document)
            .unwrap(),
    )
    .unwrap();
    assert_styled_blanks(&open_xlsx_with(&saved_path, reader).unwrap().workbook);
}

const NUMBER_TAIL: &str = r#" t="n"/>"#;
const NUMBER_WITH_EMPTY_VALUE_TAIL: &str = r#" t="n"><v></v></c>"#;

#[test]
fn ironcalc_reader_keeps_styled_blanks_empty() {
    for edited in ["A1", "D31"] {
        blanks_stay_empty(XlsxReader::IronCalc, None, edited);
    }
    for edited in ["A1", "D31"] {
        blanks_stay_empty(XlsxReader::IronCalc, Some(NUMBER_TAIL), edited);
    }
    for edited in ["A1", "D31"] {
        blanks_stay_empty(
            XlsxReader::IronCalc,
            Some(NUMBER_WITH_EMPTY_VALUE_TAIL),
            edited,
        );
    }
}

#[test]
fn fast_reader_keeps_styled_blanks_empty() {
    for edited in ["A1", "D31"] {
        blanks_stay_empty(XlsxReader::Fast, None, edited);
    }
    for edited in ["A1", "D31"] {
        blanks_stay_empty(XlsxReader::Fast, Some(NUMBER_TAIL), edited);
    }
    for edited in ["A1", "D31"] {
        blanks_stay_empty(XlsxReader::Fast, Some(NUMBER_WITH_EMPTY_VALUE_TAIL), edited);
    }
}
