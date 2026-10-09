// Open and save without edits must keep the page layout, view and outline settings that
// IronCalc alone drops; an edit that would make them wrong must be reported instead.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, Unsupported, open_xlsx, save_xlsx_atomic};
use zenkai_types::{ColIdx, RowIdx, SheetId};

// CT_Worksheet's child order, as far as Zenkai writes it.
const ORDER: [&str; 17] = [
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "mergeCells",
    "conditionalFormatting",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "drawing",
    "legacyDrawing",
    "extLst",
];

fn layout_source(path: &Path) {
    let mut book = Source::new();
    book.add_worksheet().write_string(0, 0, "first").unwrap();
    let sheet = book.add_worksheet().set_name("Report").unwrap();
    for row in 0..12u32 {
        sheet.write_number(row, 0, f64::from(row)).unwrap();
    }
    sheet.group_rows(1, 4).unwrap();
    sheet.group_rows_collapsed(6, 9).unwrap();
    sheet.group_columns(1, 2).unwrap();
    sheet.group_symbols_above(true);
    sheet.set_tab_color("#00B050");
    sheet.set_zoom(80);
    sheet.set_view_page_layout();
    sheet.set_default_row_height(18);
    sheet.set_paper_size(9);
    sheet.set_landscape();
    sheet.set_margins(0.5, 0.5, 0.8, 0.8, 0.3, 0.3);
    sheet.set_print_fit_to_pages(1, 0);
    sheet.set_print_center_horizontally(true);
    sheet.set_print_gridlines(true);
    sheet.set_header("&L&\"Arial,Bold\"Q3 && Q4 <draft>&RPage &P of &N");
    sheet.set_footer("&C&F");
    sheet.set_page_breaks(&[6]).unwrap();
    sheet.set_vertical_page_breaks(&[3]).unwrap();
    book.save(path).unwrap();
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

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name(name))
}

fn attributes(node: roxmltree::Node, prefix: &str, out: &mut BTreeMap<String, String>) {
    for attribute in node.attributes() {
        if attribute.namespace().is_none() {
            out.insert(
                format!("{prefix}@{}", attribute.name()),
                attribute.value().to_string(),
            );
        }
    }
}

// Every setting this test cares about, as path -> value, so two files compare as maps.
fn settings(xml: &str) -> BTreeMap<String, String> {
    let document = roxmltree::Document::parse(xml).unwrap();
    let sheet = document.root_element();
    let mut out = BTreeMap::new();
    if let Some(properties) = child(sheet, "sheetPr") {
        for name in ["tabColor", "outlinePr", "pageSetUpPr"] {
            if let Some(node) = child(properties, name) {
                attributes(node, name, &mut out);
            }
        }
    }
    let view = child(sheet, "sheetViews")
        .and_then(|v| child(v, "sheetView"))
        .unwrap();
    for name in [
        "zoomScale",
        "zoomScaleNormal",
        "zoomScalePageLayoutView",
        "view",
    ] {
        if let Some(value) = view.attribute(name) {
            out.insert(format!("sheetView@{name}"), value.to_string());
        }
    }
    for name in [
        "sheetFormatPr",
        "printOptions",
        "pageMargins",
        "pageSetup",
        "headerFooter",
    ] {
        if let Some(node) = child(sheet, name) {
            attributes(node, name, &mut out);
            for text in node.children().filter(roxmltree::Node::is_element) {
                out.insert(
                    format!("{name}/{}", text.tag_name().name()),
                    text.text().unwrap_or_default().to_string(),
                );
            }
        }
    }
    for name in ["rowBreaks", "colBreaks"] {
        if let Some(node) = child(sheet, name) {
            for brk in node.children().filter(|n| n.has_tag_name("brk")) {
                attributes(
                    brk,
                    &format!("{name}/{}", brk.attribute("id").unwrap()),
                    &mut out,
                );
            }
        }
    }
    let rows = child(sheet, "sheetData").unwrap().children();
    for row in rows.filter(roxmltree::Node::is_element) {
        for name in ["outlineLevel", "collapsed", "hidden"] {
            if let Some(value) = row.attribute(name) {
                out.insert(
                    format!("row{}@{name}", row.attribute("r").unwrap()),
                    value.to_string(),
                );
            }
        }
    }
    if let Some(cols) = child(sheet, "cols") {
        for col in cols.children().filter(roxmltree::Node::is_element) {
            let min: u32 = col.attribute("min").unwrap().parse().unwrap();
            let max: u32 = col.attribute("max").unwrap().parse().unwrap();
            for index in min..=max {
                for name in ["outlineLevel", "collapsed", "hidden"] {
                    if let Some(value) = col.attribute(name) {
                        out.insert(format!("col{index}@{name}"), value.to_string());
                    }
                }
            }
        }
    }
    out
}

