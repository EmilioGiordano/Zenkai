// Spec: a save warns before losing content, and only about what it loses. The answer a
// snapshot gives must match the file it writes: a patch save copies every part it does
// not rewrite, a full save writes only what the model holds.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rust_xlsxwriter::{
    Chart, ChartType, DataValidation, DataValidationRule, Formula, Note, Table, Workbook as Source,
    Worksheet,
};
use zenkai_engine::{
    Engine, SaveTarget, Unsupported, Workbook, open_xlsx, save_xlsx, scan_unsupported,
};
use zenkai_types::{CellPos, SheetId};

const HELD_PART: &str = "xl/worksheets/sheet1.xml";
const HELD_RELS: &str = "xl/worksheets/_rels/sheet1.xml.rels";

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

// Checks every save path against the file it wrote and returns what saving after an
// edit on the sheet holding the content says it drops.
fn dropped_by_each_path(source: &Path) -> Vec<Unsupported> {
    let original = std::fs::read(source).unwrap();
    let found = scan(&original);
    assert!(!found.is_empty(), "the source holds nothing to drop");

    let other = saved_after(source, Edit::OtherSheet);
    assert!(other.dropped.is_empty(), "{:?}", other.dropped);
    assert_eq!(part(&other.bytes, HELD_PART), part(&original, HELD_PART));
    assert_eq!(part(&other.bytes, HELD_RELS), part(&original, HELD_RELS));
    assert_eq!(other.kept, scan(&other.bytes));
    assert_eq!(scan(&other.bytes), found);

    let same = saved_after(source, Edit::SameSheet);
    assert_ne!(part(&same.bytes, HELD_PART), part(&original, HELD_PART));
    assert_eq!(part(&same.bytes, HELD_RELS), None);
    assert_eq!(same.kept, scan(&same.bytes));
    let gone: BTreeSet<Unsupported> = set(&found)
        .difference(&set(&scan(&same.bytes)))
        .copied()
        .collect();
    assert!(
        gone.is_subset(&set(&same.dropped)),
        "lost {gone:?}, said {:?}",
        same.dropped
    );

    let full = saved_after(source, Edit::RenameSheet);
    assert_eq!(full.dropped, found);
    assert!(full.kept.is_empty(), "{:?}", full.kept);
    assert!(scan(&full.bytes).is_empty(), "{:?}", scan(&full.bytes));

    same.dropped
}

#[test]
fn an_autofilter_is_dropped_only_with_its_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
    });
    assert_eq!(dropped_by_each_path(&path), [Unsupported::AutoFilter]);
}

#[test]
fn hyperlinks_are_dropped_only_with_their_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.write_url(6, 0, "https://example.com").unwrap();
    });
    assert_eq!(dropped_by_each_path(&path), [Unsupported::Hyperlinks]);
}

#[test]
fn data_validation_is_dropped_only_with_its_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        let rule = DataValidation::new().allow_whole_number(DataValidationRule::Between(1, 10));
        held.add_data_validation(0, 0, 2, 0, &rule).unwrap();
    });
    assert_eq!(dropped_by_each_path(&path), [Unsupported::DataValidation]);
}

#[test]
fn sheet_protection_is_dropped_only_with_its_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.protect();
    });
    assert_eq!(dropped_by_each_path(&path), [Unsupported::SheetProtection]);
}

// The table part stays behind with its own filter, so a reopen still finds both kinds.
#[test]
fn a_table_is_dropped_only_with_its_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.add_table(0, 0, 2, 2, &Table::new()).unwrap();
    });
    assert_eq!(dropped_by_each_path(&path), [Unsupported::Tables]);
}

#[test]
fn comments_are_dropped_only_with_their_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.insert_note(0, 0, &Note::new("checked")).unwrap();
    });
    assert_eq!(
        dropped_by_each_path(&path),
        [Unsupported::Images, Unsupported::Comments]
    );
}

#[test]
fn a_chart_is_dropped_only_with_its_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        let mut chart = Chart::new(ChartType::Column);
        chart.add_series().set_values("Held!$A$1:$A$3");
        held.insert_chart(8, 4, &chart).unwrap();
    });
    assert_eq!(
        dropped_by_each_path(&path),
        [Unsupported::Charts, Unsupported::Images]
    );
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

#[test]
fn a_pivot_table_is_dropped_only_with_its_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_pivot(&path);
    assert_eq!(dropped_by_each_path(&path), [Unsupported::PivotTables]);
}

// A drawing whose relationships cannot be read may hold anything, so the sheet pointing at
// it is taken to own every part no worksheet is known to own.
#[test]
fn a_sheet_with_unreadable_links_is_taken_to_own_everything_outside_the_sheets() {
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
    assert_eq!(dropped_by_each_path(&path), found);
}

// Readers disagree on which of two entries of one name they take, so no save copies from
// such a file and every save reports what it holds.
#[test]
fn relationships_stored_twice_make_every_save_report_everything() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |_| {});
    with_pivot(&path);
    let empty =
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#;
    with_parts(&path, &[("xl/worksheets/_rels/sheet1.xml.rels", "", empty)]);
    let found = scan(&std::fs::read(&path).unwrap());
    assert_eq!(found, [Unsupported::PivotTables]);
    let other = saved_after(&path, Edit::OtherSheet);
    assert_eq!(other.dropped, found);
    assert!(other.kept.is_empty(), "{:?}", other.kept);
    assert!(scan(&other.bytes).is_empty(), "{:?}", scan(&other.bytes));
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
    assert!(dropped_by_each_path(&path).is_empty());
}

// A value that changes through a formula rewrites the sheet holding the formula.
#[test]
fn an_edit_elsewhere_that_changes_a_formula_drops_the_formula_sheet_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = source(dir.path(), |held| {
        held.autofilter(0, 0, 2, 2).unwrap();
    });
    let saved = saved_after(&path, Edit::CrossSheetFormula);
    assert_eq!(saved.dropped, [Unsupported::AutoFilter]);
    assert!(scan(&saved.bytes).is_empty(), "{:?}", scan(&saved.bytes));
    assert!(saved.kept.is_empty(), "{:?}", saved.kept);
}

// The file that warned about pivot tables, a chart and a filter: saving in place after
// edits drops only what the edited sheets owned, and nothing a second time.
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
    assert_eq!(data.dropped, [Unsupported::AutoFilter]);
    assert_eq!(data.kept, scan(&data.bytes));

    book.set_input(SheetId(0), pos("B2"), "8").unwrap();
    let again = save(&book, &path);
    assert!(again.dropped.is_empty(), "{:?}", again.dropped);

    let mut reopened = open_xlsx(&path).unwrap().workbook;
    reopened.set_input(SheetId(0), pos("B3"), "9").unwrap();
    let snapshot = reopened.save_snapshot(&path, SaveTarget::Document).unwrap();
    assert!(snapshot.dropped().is_empty(), "{:?}", snapshot.dropped());
}
