// A filter in a file: what Excel writes opens filtered with either reader, and every save
// writes the filter and the rows it hides back.

use std::io::{Read, Write};
use std::path::Path;

use zenkai_engine::{
    Engine, ReaderComparison, SaveTarget, Unsupported, Workbook, XlsxReader, compare_readers,
    open_xlsx_with, save_xlsx,
};
use zenkai_types::{CellPos, ColumnCriteria};

use super::{SHEET, col, hidden_rows, pos, range, row, shown};

const SHEET_PART: &str = "xl/worksheets/sheet1.xml";

const STRINGS: &str = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="9" uniqueCount="6"><si><t>Name</t></si><si><t>Qty</t></si><si><t>Total</t></si><si><t>x</t></si><si><t>y</t></si><si><t>z</t></si></sst>"#;

// As Excel writes a sheet filtered to Name = x: the rows it hides marked hidden, and row 9,
// outside the filter, hidden by hand.
fn excel_sheet(filter: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetPr filterMode="1"/><dimension ref="A1:E9"/><sheetViews><sheetView tabSelected="1" workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData><row r="1" spans="1:5"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c><c r="E1"><f>SUBTOTAL(9,C2:C6)</f><v>90</v></c></row><row r="2" spans="1:5"><c r="A2" t="s"><v>3</v></c><c r="B2"><v>1</v></c><c r="C2"><v>10</v></c></row><row r="3" spans="1:5" hidden="1"><c r="A3" t="s"><v>4</v></c><c r="B3"><v>2</v></c><c r="C3"><v>20</v></c></row><row r="4" spans="1:5"><c r="A4" t="s"><v>3</v></c><c r="B4"><v>3</v></c><c r="C4"><v>30</v></c></row><row r="5" spans="1:5" hidden="1"><c r="A5" t="s"><v>5</v></c><c r="B5"><v>4</v></c><c r="C5"><v>40</v></c></row><row r="6" spans="1:5"><c r="A6" t="s"><v>3</v></c><c r="B6"><v>5</v></c><c r="C6"><v>50</v></c></row><row r="9" spans="1:5" hidden="1"><c r="A9"><v>7</v></c></row></sheetData>{filter}<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#
    )
}

const BY_NAME: &str = r#"<autoFilter ref="A1:C6"><filterColumn colId="0"><filters><filter val="x"/></filters></filterColumn></autoFilter>"#;

// Hides rows 3 and 5 too, by a rule only Excel can edit.
const BY_RULE: &str = r#"<autoFilter ref="A1:C6"><filterColumn colId="2"><customFilters and="1"><customFilter operator="notEqual" val="20"/><customFilter operator="notEqual" val="40"/></customFilters></filterColumn></autoFilter>"#;