// Excel writes the deepest outline level on <sheetFormatPr>; rust_xlsxwriter does not, and
// Zenkai adds it, so it is compared apart.
fn without_outline_summary(mut settings: BTreeMap<String, String>) -> BTreeMap<String, String> {
    for name in ["outlineLevelRow", "outlineLevelCol"] {
        let level = settings.remove(&format!("sheetFormatPr@{name}"));
        assert!(
            level.as_deref().is_none_or(|level| level == "1"),
            "{name}: {level:?}"
        );
    }
    settings
}

fn assert_schema_order(xml: &str) {
    let document = roxmltree::Document::parse(xml).unwrap();
    let positions: Vec<usize> = document
        .root_element()
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|n| {
            let name = n.tag_name().name();
            ORDER
                .iter()
                .position(|o| *o == name)
                .unwrap_or_else(|| panic!("unexpected element {name}"))
        })
        .collect();
    assert!(positions.is_sorted(), "children out of order: {xml}");
}

#[test]
fn open_and_save_keeps_layout_view_and_outline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.xlsx");
    layout_source(&path);
    let original = std::fs::read(&path).unwrap();
    let opened = open_xlsx(&path).unwrap();
    assert!(opened.unsupported.is_empty(), "{:?}", opened.unsupported);
    let saved_path = dir.path().join("saved.xlsx");
    save_xlsx_atomic(&opened.workbook, &saved_path).unwrap();
    let saved = std::fs::read(&saved_path).unwrap();

    let before = settings(&part(&original, "xl/worksheets/sheet2.xml"));
    let after = settings(&part(&saved, "xl/worksheets/sheet2.xml"));
    for key in [
        "tabColor@rgb",
        "sheetView@zoomScale",
        "sheetView@view",
        "pageMargins@left",
        "pageSetup@orientation",
        "headerFooter/oddHeader",
        "rowBreaks/6@id",
        "colBreaks/3@id",
        "row8@outlineLevel",
        "col2@outlineLevel",
    ] {
        assert!(
            before.contains_key(key),
            "the source lacks {key}: {before:?}"
        );
    }
    assert_eq!(before, without_outline_summary(after));
    assert_schema_order(&part(&saved, "xl/worksheets/sheet2.xml"));
    assert_eq!(
        settings(&part(&original, "xl/worksheets/sheet1.xml")),
        settings(&part(&saved, "xl/worksheets/sheet1.xml"))
    );

    let again_path = dir.path().join("again.xlsx");
    let reopened = open_xlsx(&saved_path).unwrap();
    save_xlsx_atomic(&reopened.workbook, &again_path).unwrap();
    let again = std::fs::read(&again_path).unwrap();
    for sheet in ["xl/worksheets/sheet1.xml", "xl/worksheets/sheet2.xml"] {
        assert_eq!(
            part(&saved, sheet),
            part(&again, sheet),
            "{sheet} changed on a second save"
        );
    }
}

#[test]
fn settings_follow_a_sheet_that_moves_or_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.xlsx");
    layout_source(&path);
    let original = settings(&part(
        &std::fs::read(&path).unwrap(),
        "xl/worksheets/sheet2.xml",
    ));
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.move_sheet(SheetId(1), 0).unwrap();
    book.rename_sheet(SheetId(0), "Renamed").unwrap();
    book.delete_sheet(SheetId(0)).unwrap();
    book.undo().unwrap();
    let saved = book.to_xlsx().unwrap();
    assert_eq!(
        without_outline_summary(settings(&part(&saved, "xl/worksheets/sheet1.xml"))),
        original
    );
    let plain = settings(&part(&saved, "xl/worksheets/sheet2.xml"));
    assert!(!plain.contains_key("tabColor@rgb"), "{plain:?}");
}

