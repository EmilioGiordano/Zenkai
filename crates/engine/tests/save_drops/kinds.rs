// Content Zenkai reads past and never writes back. A save keeps it only by copying the
// part holding it from the file, and says so whenever it writes that part itself.

use std::path::{Path, PathBuf};

use rust_xlsxwriter::{ConditionalFormatDataBar, Sparkline};
use zenkai_engine::{Unsupported, open_xlsx};

use super::{
    Edit, HELD_PART, assert_sound, held_sheet, kept_by_each_path, part, saved_after, scan, source,
    with_held_sheet_replaced, with_part_replaced, with_parts,
};

const WORKBOOK_PART: &str = "xl/workbook.xml";

fn warned_on_open(path: &Path) -> Vec<Unsupported> {
    let found = scan(&std::fs::read(path).unwrap());
    assert_eq!(open_xlsx(path).unwrap().unsupported, found);
    found
}

// A save that copies the held sheet keeps the content; one that writes the sheet again
// leaves out `marker`, the text holding it, and says it drops `kind`, as a whole save does.
fn dropped_when_its_sheet_is_written(source: &Path, kind: Unsupported, marker: &str) {
    let original = std::fs::read(source).unwrap();
    let found = warned_on_open(source);
    assert!(found.contains(&kind), "{found:?}");
    assert!(held_sheet(&original).contains(marker), "{marker}");

    let other = saved_after(source, Edit::OtherSheet);
    assert!(other.dropped.is_empty(), "{:?}", other.dropped);
    assert_eq!(part(&other.bytes, HELD_PART), part(&original, HELD_PART));
    assert_eq!(other.kept, scan(&other.bytes));
    assert_eq!(scan(&other.bytes), found);

    let same = saved_after(source, Edit::SameSheet);
    assert_eq!(same.dropped, [kind]);
    assert_eq!(same.kept, scan(&same.bytes));
    let sheet = held_sheet(&same.bytes);
    assert!(!sheet.contains(marker), "{sheet}");
    assert_sound(&same, &original);

    let full = saved_after(source, Edit::RenameSheet);
    assert_eq!(full.dropped, found);
    assert!(full.kept.is_empty(), "{:?}", full.kept);
    assert!(scan(&full.bytes).is_empty(), "{:?}", scan(&full.bytes));
}

fn after_cells(dir: &Path, element: &str) -> PathBuf {
    let path = source(dir, |_| {});
    let element = format!("</sheetData>{element}");
    with_held_sheet_replaced(&path, &[("</sheetData>", element.as_str())]);
    path
}

fn at_the_end(dir: &Path, element: &str) -> PathBuf {
    let path = source(dir, |_| {});
    let element = format!("{element}</worksheet>");
    with_held_sheet_replaced(&path, &[("</worksheet>", element.as_str())]);
    path
}

#[test]
fn sparklines_are_reported_when_their_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        let sparkline = Sparkline::new().set_range("Held!$A$1:$C$1");
        held.add_sparkline(0, 4, &sparkline).unwrap();
    });
    dropped_when_its_sheet_is_written(&path, Unsupported::Sparklines, "sparklineGroups");
}

// Excel writes a rule comparing with another sheet into the extension list, where IronCalc
// reads only data bars, icon sets and expressions.
#[test]
fn an_extended_rule_ironcalc_does_not_read_is_reported_when_its_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let rule = concat!(
        r#"<extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}" "#,
        r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
        r#"<x14:conditionalFormattings><x14:conditionalFormatting "#,
        r#"xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main">"#,
        r#"<x14:cfRule type="cellIs" priority="1" operator="greaterThan" "#,
        r#"id="{00000000-0000-0000-0000-000000000001}"><xm:f>Other!$A$1</xm:f>"#,
        r#"<x14:dxf><font><b/></font></x14:dxf></x14:cfRule><xm:sqref>A1:C3</xm:sqref>"#,
        r#"</x14:conditionalFormatting></x14:conditionalFormattings></ext></extLst>"#
    );
    let path = at_the_end(dir.path(), rule);
    dropped_when_its_sheet_is_written(
        &path,
        Unsupported::ExtendedConditionalFormats,
        "conditionalFormattings",
    );
}

