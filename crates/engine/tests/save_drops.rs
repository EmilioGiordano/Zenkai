// Spec: a save warns before losing content, and only about what it loses. The answer a
// snapshot gives must match the file it writes: a patch save copies every part it does
// not rewrite and writes a rewritten sheet back with what an edit cannot invalidate, a
// full save writes only what the model holds.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rust_xlsxwriter::{
    Chart, ChartType, ConditionalFormatCell, ConditionalFormatCellRule, DataValidation,
    DataValidationRule, Format, Formula, Note, Table, Workbook as Source, Worksheet,
};
use zenkai_engine::{
    Engine, SaveTarget, Unsupported, Workbook, open_xlsx, save_xlsx, scan_unsupported,
};
use zenkai_types::{CellPos, RowIdx, SheetId};

const HELD_PART: &str = "xl/worksheets/sheet1.xml";
const HELD_RELS: &str = "xl/worksheets/_rels/sheet1.xml.rels";
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

// The children of CT_Worksheet, in the order the schema requires.
const WORKSHEET_ORDER: [&str; 39] = [
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetCalcPr",
    "sheetProtection",
    "protectedRanges",
    "scenarios",
    "autoFilter",
    "sortState",
    "dataConsolidate",
    "customSheetViews",
    "mergeCells",
    "phoneticPr",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "customProperties",
    "cellWatches",
    "ignoredErrors",
    "smartTags",
    "drawing",
    "legacyDrawing",
    "legacyDrawingHF",
    "drawingHF",
    "picture",
    "oleObjects",
    "controls",
    "webPublishItems",
    "tableParts",
    "extLst",
];

#[derive(Clone, Copy, Debug)]
enum Edit {
    OtherSheet,
    SameSheet,
    CrossSheetFormula,
    RenameSheet,
}

struct Saved {
    dropped: Vec<Unsupported>,
    kept: Vec<Unsupported>,
    bytes: Vec<u8>,
    path: PathBuf,
}

// "Held" carries the content under test; "Other" is a plain sheet.
fn source(dir: &Path, add: impl FnOnce(&mut Worksheet)) -> PathBuf {
    let path = dir.join("source.xlsx");
    let mut book = Source::new();
    let held = book.add_worksheet().set_name("Held").unwrap();
    for row in 0..3 {
        for col in 0..3 {
            held.write_number(row, col, f64::from(row * 3 + u32::from(col)))
                .unwrap();
        }
    }
    let doubled = Formula::new("=Other!A1*2").set_result("2");
    held.write_formula(4, 1, doubled).unwrap();
    add(held);
    let other = book.add_worksheet().set_name("Other").unwrap();
    other.write_number(0, 0, 1.0).unwrap();
    book.save(&path).unwrap();
    path
}

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            (entry.name().to_string(), content)
        })
        .collect()
}

fn part(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    entries(bytes)
        .into_iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, content)| content)
}

fn write_entries(path: &Path, entries: &[(String, Vec<u8>)]) {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in entries {
        writer.start_file(name.as_str(), options).unwrap();
        writer.write_all(content).unwrap();
    }
    std::fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
}