fn xlsx(sheet: &str) -> Vec<u8> {
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/styles.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#.to_string(),
        ),
        ("xl/sharedStrings.xml", STRINGS.to_string()),
        (SHEET_PART, sheet.to_string()),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::FileOptions::default();
    for (name, body) in parts {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

pub fn sheet_xml(xlsx: &[u8]) -> String {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(xlsx)).unwrap();
    let mut xml = String::new();
    archive
        .by_name(SHEET_PART)
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

fn row_tag(xml: &str, number: u32) -> &str {
    let start = xml.find(&format!(r#"<row r="{number}""#)).unwrap();
    let end = start + xml[start..].find('>').unwrap();
    &xml[start..=end]
}

fn open(dir: &Path, bytes: &[u8], reader: XlsxReader) -> Workbook {
    let path = dir.join("filtered.xlsx");
    std::fs::write(&path, bytes).unwrap();
    let opened = open_xlsx_with(&path, reader).unwrap();
    assert!(opened.fallback.is_none(), "{:?}", opened.fallback);
    assert!(
        !opened.unsupported.contains(&Unsupported::AutoFilter),
        "{:?}",
        opened.unsupported
    );
    opened.workbook
}

fn saved(book: &Workbook, dir: &Path) -> Vec<u8> {
    let path = dir.join("saved.xlsx");
    let snapshot = book.save_snapshot(&path, SaveTarget::Document).unwrap();
    assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
    save_xlsx(snapshot).unwrap();
    std::fs::read(&path).unwrap()
}

const READERS: [XlsxReader; 2] = [XlsxReader::Fast, XlsxReader::IronCalc];

#[test]
fn both_readers_read_a_filtered_sheet_alike() {
    for sheet in [excel_sheet(BY_NAME), excel_sheet(BY_RULE)] {
        let comparison = compare_readers(&xlsx(&sheet));
        assert!(
            matches!(comparison, ReaderComparison::Same),
            "{comparison:?}"
        );
    }
}

#[test]
fn a_sheet_filtered_in_excel_opens_filtered() {
    let dir = tempfile::tempdir().unwrap();
    for reader in READERS {
        let mut book = open(dir.path(), &xlsx(&excel_sheet(BY_NAME)), reader);
        let view = book.auto_filter(SHEET).unwrap();
        assert_eq!(view.range, range("A1:C6"));
        assert_eq!(view.filtered_out, [(row(3), row(3)), (row(5), row(5))]);
        assert_eq!((view.found, view.total), (3, 5));
        assert_eq!(hidden_rows(&book), [3, 5, 9]);
        assert_eq!(book.cell(SHEET, pos("E1")).text, "90");
        let names = book.filter_values(SHEET, col('A'), "").unwrap();
        let checks: Vec<(&str, bool)> = names
            .items
            .iter()
            .map(|item| (item.text.as_str(), item.shown))
            .collect();
        assert_eq!(checks, [("x", true), ("y", false), ("z", false)]);

        // Row 9 is outside the filter, so it stays hidden by hand.
        book.clear_filters(SHEET).unwrap();
        assert_eq!(hidden_rows(&book), [9]);
        assert_eq!(book.cell(SHEET, pos("E1")).text, "150");
    }
}

#[test]
fn open_and_save_keep_the_filter_and_the_rows_it_hides() {
    let dir = tempfile::tempdir().unwrap();
    let book = open(dir.path(), &xlsx(&excel_sheet(BY_NAME)), XlsxReader::Fast);
    let copied = sheet_xml(&saved(&book, dir.path()));
    let written = sheet_xml(&book.to_xlsx().unwrap());
    for xml in [copied, written] {
        assert!(xml.contains(BY_NAME), "{xml}");
        assert!(xml.contains(r#"<sheetPr filterMode="1"/>"#), "{xml}");
        for number in [3, 5, 9] {
            assert!(row_tag(&xml, number).contains(r#"hidden="1""#), "{xml}");
        }
        assert!(!row_tag(&xml, 4).contains("hidden"), "{xml}");
    }
}

#[test]
fn every_save_after_an_edit_writes_the_filter_as_it_is_now() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = open(dir.path(), &xlsx(&excel_sheet(BY_NAME)), XlsxReader::Fast);
    book.set_input(SHEET, pos("D2"), "note").unwrap();
    let xml = sheet_xml(&saved(&book, dir.path()));
    assert!(xml.contains(BY_NAME), "{xml}");
    assert!(row_tag(&xml, 3).contains(r#"hidden="1""#), "{xml}");

    book.set_column_filter(SHEET, col('A'), shown(&["y"], false))
        .unwrap();
    let saved_bytes = saved(&book, dir.path());
    let xml = sheet_xml(&saved_bytes);
    assert!(xml.contains(r#"<filter val="y"/>"#), "{xml}");
    let elsewhere = tempfile::tempdir().unwrap();
    let reopened = open(elsewhere.path(), &saved_bytes, XlsxReader::Fast);
    assert_eq!(hidden_rows(&reopened), [2, 4, 5, 6, 9]);
    assert_eq!(reopened.auto_filter(SHEET), book.auto_filter(SHEET));

    book.clear_filters(SHEET).unwrap();
    let xml = sheet_xml(&saved(&book, dir.path()));
    assert!(xml.contains(r#"<autoFilter ref="A1:C6"/>"#), "{xml}");
    assert!(!xml.contains("filterMode"), "{xml}");

    book.toggle_auto_filter(SHEET, range("A1:C6")).unwrap();
    let xml = sheet_xml(&saved(&book, dir.path()));
    assert!(!xml.contains("<autoFilter"), "{xml}");
    assert!(!xml.contains("filterMode"), "{xml}");
    assert!(row_tag(&xml, 9).contains(r#"hidden="1""#), "{xml}");
}

#[test]
fn a_rule_only_excel_edits_is_kept_shown_as_custom_and_can_be_cleared() {
    for reader in READERS {
        let dir = tempfile::tempdir().unwrap();
        let mut book = open(dir.path(), &xlsx(&excel_sheet(BY_RULE)), reader);
        let view = book.auto_filter(SHEET).unwrap();
        assert_eq!(view.criteria, [(col('C'), ColumnCriteria::Custom)]);
        assert_eq!(hidden_rows(&book), [3, 5, 9]);
        assert!(book.filter_values(SHEET, col('C'), "").unwrap().custom);
        let copied = sheet_xml(&saved(&book, dir.path()));
        let written = sheet_xml(&book.to_xlsx().unwrap());
        for xml in [copied, written] {
            assert!(xml.contains(BY_RULE), "{xml}");
        }

        // The rule cannot be read again, so the rows it hid stay hidden when another
        // column is filtered.
        book.set_column_filter(SHEET, col('A'), shown(&["x", "y", "z"], false))
            .unwrap();
        assert_eq!(hidden_rows(&book), [3, 5, 9]);
        book.sort_filter(SHEET, col('B'), true).unwrap();
        let qty: Vec<String> = [2, 4, 6]
            .into_iter()
            .map(|number| book.input(SHEET, CellPos::new(row(number), col('B'))))
            .collect();
        assert_eq!(qty, ["5", "3", "1"]);
        assert_eq!(book.input(SHEET, pos("B3")), "2");

        book.clear_filters(SHEET).unwrap();
        assert_eq!(hidden_rows(&book), [9]);
        assert!(book.auto_filter(SHEET).unwrap().criteria.is_empty());
    }
}

#[test]
fn a_filter_written_by_another_program_reads_and_saves_the_same() {
    use rust_xlsxwriter::{FilterCondition, FilterCriteria};
    let dir = tempfile::tempdir().unwrap();
    let mut source = rust_xlsxwriter::Workbook::new();
    let sheet = source.add_worksheet();
    sheet.write_string(0, 0, "Name").unwrap();
    sheet.write_string(0, 1, "Qty").unwrap();
    for (number, (name, qty)) in (1u32..).zip([("x", 1.0), ("y", 2.0), ("x", 3.0), ("z", 4.0)]) {
        sheet.write_string(number, 0, name).unwrap();
        sheet.write_number(number, 1, qty).unwrap();
    }
    sheet.autofilter(0, 0, 4, 1).unwrap();
    sheet
        .filter_column(0, &FilterCondition::new().add_list_filter("x"))
        .unwrap();
    sheet
        .filter_column(
            1,
            &FilterCondition::new().add_custom_filter(FilterCriteria::GreaterThan, 0.5),
        )
        .unwrap();
    let path = dir.path().join("written.xlsx");
    source.save(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(matches!(compare_readers(&bytes), ReaderComparison::Same));

    let views: Vec<_> = READERS
        .into_iter()
        .map(|reader| {
            let book = open(dir.path(), &bytes, reader);
            assert_eq!(hidden_rows(&book), [3, 5]);
            book.auto_filter(SHEET).unwrap()
        })
        .collect();
    assert_eq!(views[0], views[1]);
    assert_eq!(views[0].range, range("A1:B5"));
    assert!(matches!(
        views[0].criteria.as_slice(),
        [(_, ColumnCriteria::Values(_)), (_, ColumnCriteria::Custom)]
    ));

    let book = open(dir.path(), &bytes, XlsxReader::Fast);
    let elsewhere = tempfile::tempdir().unwrap();
    let reopened = open(elsewhere.path(), &book.to_xlsx().unwrap(), XlsxReader::Fast);
    assert_eq!(reopened.auto_filter(SHEET).as_ref(), Some(&views[0]));
    assert_eq!(hidden_rows(&reopened), [3, 5]);
}