// IronCalc reads a data bar's extension and writes it back, so it is not reported.
#[test]
fn a_data_bar_with_its_extension_is_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.add_conditional_format(0, 0, 2, 0, &ConditionalFormatDataBar::new())
            .unwrap();
    });
    let original = std::fs::read(&path).unwrap();
    assert!(held_sheet(&original).contains("conditionalFormattings"));
    assert!(warned_on_open(&path).is_empty());
    let full = saved_after(&path, Edit::RenameSheet);
    assert!(full.dropped.is_empty(), "{:?}", full.dropped);
}

const SLICER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.microsoft.com/office/2007/relationships/slicer" Target="../slicers/slicer1.xml"/></Relationships>"#;
const SLICER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<slicers xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><slicer name="Region" cache="Slicer_Region" caption="Region" rowHeight="241300"/></slicers>"#;
const SLICER_CACHE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<slicerCacheDefinition xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" name="Slicer_Region" sourceName="Region"/>"#;

// The slicer parts stay in the file, but a sheet written again without its slicer list no
// longer shows them.
#[test]
fn a_slicer_is_reported_when_its_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let list = concat!(
        r#"<extLst><ext uri="{A8765BA9-456A-4dab-B4F3-ACF838C121DE}" "#,
        r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
        r#"<x14:slicerList><x14:slicer r:id="rId1"/></x14:slicerList></ext></extLst>"#
    );
    let path = at_the_end(dir.path(), list);
    with_parts(
        &path,
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", "", SLICER_RELS),
            (
                "xl/slicers/slicer1.xml",
                "application/vnd.ms-excel.slicer+xml",
                SLICER,
            ),
            (
                "xl/slicerCaches/slicerCache1.xml",
                "application/vnd.ms-excel.slicerCache+xml",
                SLICER_CACHE,
            ),
        ],
    );
    dropped_when_its_sheet_is_written(&path, Unsupported::Slicers, "slicerList");
}

#[test]
fn custom_sheet_views_are_reported_when_their_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = after_cells(
        dir.path(),
        r#"<customSheetViews><customSheetView guid="{00000000-0000-0000-0000-000000000001}" scale="90"/></customSheetViews>"#,
    );
    dropped_when_its_sheet_is_written(&path, Unsupported::CustomViews, "customSheetViews");
}

#[test]
fn phonetic_settings_are_reported_when_their_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = after_cells(
        dir.path(),
        r#"<phoneticPr fontId="0" type="noConversion"/>"#,
    );
    dropped_when_its_sheet_is_written(&path, Unsupported::Phonetic, "phoneticPr");
}

#[test]
fn scenarios_are_reported_when_their_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = after_cells(
        dir.path(),
        r#"<scenarios><scenario name="High" count="1" user="Zenkai"><inputCells r="A1" val="20"/></scenario></scenarios>"#,
    );
    dropped_when_its_sheet_is_written(&path, Unsupported::Scenarios, "<scenarios");
}

#[test]
fn cell_watches_are_reported_when_their_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = at_the_end(
        dir.path(),
        r#"<cellWatches><cellWatch r="A1"/></cellWatches>"#,
    );
    dropped_when_its_sheet_is_written(&path, Unsupported::CellWatches, "cellWatches");
}

const PRINTER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/printerSettings" Target="../printerSettings/printerSettings1.bin"/></Relationships>"#;