// Adds parts no writer used here produces, each declared in the content types.
fn with_parts(path: &Path, added: &[(&str, &str, &str)]) {
    let mut parts = entries(&std::fs::read(path).unwrap());
    let types = parts
        .iter_mut()
        .find(|(name, _)| name == "[Content_Types].xml")
        .unwrap();
    let overrides: String = added
        .iter()
        .filter(|(name, _, _)| !name.ends_with(".rels"))
        .map(|(name, content_type, _)| {
            format!(r#"<Override PartName="/{name}" ContentType="{content_type}"/>"#)
        })
        .collect();
    let declared = String::from_utf8(types.1.clone())
        .unwrap()
        .replace("</Types>", &format!("{overrides}</Types>"));
    types.1 = declared.into_bytes();
    parts.extend(
        added
            .iter()
            .map(|(name, _, content)| (name.to_string(), content.as_bytes().to_vec())),
    );
    write_entries(path, &parts);
}

// Replaces the first occurrence of each text in the held sheet's part.
fn with_held_sheet_replaced(path: &Path, replaced: &[(&str, &str)]) {
    let mut parts = entries(&std::fs::read(path).unwrap());
    let sheet = parts
        .iter_mut()
        .find(|(name, _)| name == HELD_PART)
        .unwrap();
    let mut text = String::from_utf8(sheet.1.clone()).unwrap();
    for (from, to) in replaced {
        assert!(text.contains(from), "{from}");
        text = text.replacen(from, to, 1);
    }
    sheet.1 = text.into_bytes();
    write_entries(path, &parts);
}

fn scan(bytes: &[u8]) -> Vec<Unsupported> {
    scan_unsupported(bytes).unwrap()
}

fn pos(a1: &str) -> CellPos {
    CellPos::parse_a1(a1).unwrap()
}

fn save(book: &Workbook, path: &Path) -> Saved {
    let snapshot = book.save_snapshot(path, SaveTarget::Document).unwrap();
    let dropped = snapshot.dropped().to_vec();
    let kept = snapshot.kept().to_vec();
    save_xlsx(snapshot).unwrap();
    Saved {
        dropped,
        kept,
        bytes: std::fs::read(path).unwrap(),
        path: path.to_path_buf(),
    }
}

fn saved_after(source: &Path, edit: Edit) -> Saved {
    let mut book = open_xlsx(source).unwrap().workbook;
    match edit {
        Edit::OtherSheet => book.set_input(SheetId(1), pos("D10"), "5"),
        Edit::SameSheet => book.set_input(SheetId(0), pos("D10"), "5"),
        Edit::CrossSheetFormula => book.set_input(SheetId(1), pos("A1"), "5"),
        Edit::RenameSheet => book.rename_sheet(SheetId(1), "Renamed"),
    }
    .unwrap();
    save(&book, &source.with_file_name(format!("{edit:?}.xlsx")))
}

fn set(kinds: &[Unsupported]) -> BTreeSet<Unsupported> {
    kinds.iter().copied().collect()
}

fn held_sheet(bytes: &[u8]) -> String {
    String::from_utf8(part(bytes, HELD_PART).unwrap()).unwrap()
}

// The names of the sheet's child elements, in file order.
fn children(sheet: &str) -> Vec<String> {
    let document = roxmltree::Document::parse(sheet).unwrap();
    document
        .root_element()
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|node| node.tag_name().name().to_string())
        .collect()
}

fn referenced_ids(sheet: &str) -> BTreeSet<String> {
    let document = roxmltree::Document::parse(sheet).unwrap();
    document
        .descendants()
        .flat_map(|node| node.attributes())
        .filter(|attribute| attribute.namespace() == Some(RELATIONSHIPS))
        .map(|attribute| attribute.value().to_string())
        .collect()
}

fn declared_ids(bytes: &[u8]) -> BTreeSet<String> {
    let Some(rels) = part(bytes, HELD_RELS) else {
        return BTreeSet::new();
    };
    let rels = String::from_utf8(rels).unwrap();
    let document = roxmltree::Document::parse(&rels).unwrap();
    document
        .descendants()
        .filter(|node| node.has_tag_name("Relationship"))
        .map(|node| node.attribute("Id").unwrap().to_string())
        .collect()
}

fn resolved(dir: &str, target: &str) -> String {
    let mut path: Vec<&str> = match target.starts_with('/') {
        true => Vec::new(),
        false => dir.split('/').filter(|s| !s.is_empty()).collect(),
    };
    for segment in target.trim_start_matches('/').split('/') {
        match segment {
            ".." => {
                path.pop();
            }
            "." => {}
            segment => path.push(segment),
        }
    }
    path.join("/")
}

