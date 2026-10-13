// rust_xlsxwriter writes each worksheet on one line, so every chunk the readers stream
// ends inside a row. Valid files of that shape must open through every reader path.
// Thirty thousand rows give dozens of chunk ends, so some end cuts a tag.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use rust_xlsxwriter::{Formula, Workbook as Source, Worksheet};
use zenkai_engine::{Engine, Workbook, XlsxReader, open_xlsx_with};
use zenkai_types::{CellPos, SheetId};

const DISTINCT_TEXTS: u32 = 200;

fn fill(sheet: &mut Worksheet, rows: u32) {
    for row in 1..=rows {
        let line = row + 1;
        sheet.write_number(row, 0, f64::from(row)).unwrap();
        sheet
            .write_string(row, 1, format!("item {}", row % DISTINCT_TEXTS))
            .unwrap();
        sheet
            .write_number(row, 2, f64::from(row % 977) / 4.0)
            .unwrap();
        sheet
            .write_formula(row, 3, Formula::new(format!("=A{line}*C{line}")))
            .unwrap();
    }
}

fn write(path: &Path, sheet_rows: &[u32]) {
    let mut book = Source::new();
    for (index, rows) in sheet_rows.iter().enumerate() {
        let sheet = book.add_worksheet();
        fill(sheet, *rows);
        if index == 0 {
            sheet.group_rows(2, 20).unwrap();
        }
    }
    book.save(path).unwrap();
}

fn texts(book: &Workbook, sheets: usize, rows: u32) -> Vec<String> {
    (0..sheets)
        .flat_map(|sheet| {
            [1, rows / 2, rows].into_iter().flat_map(move |row| {
                ["A", "B", "C", "D"].map(move |col| (sheet, format!("{col}{}", row + 1)))
            })
        })
        .map(|(sheet, a1)| {
            let sheet = SheetId(u32::try_from(sheet).unwrap());
            book.cell(sheet, CellPos::parse_a1(&a1).unwrap()).text
        })
        .collect()
}

#[test]
fn one_line_sheets_open_with_every_reader() {
    const ROWS: u32 = 30_000;
    let shapes: [(&str, Vec<u32>); 3] = [
        ("one sheet", vec![ROWS]),
        ("big plus small", vec![ROWS, 100, 100, 100]),
        ("even", vec![ROWS / 4; 4]),
    ];
    for (shape, sheet_rows) in shapes {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        write(&path, &sheet_rows);
        let rows = *sheet_rows.iter().min().unwrap();
        let opened = [XlsxReader::Fast, XlsxReader::IronCalc, XlsxReader::Shadow].map(|reader| {
            let opened = open_xlsx_with(&path, reader)
                .unwrap_or_else(|error| panic!("{shape}, {reader:?}: {error}"));
            assert_eq!(opened.fallback, None, "{shape}, {reader:?}");
            texts(&opened.workbook, sheet_rows.len(), rows)
        });
        assert_eq!(opened[0], opened[1], "{shape}");
        assert_eq!(opened[0], opened[2], "{shape}");
        assert!(
            opened[0].iter().any(|text| text.starts_with("item")),
            "{shape}"
        );
    }
}