// The page setup is written back without the link to its printer settings, which stay in
// the file unused.
#[test]
fn printer_settings_are_reported_when_their_sheet_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.set_paper_size(9);
    });
    with_held_sheet_replaced(&path, &[("<pageSetup ", r#"<pageSetup r:id="rId1" "#)]);
    with_parts(
        &path,
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", "", PRINTER_RELS),
            (
                "xl/printerSettings/printerSettings1.bin",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.printerSettings",
                "printer",
            ),
        ],
    );
    dropped_when_its_sheet_is_written(&path, Unsupported::PrinterSettings, "r:id=");
}

const CONNECTIONS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<connections xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><connection id="1" name="data" type="6" refreshedVersion="6" background="1" saveData="1"><textPr codePage="65001" sourceFile="C:\data.csv" comma="1"/></connection></connections>"#;
const CONNECTIONS_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.connections+xml";

#[test]
fn connections_are_dropped_only_by_a_full_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_parts(
        &path,
        &[("xl/connections.xml", CONNECTIONS_TYPE, CONNECTIONS)],
    );
    assert_eq!(warned_on_open(&path), [Unsupported::Connections]);
    kept_by_each_path(&path, None);
}

const QUERY_TABLE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<queryTable xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="data" connectionId="1" autoFormatId="16" applyNumberFormats="0" applyBorderFormats="0" applyFontFormats="0" applyPatternFormats="0" applyAlignmentFormats="0" applyWidthHeightFormats="0"/>"#;
const QUERY_TABLE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/queryTable" Target="../queryTables/queryTable1.xml"/></Relationships>"#;

// The sheet's relationships point at a kind no reader lists, so it is taken to own every
// part outside the sheets; its relationships go along with it, so an edit loses none.
#[test]
fn a_query_table_is_dropped_only_by_a_full_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_parts(
        &path,
        &[
            ("xl/connections.xml", CONNECTIONS_TYPE, CONNECTIONS),
            (
                "xl/queryTables/queryTable1.xml",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.queryTable+xml",
                QUERY_TABLE,
            ),
            ("xl/worksheets/_rels/sheet1.xml.rels", "", QUERY_TABLE_RELS),
        ],
    );
    assert_eq!(
        warned_on_open(&path),
        [Unsupported::Connections, Unsupported::QueryTables]
    );
    kept_by_each_path(&path, None);
}

#[test]
fn custom_xml_is_dropped_only_by_a_full_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_parts(
        &path,
        &[
            (
                "customXml/item1.xml",
                "application/xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><properties xmlns="urn:zenkai:test"><owner>Finance</owner></properties>"#,
            ),
            (
                "customXml/itemProps1.xml",
                "application/vnd.openxmlformats-officedocument.customXmlProperties+xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:datastoreItem ds:itemID="{00000000-0000-0000-0000-000000000001}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs/></ds:datastoreItem>"#,
            ),
        ],
    );
    assert_eq!(warned_on_open(&path), [Unsupported::CustomXml]);
    kept_by_each_path(&path, None);
}

#[test]
fn a_data_model_is_dropped_only_by_a_full_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_parts(
        &path,
        &[(
            "xl/model/item.data",
            "application/vnd.openxmlformats-officedocument.model+data",
            "model",
        )],
    );
    assert_eq!(warned_on_open(&path), [Unsupported::DataModel]);
    kept_by_each_path(&path, None);
}

#[test]
fn workbook_protection_and_custom_views_are_dropped_only_by_a_full_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_part_replaced(
        &path,
        WORKBOOK_PART,
        &[
            (
                "<bookViews>",
                r#"<workbookProtection lockStructure="1"/><bookViews>"#,
            ),
            (
                "</workbook>",
                concat!(
                    r#"<customWorkbookViews><customWorkbookView name="Review" "#,
                    r#"guid="{00000000-0000-0000-0000-000000000001}" windowWidth="16000" "#,
                    r#"windowHeight="9000" activeSheetId="1"/></customWorkbookViews></workbook>"#
                ),
            ),
        ],
    );
    assert_eq!(
        warned_on_open(&path),
        [Unsupported::CustomViews, Unsupported::WorkbookProtection]
    );
    kept_by_each_path(&path, None);
}