// Parts no relationship points at.
fn orphans(bytes: &[u8]) -> BTreeSet<String> {
    let parts = entries(bytes);
    let mut targets = BTreeSet::new();
    for (name, content) in &parts {
        let Some((dir, _)) = name.rsplit_once("_rels/") else {
            continue;
        };
        let text = String::from_utf8_lossy(content);
        let Ok(document) = roxmltree::Document::parse(&text) else {
            continue;
        };
        for relationship in document
            .descendants()
            .filter(|node| node.has_tag_name("Relationship"))
            .filter(|node| node.attribute("TargetMode") != Some("External"))
        {
            targets.insert(resolved(dir, relationship.attribute("Target").unwrap()));
        }
    }
    parts
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !name.contains("_rels/") && name != "[Content_Types].xml")
        .filter(|name| !targets.contains(name))
        .collect()
}

// Every r:id the rewritten sheet holds resolves, no part it pointed at is left
// unreferenced, and the file opens again.
fn assert_sound(saved: &Saved, original: &[u8]) {
    let sheet = held_sheet(&saved.bytes);
    let ids = referenced_ids(&sheet);
    let declared = declared_ids(&saved.bytes);
    assert!(ids.is_subset(&declared), "{ids:?} not in {declared:?}");
    let orphans_before = orphans(original);
    let orphans_after = orphans(&saved.bytes);
    assert!(
        orphans_after.is_subset(&orphans_before),
        "{orphans_after:?} were {orphans_before:?}"
    );
    open_xlsx(&saved.path).unwrap();
}

// Checks every save path against the file it wrote. An edit on the sheet holding the
// content keeps it: the relationships part goes along unchanged, and `element`, the sheet
// element holding the content or pointing at it, is written back.
fn kept_by_each_path(source: &Path, element: Option<&str>) {
    let original = std::fs::read(source).unwrap();
    let found = scan(&original);
    assert!(!found.is_empty(), "the source holds nothing to drop");
    if let Some(element) = element {
        assert!(
            children(&held_sheet(&original))
                .iter()
                .any(|e| e == element)
        );
    }

    let other = saved_after(source, Edit::OtherSheet);
    assert!(other.dropped.is_empty(), "{:?}", other.dropped);
    assert_eq!(part(&other.bytes, HELD_PART), part(&original, HELD_PART));
    assert_eq!(part(&other.bytes, HELD_RELS), part(&original, HELD_RELS));
    assert_eq!(other.kept, scan(&other.bytes));
    assert_eq!(scan(&other.bytes), found);

    let same = saved_after(source, Edit::SameSheet);
    assert!(same.dropped.is_empty(), "{:?}", same.dropped);
    assert_ne!(part(&same.bytes, HELD_PART), part(&original, HELD_PART));
    assert_eq!(part(&same.bytes, HELD_RELS), part(&original, HELD_RELS));
    assert_eq!(same.kept, scan(&same.bytes));
    assert_eq!(scan(&same.bytes), found);
    if let Some(element) = element {
        let written = children(&held_sheet(&same.bytes));
        assert!(written.iter().any(|e| e == element), "{written:?}");
    }
    assert_sound(&same, &original);

    let full = saved_after(source, Edit::RenameSheet);
    assert_eq!(full.dropped, found);
    assert!(full.kept.is_empty(), "{:?}", full.kept);
    assert!(scan(&full.bytes).is_empty(), "{:?}", scan(&full.bytes));
}

#[test]
fn an_autofilter_is_kept_when_its_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
    });
    kept_by_each_path(&path, Some("autoFilter"));
}

#[test]
fn hyperlinks_are_kept_when_their_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.write_url(6, 0, "https://example.com").unwrap();
    });
    kept_by_each_path(&path, Some("hyperlinks"));
}

