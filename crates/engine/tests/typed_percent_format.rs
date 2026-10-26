// A typed percentage gets Excel's built-in format 0% or 0.00%, and the saved file names it
// by its built-in id, as Excel writes it (GitHub #112).
#![allow(clippy::unwrap_used)]

use std::io::Read;
use std::path::Path;

use rust_xlsxwriter::{Format, Workbook as Source};
use zenkai_engine::{
    Engine, SaveTarget, Workbook, XlsxReader, open_xlsx, open_xlsx_with, save_xlsx,
};
use zenkai_types::{CellPos, Range, SheetId};

const SHEET: SheetId = SheetId(0);
const SHEET_PART: &str = "xl/worksheets/sheet1.xml";
const STYLES_PART: &str = "xl/styles.xml";

fn at(text: &str) -> CellPos {
    CellPos::parse_a1(text).unwrap()
}

fn typed(book: &mut Workbook, cell: &str, text: &str) -> (String, String) {
    book.set_input(SHEET, at(cell), text).unwrap();
    let view = book.cell(SHEET, at(cell));
    (view.text, view.style.num_fmt)
}

fn part(file: &Path, name: &str) -> String {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

// The numFmtId of the cellXfs entry the saved cell points at.
fn saved_num_fmt_id(file: &Path, cell: &str) -> String {
    let sheet = part(file, SHEET_PART);
    let sheet = roxmltree::Document::parse(&sheet).unwrap();
    let style: usize = sheet
        .descendants()
        .find(|node| node.has_tag_name("c") && node.attribute("r") == Some(cell))
        .unwrap()
        .attribute("s")
        .unwrap()
        .parse()
        .unwrap();
    let styles = part(file, STYLES_PART);
    let styles = roxmltree::Document::parse(&styles).unwrap();
    let xfs = styles
        .descendants()
        .find(|node| node.has_tag_name("cellXfs"))
        .unwrap();
    xfs.children()
        .filter(roxmltree::Node::is_element)
        .nth(style)
        .unwrap()
        .attribute("numFmtId")
        .unwrap()
        .to_string()
}

fn custom_percent_formats(file: &Path) -> Vec<String> {
    let styles = part(file, STYLES_PART);
    let styles = roxmltree::Document::parse(&styles).unwrap();
    styles
        .descendants()
        .filter(|node| node.has_tag_name("numFmt"))
        .filter_map(|node| node.attribute("formatCode"))
        .filter(|code| code.contains('%'))
        .map(str::to_string)
        .collect()
}

fn assert_saved_percentages(file: &Path) {
    assert_eq!(saved_num_fmt_id(file, "B2"), "9");
    assert_eq!(saved_num_fmt_id(file, "B3"), "10");
    assert_eq!(custom_percent_formats(file), Vec::<String>::new());
    for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
        let reopened = open_xlsx_with(file, reader).unwrap().workbook;
        let num_fmt = |cell: &str| reopened.cell(SHEET, at(cell)).style.num_fmt;
        assert_eq!(num_fmt("B2"), "0%", "{reader:?}");
        assert_eq!(num_fmt("B3"), "0.00%", "{reader:?}");
        assert_eq!(reopened.cell(SHEET, at("B3")).text, "5.50%", "{reader:?}");
    }
}

#[test]
fn a_typed_percentage_gets_excels_format() {
    let mut book = Workbook::new_empty().unwrap();
    assert_eq!(typed(&mut book, "A1", "5%"), ("5%".into(), "0%".into()));
    assert_eq!(
        typed(&mut book, "A2", "5.5%"),
        ("5.50%".into(), "0.00%".into())
    );
    assert_eq!(typed(&mut book, "A3", "-5%"), ("-5%".into(), "0%".into()));
    assert_eq!(
        typed(&mut book, "A4", "-12.5%"),
        ("-12.50%".into(), "0.00%".into())
    );
    assert_eq!(typed(&mut book, "A5", "5.125%").1, "0.00%");
    assert_eq!(book.cell(SHEET, at("A5")).number, Some(0.05125));
}

#[test]
fn a_typed_percentage_keeps_the_percentage_format_a_cell_has() {
    let mut book = Workbook::new_empty().unwrap();
    let a1 = Range::new(at("A1"), at("A1"));
    book.set_number_format(SHEET, a1, "0.000%").unwrap();
    assert_eq!(
        typed(&mut book, "A1", "5%"),
        ("5.000%".into(), "0.000%".into())
    );

    assert_eq!(
        typed(&mut book, "A2", "5.5%"),
        ("5.50%".into(), "0.00%".into())
    );
    assert_eq!(
        typed(&mut book, "A2", "8%"),
        ("8.00%".into(), "0.00%".into())
    );

    let a3 = Range::new(at("A3"), at("A3"));
    book.set_number_format(SHEET, a3, "#,##0.00").unwrap();
    assert_eq!(typed(&mut book, "A3", "5%"), ("5%".into(), "0%".into()));
}

#[test]
fn a_new_workbook_saves_typed_percentages_with_the_built_in_ids() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("new.xlsx");
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SHEET, at("B2"), "5%").unwrap();
    book.set_input(SHEET, at("B3"), "5.5%").unwrap();
    save_xlsx(book.save_snapshot(&file, SaveTarget::Document).unwrap()).unwrap();
    assert_saved_percentages(&file);
}

#[test]
fn an_opened_file_saves_typed_percentages_with_the_built_in_ids() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("opened.xlsx");
    let mut source = Source::new();
    let kilos = Format::new().set_num_format("0.000\" kg\"");
    let sheet = source.add_worksheet();
    sheet.write_number_with_format(0, 0, 1.5, &kilos).unwrap();
    sheet.write_number(1, 0, 2.0).unwrap();
    source.save(&file).unwrap();

    let mut book = open_xlsx(&file).unwrap().workbook;
    book.set_input(SHEET, at("B2"), "5%").unwrap();
    book.set_input(SHEET, at("B3"), "5.5%").unwrap();
    let snapshot = book.save_snapshot(&file, SaveTarget::Document).unwrap();
    assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
    save_xlsx(snapshot).unwrap();
    assert_saved_percentages(&file);
}