#[test]
fn inserting_above_the_outline_drops_it_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.xlsx");
    layout_source(&path);
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.insert_rows(SheetId(1), RowIdx::new(20).unwrap(), 1)
        .unwrap();
    book.insert_columns(SheetId(1), ColIdx::new(10).unwrap(), 1)
        .unwrap();
    assert!(book.dropped_on_save().is_empty());
    let kept = settings(&part(&book.to_xlsx().unwrap(), "xl/worksheets/sheet2.xml"));
    assert!(kept.contains_key("row8@outlineLevel") && kept.contains_key("rowBreaks/6@id"));

    book.insert_rows(SheetId(1), RowIdx::new(0).unwrap(), 1)
        .unwrap();
    assert_eq!(
        book.dropped_on_save(),
        [Unsupported::Outline, Unsupported::PageBreaks]
    );
    book.undo().unwrap();
    let saved = settings(&part(&book.to_xlsx().unwrap(), "xl/worksheets/sheet2.xml"));
    assert!(
        !saved
            .keys()
            .any(|k| (k.starts_with("row") && !k.ends_with("@hidden"))
                || k == "sheetFormatPr@outlineLevelRow"),
        "{saved:?}"
    );
    assert!(
        saved.contains_key("col2@outlineLevel"),
        "columns were not moved: {saved:?}"
    );
    assert!(saved.contains_key("pageMargins@left"));
}

// Excel writes r:id on <pageSetup> for its printer settings part, which Zenkai does not
// write; a dangling id makes Excel repair the file.
#[test]
fn printer_settings_link_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("layout.xlsx");
    layout_source(&path);
    let bytes = std::fs::read(&path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        if name == "xl/worksheets/sheet2.xml" {
            let text = String::from_utf8(content).unwrap();
            content = text
                .replace("<pageSetup ", r#"<pageSetup r:id="rId9" "#)
                .into_bytes();
        }
        writer
            .start_file(name, zip::write::FileOptions::default())
            .unwrap();
        writer.write_all(&content).unwrap();
    }
    let edited = dir.path().join("edited.xlsx");
    std::fs::write(&edited, writer.finish().unwrap().into_inner()).unwrap();
    let book = open_xlsx(&edited).unwrap().workbook;
    let sheet = part(&book.to_xlsx().unwrap(), "xl/worksheets/sheet2.xml");
    assert!(
        sheet.contains("<pageSetup ") && !sheet.contains("r:id"),
        "{sheet}"
    );
}

#[test]
fn saved_values_are_recalculated_not_the_zeros_cached_in_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zeros.xlsx");
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    sheet.write_number(0, 0, 21.0).unwrap();
    for row in 1..4u32 {
        let formula = rust_xlsxwriter::Formula::new(format!("=A1*{}", row + 1)).set_result("0");
        sheet.write_formula(row, 0, formula).unwrap();
    }
    book.save(&path).unwrap();
    let source = std::fs::read(&path).unwrap();
    assert!(part(&source, "xl/workbook.xml").contains("fullCalcOnLoad"));

    let opened = open_xlsx(&path).unwrap();
    let saved_path = dir.path().join("saved.xlsx");
    save_xlsx_atomic(&opened.workbook, &saved_path).unwrap();
    let saved = part(
        &std::fs::read(&saved_path).unwrap(),
        "xl/worksheets/sheet1.xml",
    );
    let document = roxmltree::Document::parse(&saved).unwrap();
    let values: Vec<f64> = document
        .descendants()
        .filter(|n| n.has_tag_name("c") && n.children().any(|c| c.has_tag_name("f")))
        .filter_map(|n| child(n, "v")?.text()?.parse().ok())
        .collect();
    assert_eq!(values, [42.0, 63.0, 84.0], "{saved}");
}