#[test]
fn data_validation_is_kept_when_its_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        let rule = DataValidation::new().allow_whole_number(DataValidationRule::Between(1, 10));
        held.add_data_validation(0, 0, 2, 0, &rule).unwrap();
    });
    kept_by_each_path(&path, Some("dataValidations"));
}

// Excel writes list validations naming another sheet into the sheet's extension list,
// which is not kept, so the sheet's validations all go and the save says so.
#[test]
fn validations_excel_extends_are_dropped_and_said_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        let rule = DataValidation::new().allow_whole_number(DataValidationRule::Between(1, 10));
        held.add_data_validation(0, 0, 2, 0, &rule).unwrap();
    });
    with_held_sheet_replaced(
        &path,
        &[(
            "</worksheet>",
            concat!(
                r#"<extLst><ext uri="{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}" "#,
                r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
                r#"<x14:dataValidations count="1"/></ext></extLst></worksheet>"#
            ),
        )],
    );
    let same = saved_after(&path, Edit::SameSheet);
    assert_eq!(same.dropped, [Unsupported::DataValidation]);
    assert_eq!(same.kept, scan(&same.bytes));
    let written = children(&held_sheet(&same.bytes));
    assert!(
        !written.iter().any(|e| e == "dataValidations"),
        "{written:?}"
    );
}

#[test]
fn sheet_protection_is_kept_when_its_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.protect();
    });
    kept_by_each_path(&path, Some("sheetProtection"));
}

fn with_table(held: &mut Worksheet) {
    held.add_table(0, 0, 2, 2, &Table::new()).unwrap();
}

#[test]
fn a_table_is_kept_when_its_body_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), with_table);
    kept_by_each_path(&path, Some("tableParts"));
}

#[test]
fn comments_are_kept_when_their_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.insert_note(0, 0, &Note::new("checked")).unwrap();
    });
    kept_by_each_path(&path, Some("legacyDrawing"));
}

#[test]
fn a_chart_is_kept_when_its_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        let mut chart = Chart::new(ChartType::Column);
        chart.add_series().set_values("Held!$A$1:$A$3");
        held.insert_chart(8, 4, &chart).unwrap();
    });
    kept_by_each_path(&path, Some("drawing"));
}

const PIVOT_TABLE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Pivot1" cacheId="1" dataCaption="Values"><location ref="E1:F3" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/></pivotTableDefinition>"#;
const PIVOT_CACHE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cacheSource type="worksheet"><worksheetSource ref="A1:C3" sheet="Held"/></cacheSource></pivotCacheDefinition>"#;
const PIVOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable" Target="../pivotTables/pivotTable1.xml"/></Relationships>"#;

fn with_pivot(path: &Path) {
    with_parts(
        path,
        &[
            (
                "xl/pivotTables/pivotTable1.xml",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml",
                PIVOT_TABLE,
            ),
            (
                "xl/pivotCache/pivotCacheDefinition1.xml",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml",
                PIVOT_CACHE,
            ),
            ("xl/worksheets/_rels/sheet1.xml.rels", "", PIVOT_RELS),
        ],
    );
}

// No sheet element points at a pivot table; the relationships part alone keeps it.
#[test]
fn a_pivot_table_is_kept_when_its_sheet_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_pivot(&path);
    kept_by_each_path(&path, None);
}

// A drawing whose relationships cannot be read may hold anything, so the sheet pointing at
// it is taken to own every part no worksheet is known to own; its relationships part goes
// along unchanged, so an edit on it loses none of them.
#[test]
fn a_sheet_with_unreadable_links_keeps_everything_outside_the_sheets() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_parts(
        &path,
        &[
            (
                "xl/drawings/drawing1.xml",
                "application/vnd.openxmlformats-officedocument.drawing+xml",
                r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"/>"#,
            ),
            ("xl/drawings/_rels/drawing1.xml.rels", "", "not <XML"),
            (
                "xl/worksheets/_rels/sheet1.xml.rels",
                "",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#,
            ),
            (
                "xl/pivotTables/pivotTable1.xml",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml",
                PIVOT_TABLE,
            ),
        ],
    );
    let found = scan(&std::fs::read(&path).unwrap());
    assert_eq!(found, [Unsupported::Images, Unsupported::PivotTables]);
    kept_by_each_path(&path, None);
}

// No worksheet owns it, so only a full save drops it.
#[test]
fn an_external_link_is_dropped_only_by_a_full_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_parts(
        &path,
        &[(
            "xl/externalLinks/externalLink1.xml",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml",
            r#"<externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>"#,
        )],
    );
    kept_by_each_path(&path, None);
}

// A value that changes through a formula rewrites the sheet holding the formula, which
// keeps what it held.
#[test]
fn an_edit_elsewhere_that_changes_a_formula_keeps_the_formula_sheet_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
    });
    let original = std::fs::read(&path).unwrap();
    let saved = saved_after(&path, Edit::CrossSheetFormula);
    assert_ne!(part(&saved.bytes, HELD_PART), part(&original, HELD_PART));
    assert!(saved.dropped.is_empty(), "{:?}", saved.dropped);
    assert_eq!(scan(&saved.bytes), [Unsupported::AutoFilter]);
    assert_eq!(saved.kept, scan(&saved.bytes));
}

fn inserted_row(source: &Path, at: i64, name: &str) -> Saved {
    let mut book = open_xlsx(source).unwrap().workbook;
    book.insert_rows(SheetId(0), RowIdx::clamped(at), 1)
        .unwrap();
    save(&book, &source.with_file_name(name))
}

// Zenkai cannot shift the range a filter names, as Excel would, so a row inserted through
// it drops the filter; one inserted below it leaves the filter right.
#[test]
fn inserting_rows_through_a_filter_drops_it_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
    });
    let original = std::fs::read(&path).unwrap();

    let below = inserted_row(&path, 5, "below.xlsx");
    assert!(below.dropped.is_empty(), "{:?}", below.dropped);
    assert_eq!(below.kept, scan(&below.bytes));
    assert!(
        children(&held_sheet(&below.bytes))
            .iter()
            .any(|e| e == "autoFilter")
    );
    assert_sound(&below, &original);

    let through = inserted_row(&path, 1, "through.xlsx");
    assert_eq!(through.dropped, [Unsupported::AutoFilter]);
    assert_eq!(through.kept, scan(&through.bytes));
    assert!(
        scan(&through.bytes).is_empty(),
        "{:?}",
        scan(&through.bytes)
    );
    assert!(
        !children(&held_sheet(&through.bytes))
            .iter()
            .any(|e| e == "autoFilter")
    );
    open_xlsx(&through.path).unwrap();
}

// A table part names its range and the parts a drawing anchors name cells, so any row
// inserted on the sheet drops its relationships and what they point at.
#[test]
fn inserting_a_row_on_a_table_sheet_drops_its_tables_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), with_table);
    let saved = inserted_row(&path, 20, "inserted.xlsx");
    assert!(
        saved.dropped.contains(&Unsupported::Tables),
        "{:?}",
        saved.dropped
    );
    assert_eq!(saved.kept, scan(&saved.bytes));
    assert_eq!(part(&saved.bytes, HELD_RELS), None);
    assert!(referenced_ids(&held_sheet(&saved.bytes)).is_empty());
    open_xlsx(&saved.path).unwrap();
}

// Excel repairs a file whose table header cells differ from the column names the table
// part holds, and the part is copied unchanged.
#[test]
fn editing_a_table_header_drops_its_tables_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), with_table);
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.set_input(SheetId(0), pos("A1"), "Renamed").unwrap();
    let saved = save(&book, &dir.path().join("header.xlsx"));
    assert!(
        saved.dropped.contains(&Unsupported::Tables),
        "{:?}",
        saved.dropped
    );
    assert_eq!(saved.kept, scan(&saved.bytes));
    assert_eq!(part(&saved.bytes, HELD_RELS), None);
    assert!(referenced_ids(&held_sheet(&saved.bytes)).is_empty());
    open_xlsx(&saved.path).unwrap();
}

// A whole save writes no sheet relationships, so a later patch save into that file must
// not write back an element that points into them.
#[test]
fn a_patch_save_after_a_whole_save_brings_nothing_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
        let mut chart = Chart::new(ChartType::Column);
        chart.add_series().set_values("Held!$A$1:$A$3");
        held.insert_chart(8, 4, &chart).unwrap();
    });
    let found = scan(&std::fs::read(&path).unwrap());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.rename_sheet(SheetId(1), "Renamed").unwrap();
    let full = save(&book, &path);
    assert_eq!(full.dropped, found);

    book.set_input(SheetId(0), pos("D10"), "5").unwrap();
    let patched = save(&book, &path);
    assert!(patched.dropped.is_empty(), "{:?}", patched.dropped);
    assert!(
        scan(&patched.bytes).is_empty(),
        "{:?}",
        scan(&patched.bytes)
    );
    assert_eq!(part(&patched.bytes, HELD_RELS), None);
    let sheet = held_sheet(&patched.bytes);
    assert!(referenced_ids(&sheet).is_empty(), "{sheet}");
    assert!(
        !children(&sheet).iter().any(|e| e == "autoFilter"),
        "{sheet}"
    );
    open_xlsx(&patched.path).unwrap();
}

#[test]
fn a_rewritten_sheet_holds_its_elements_in_schema_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.protect();
        held.autofilter(0, 0, 2, 2).unwrap();
        held.merge_range(10, 0, 10, 2, "merged", &Format::new())
            .unwrap();
        let red = Format::new().set_font_color("#C00000");
        let rule = ConditionalFormatCell::new()
            .set_rule(ConditionalFormatCellRule::GreaterThan(3))
            .set_format(&red);
        held.add_conditional_format(0, 0, 2, 0, &rule).unwrap();
        let validation =
            DataValidation::new().allow_whole_number(DataValidationRule::Between(1, 10));
        held.add_data_validation(0, 1, 2, 1, &validation).unwrap();
        held.write_url(6, 0, "https://example.com").unwrap();
        held.set_landscape();
        held.set_margins(0.5, 0.5, 0.8, 0.8, 0.3, 0.3);
        held.set_page_breaks(&[6]).unwrap();
        let mut chart = Chart::new(ChartType::Column);
        chart.add_series().set_values("Held!$A$1:$A$3");
        held.insert_chart(12, 4, &chart).unwrap();
        held.insert_note(1, 0, &Note::new("checked")).unwrap();
    });
    let original = std::fs::read(&path).unwrap();
    let same = saved_after(&path, Edit::SameSheet);
    assert!(same.dropped.is_empty(), "{:?}", same.dropped);
    let names = children(&held_sheet(&same.bytes));
    for expected in [
        "sheetProtection",
        "autoFilter",
        "mergeCells",
        "dataValidations",
        "hyperlinks",
        "pageMargins",
        "rowBreaks",
        "drawing",
        "legacyDrawing",
    ] {
        assert!(
            names.iter().any(|e| e == expected),
            "{expected} in {names:?}"
        );
    }
    let order: Vec<usize> = names
        .iter()
        .map(|name| WORKSHEET_ORDER.iter().position(|o| o == name).unwrap())
        .collect();
    assert!(order.is_sorted(), "{names:?}");
    assert_sound(&same, &original);
}

// Excel writes revision ids and markup-compatibility declarations the rewritten sheet's
// root does not declare; they are left out, and the element is kept.
#[test]
fn foreign_attributes_on_a_kept_element_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
        held.write_url(6, 0, "https://example.com").unwrap();
    });
    with_held_sheet_replaced(
        &path,
        &[
            (
                "<worksheet ",
                concat!(
                    r#"<worksheet xmlns:xmc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
                    r#"xmlns:xr="http://schemas.microsoft.com/office/spreadsheetml/2014/revision" "#,
                    r#"xmc:Ignorable="xr" "#
                ),
            ),
            (
                "<autoFilter ",
                r#"<autoFilter xr:uid="{00000000-0000-0000-0000-000000000001}" "#,
            ),
            (
                "<hyperlink ",
                r#"<hyperlink xr:uid="{00000000-0000-0000-0000-000000000002}" "#,
            ),
        ],
    );
    let original = std::fs::read(&path).unwrap();
    let same = saved_after(&path, Edit::SameSheet);
    assert!(same.dropped.is_empty(), "{:?}", same.dropped);
    let sheet = held_sheet(&same.bytes);
    assert!(!sheet.contains("uid="), "{sheet}");
    let names = children(&sheet);
    assert!(names.iter().any(|e| e == "autoFilter"), "{names:?}");
    assert!(names.iter().any(|e| e == "hyperlinks"), "{names:?}");
    assert_sound(&same, &original);
}

// The file that warned about pivot tables, a chart and a filter: saving in place after
// edits drops nothing the edited sheets can keep, and nothing a second time.
#[test]
fn saving_in_place_names_each_loss_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.xlsx");
    let mut source = Source::new();
    let data = source.add_worksheet().set_name("Data").unwrap();
    for row in 0..3 {
        data.write_number(row, 0, f64::from(row)).unwrap();
    }
    data.autofilter(0, 0, 2, 0).unwrap();
    source.add_worksheet().set_name("Pivot").unwrap();
    let charted = source.add_worksheet().set_name("Charted").unwrap();
    let mut chart = Chart::new(ChartType::Column);
    chart.add_series().set_values("Data!$A$1:$A$3");
    charted.insert_chart(0, 0, &chart).unwrap();
    source.add_worksheet().set_name("Plain").unwrap();
    source.save(&path).unwrap();
    with_parts(
        &path,
        &[
            (
                "xl/pivotTables/pivotTable1.xml",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml",
                PIVOT_TABLE,
            ),
            ("xl/worksheets/_rels/sheet2.xml.rels", "", PIVOT_RELS),
        ],
    );
    let all = [
        Unsupported::Charts,
        Unsupported::Images,
        Unsupported::PivotTables,
        Unsupported::AutoFilter,
    ];
    assert_eq!(set(&scan(&std::fs::read(&path).unwrap())), set(&all));

    let mut book = open_xlsx(&path).unwrap().workbook;
    book.set_input(SheetId(3), pos("A1"), "plain").unwrap();
    let plain = save(&book, &path);
    assert!(plain.dropped.is_empty(), "{:?}", plain.dropped);
    assert_eq!(set(&plain.kept), set(&all));

    book.set_input(SheetId(0), pos("B1"), "7").unwrap();
    let data = save(&book, &path);
    assert!(data.dropped.is_empty(), "{:?}", data.dropped);
    assert_eq!(set(&data.kept), set(&all));
    assert_eq!(data.kept, scan(&data.bytes));

    book.insert_rows(SheetId(0), RowIdx::clamped(0), 1).unwrap();
    let moved = save(&book, &path);
    assert_eq!(moved.dropped, [Unsupported::AutoFilter]);
    assert_eq!(moved.kept, scan(&moved.bytes));

    book.set_input(SheetId(0), pos("B2"), "8").unwrap();
    let again = save(&book, &path);
    assert!(again.dropped.is_empty(), "{:?}", again.dropped);

    let mut reopened = open_xlsx(&path).unwrap().workbook;
    reopened.set_input(SheetId(0), pos("B3"), "9").unwrap();
    let snapshot = reopened.save_snapshot(&path, SaveTarget::Document).unwrap();
    assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
}
