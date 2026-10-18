// Spec: a save after a small edit rewrites only what changed and copies the rest of the
// file. A file saved that way must reopen exactly as one written whole from the same
// workbook, whatever the edits, and a part no edit touched must stay byte for byte. What
// each save says it keeps is what the file holds, and every relationship a rewritten
// sheet names is still there.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use proptest::prelude::*;
use rust_xlsxwriter::{
    Chart, ChartType, ConditionalFormatCell, ConditionalFormatCellRule, Format, Formula, Table,
    Workbook as Source,
};
use zenkai_engine::{
    Engine, EngineError, SaveTarget, Workbook, XlsxReader, open_xlsx, open_xlsx_with, save_xlsx,
    scan_unsupported,
};
use zenkai_types::{CellPos, ColIdx, NumberFormat, Range, Rgb, RowIdx, SheetId, StyleChange};

mod support;
use support::cell_xfs;

const SIDE: i64 = 7;
const SHEETS: [&str; 4] = ["Data", "Calc", "Notes", "Untouched"];
const UNTOUCHED_PART: &str = "xl/worksheets/sheet4.xml";
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

// Written by another application, so its styles, strings and relationships are not
// IronCalc's own and a copied part means what it meant there.
fn base_file(path: &Path) {
    let mut book = Source::new();
    let bold = Format::new().set_bold();
    let money = Format::new().set_num_format("#,##0.00");
    let red = Format::new().set_font_color("#C00000");
    let data = book.add_worksheet().set_name("Data").unwrap();
    for row in 0..6u32 {
        data.write_number(row, 0, f64::from(row) * 1.5).unwrap();
        data.write_string(row, 1, ["apple", "pear", "fig"][row as usize % 3])
            .unwrap();
    }
    data.write_number_with_format(0, 2, 42.0, &bold).unwrap();
    data.write_number_with_format(1, 2, 1234.5, &money).unwrap();
    let rule = ConditionalFormatCell::new()
        .set_rule(ConditionalFormatCellRule::GreaterThan(3))
        .set_format(&red);
    data.add_conditional_format(0, 0, 5, 0, &rule).unwrap();
    data.autofilter(0, 1, 5, 1).unwrap();
    let calc = book.add_worksheet().set_name("Calc").unwrap();
    // Cached results as Excel would store them, so a copied sheet's values are right.
    calc.write_formula(0, 0, Formula::new("=Data!A2*2").set_result("3"))
        .unwrap();
    calc.write_formula(1, 0, Formula::new("=SUM(Data!A1:A6)").set_result("22.5"))
        .unwrap();
    calc.write_formula(2, 0, Formula::new("=Data!B3&\"!\"").set_result("fig!"))
        .unwrap();
    calc.write_string_with_format(0, 1, "total", &bold).unwrap();
    let mut chart = Chart::new(ChartType::Column);
    chart.add_series().set_values("Data!$A$1:$A$6");
    calc.insert_chart(8, 4, &chart).unwrap();
    let notes = book.add_worksheet().set_name("Notes").unwrap();
    notes.write_string(0, 0, "pear").unwrap();
    notes.write_number_with_format(1, 0, 9.25, &money).unwrap();
    // Inside the edited range, so edits reach its header and body.
    notes.add_table(0, 3, 3, 4, &Table::new()).unwrap();
    let untouched = book.add_worksheet().set_name("Untouched").unwrap();
    untouched.write_string(0, 0, "never edited").unwrap();
    untouched.write_number(1, 1, 3.0).unwrap();
    book.save(path).unwrap();
}

#[derive(Clone, Debug)]
enum Op {
    Set(u32, i64, i64, String),
    Style(u32, i64, i64, StyleChange),
    InsertRow(u32, i64),
    DeleteRow(u32, i64),
    Undo,
    Save,
}

fn input() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (-500i32..500).prop_map(|n| n.to_string()),
        prop::sample::select(vec!["apple", "fig", "kiwi", "a & <b>", " padded "])
            .prop_map(str::to_string),
        "[a-z]{1,6}",
        (0..SIDE).prop_map(|r| format!("=Data!A{}+1", r + 1)),
        (0..SIDE).prop_map(|r| format!("=Calc!A{}*3", r + 1)),
        Just("=SUM(Data!A1:A6)".to_string()),
        Just("=Data!B2&Notes!A1".to_string()),
    ]
}

fn style_change() -> impl Strategy<Value = StyleChange> {
    prop_oneof![
        any::<bool>().prop_map(StyleChange::Bold),
        any::<bool>().prop_map(StyleChange::Italic),
        prop::sample::select(vec![
            NumberFormat::General,
            NumberFormat::Percent,
            NumberFormat::Date,
            NumberFormat::Currency,
        ])
        .prop_map(StyleChange::NumberFormat),
        prop::option::of(prop::sample::select(vec![0xFF0000, 0x00FF00, 0x123456]))
            .prop_map(|color| StyleChange::Fill(color.map(Rgb))),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let sheet = || 0u32..3;
    let at = || 0..SIDE;
    prop_oneof![
        8 => (sheet(), at(), at(), input()).prop_map(|(s, r, c, v)| Op::Set(s, r, c, v)),
        2 => (sheet(), at(), at(), style_change()).prop_map(|(s, r, c, change)| Op::Style(s, r, c, change)),
        2 => (sheet(), at()).prop_map(|(s, r)| Op::InsertRow(s, r)),
        2 => (sheet(), at()).prop_map(|(s, r)| Op::DeleteRow(s, r)),
        2 => Just(Op::Undo),
        2 => Just(Op::Save),
    ]
}

fn apply(book: &mut Workbook, op: &Op, document: &Path) {
    // An edit the engine refuses is not part of the state being checked.
    let _ = match op {
        Op::Set(s, r, c, v) => book.set_input(SheetId(*s), pos(*r, *c), v),
        Op::Style(s, r, c, change) => {
            let at = pos(*r, *c);
            book.apply_style(SheetId(*s), Range::new(at, at), *change)
        }
        Op::InsertRow(s, r) => book.insert_rows(SheetId(*s), RowIdx::clamped(*r), 1),
        Op::DeleteRow(s, r) => book.delete_rows(SheetId(*s), RowIdx::clamped(*r), 1),
        Op::Undo => book.undo(),
        Op::Save => {
            let snapshot = book.save_snapshot(document, SaveTarget::Document).unwrap();
            let kept = snapshot.kept().to_vec();
            save_xlsx(snapshot).unwrap();
            assert_eq!(
                kept,
                scan_unsupported(&std::fs::read(document).unwrap()).unwrap()
            );
            assert_relationships_resolve(document);
            Ok(())
        }
    };
}

fn cells(book: &Workbook) -> Vec<String> {
    let mut lines = Vec::new();
    for sheet in book.sheets() {
        lines.push(format!("sheet {sheet:?}"));
        for row in 0..SIDE + 2 {
            for col in 0..4 {
                let at = pos(row, col);
                lines.push(format!(
                    "{}!{row},{col}|{}|{:?}",
                    sheet.name,
                    book.input(sheet.id, at),
                    book.cell(sheet.id, at)
                ));
            }
        }
    }
    lines
}

fn first_difference(expected: &[String], actual: &[String]) -> String {
    expected
        .iter()
        .zip(actual)
        .find(|(a, b)| a != b)
        .map(|(a, b)| format!("expected {a}\n  actual {b}"))
        .unwrap_or_else(|| format!("{} lines against {}", expected.len(), actual.len()))
}

// The compressed bytes and checksum of a part, as stored in the archive.
fn raw_part(file: &Path, name: &str) -> Option<(u32, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    let index = (0..archive.len()).find(|&i| archive.by_index_raw(i).unwrap().name() == name)?;
    let mut entry = archive.by_index_raw(index).unwrap();
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    Some((entry.crc32(), bytes))
}

fn inflated(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut text = String::new();
    archive
        .by_name(name)
        .ok()?
        .read_to_string(&mut text)
        .unwrap();
    Some(text)
}

// The values the file stores for its cells, which Excel shows without recalculating:
// a formula whose inputs changed must have been written again.
fn stored_values(file: &Path) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    let texts = |node: roxmltree::Node| -> String {
        node.descendants()
            .filter(|n| n.has_tag_name("t"))
            .filter_map(|n| n.text())
            .collect()
    };
    let strings: Vec<String> = match inflated(&mut archive, "xl/sharedStrings.xml") {
        Some(xml) => roxmltree::Document::parse(&xml)
            .unwrap()
            .descendants()
            .filter(|n| n.has_tag_name("si"))
            .map(texts)
            .collect(),
        None => Vec::new(),
    };
    let mut lines = Vec::new();
    for sheet in 1..=SHEETS.len() {
        let xml = inflated(&mut archive, &format!("xl/worksheets/sheet{sheet}.xml")).unwrap();
        let document = roxmltree::Document::parse(&xml).unwrap();
        for cell in document.descendants().filter(|n| n.has_tag_name("c")) {
            let stored = cell
                .children()
                .find(|n| n.has_tag_name("v"))
                .and_then(|v| v.text());
            let value = match (cell.attribute("t"), stored) {
                (Some("s"), Some(index)) => strings[index.parse::<usize>().unwrap()].clone(),
                (Some("inlineStr"), _) => texts(cell),
                (_, Some(text)) => match text.parse::<f64>() {
                    Ok(number) => number.to_string(),
                    Err(_) => text.to_string(),
                },
                (_, None) => continue,
            };
            lines.push(format!("{sheet}!{}={value}", cell.attribute("r").unwrap()));
        }
    }
    lines.sort();
    lines
}

fn relationship_ids(xml: &str) -> BTreeSet<String> {
    roxmltree::Document::parse(xml)
        .unwrap()
        .descendants()
        .filter(|n| n.has_tag_name("Relationship"))
        .map(|n| n.attribute("Id").unwrap().to_string())
        .collect()
}

fn assert_relationships_resolve(file: &Path) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    for sheet in 1..=SHEETS.len() {
        let xml = inflated(&mut archive, &format!("xl/worksheets/sheet{sheet}.xml")).unwrap();
        let declared = inflated(
            &mut archive,
            &format!("xl/worksheets/_rels/sheet{sheet}.xml.rels"),
        )
        .map(|rels| relationship_ids(&rels))
        .unwrap_or_default();
        let document = roxmltree::Document::parse(&xml).unwrap();
        for attribute in document
            .descendants()
            .flat_map(|n| n.attributes())
            .filter(|a| a.namespace() == Some(RELATIONSHIPS))
        {
            assert!(
                declared.contains(attribute.value()),
                "sheet{sheet} names {} outside {declared:?}",
                attribute.value()
            );
        }
    }
}

fn part_names(file: &Path) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    (0..archive.len())
        .map(|i| archive.by_index_raw(i).unwrap().name().to_string())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn a_copying_save_reopens_as_a_whole_save(ops in prop::collection::vec(op(), 1..14)) {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("book.xlsx");
        base_file(&document);
        let original = std::fs::read(&document).unwrap();
        let mut book = open_xlsx(&document).unwrap().workbook;
        for op in &ops {
            apply(&mut book, op, &document);
        }
        let copy = dir.path().join("copy.xlsx");
        save_xlsx(book.save_snapshot(&copy, SaveTarget::Copy).unwrap()).unwrap();
        save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
        let whole_bytes = book.to_xlsx().unwrap();
        let whole_file = dir.path().join("whole.xlsx");
        std::fs::write(&whole_file, &whole_bytes).unwrap();
        let whole = Workbook::from_xlsx_bytes(&whole_bytes, "book").unwrap();
        let expected = cells(&whole);
        let expected_values = stored_values(&whole_file);
        for file in [&copy, &document] {
            // Recovery copies are restored with IronCalc's own reader.
            for reader in [XlsxReader::Fast, XlsxReader::IronCalc] {
                let reopened = cells(&open_xlsx_with(file, reader).unwrap().workbook);
                prop_assert!(reopened == expected, "{}", first_difference(&expected, &reopened));
            }
            let values = stored_values(file);
            prop_assert!(values == expected_values, "{}", first_difference(&expected_values, &values));
            assert_relationships_resolve(file);
        }
        prop_assert!(cells(&book) == expected, "{}", first_difference(&expected, &cells(&book)));
        let base = dir.path().join("base.xlsx");
        std::fs::write(&base, original).unwrap();
        // The saves above copy from the other application's file: a sheet no op touched
        // is as it was, and the styles it points at are the same, new ones at the end.
        prop_assert_eq!(raw_part(&document, UNTOUCHED_PART), raw_part(&base, UNTOUCHED_PART));
        let styles_of = |file: &Path| cell_xfs(&inflated(&mut zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap(), "xl/styles.xml").unwrap());
        let (old, new) = (styles_of(&base), styles_of(&document));
        prop_assert!(new.starts_with(&old), "an existing style changed");
    }
}

fn opened(dir: &Path) -> (std::path::PathBuf, Workbook) {
    let document = dir.join("book.xlsx");
    base_file(&document);
    let book = open_xlsx(&document).unwrap().workbook;
    (document, book)
}

// Rows inserted on one sheet drop the links of the others, which may name cells there; a
// sheet that has none is not rewritten for it.
#[test]
fn rows_moved_on_one_sheet_leave_a_sheet_without_links_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    let base = dir.path().join("base.xlsx");
    std::fs::copy(&document, &base).unwrap();
    book.insert_rows(SheetId(0), RowIdx::clamped(0), 1).unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    assert_eq!(
        raw_part(&document, UNTOUCHED_PART),
        raw_part(&base, UNTOUCHED_PART)
    );
    for name in ["xl/charts/chart1.xml", "xl/tables/table1.xml"] {
        assert_eq!(raw_part(&document, name), raw_part(&base, name), "{name}");
    }
}

#[test]
fn an_edit_rewrites_its_sheet_and_copies_every_other_part() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    let base = dir.path().join("base.xlsx");
    std::fs::copy(&document, &base).unwrap();
    book.set_input(SheetId(2), pos(0, 0), "kiwi").unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let names = part_names(&base);
    assert_eq!(part_names(&document), names);
    for name in names {
        let rewritten = ["xl/worksheets/sheet3.xml", "xl/sharedStrings.xml"];
        let same = raw_part(&document, &name) == raw_part(&base, &name);
        assert_eq!(same, !rewritten.contains(&name.as_str()), "{name}");
    }
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.cell(SheetId(2), pos(0, 0)).text, "kiwi");
    assert_eq!(reopened.cell(SheetId(1), pos(2, 0)).text, "fig!");
}

#[test]
fn a_row_inserted_on_a_read_sheet_rewrites_the_sheets_that_read_it() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    book.insert_rows(SheetId(0), RowIdx::clamped(0), 1).unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.input(SheetId(1), pos(0, 0)), "=Data!A3*2");
    assert_eq!(reopened.input(SheetId(1), pos(1, 0)), "=SUM(Data!A2:A7)");
}

#[test]
fn an_edit_after_the_snapshot_is_written_by_the_next_save() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    book.set_input(SheetId(2), pos(0, 0), "first").unwrap();
    let snapshot = book.save_snapshot(&document, SaveTarget::Document).unwrap();
    book.set_input(SheetId(3), pos(0, 0), "late").unwrap();
    save_xlsx(snapshot).unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.cell(SheetId(2), pos(0, 0)).text, "first");
    assert_eq!(reopened.cell(SheetId(3), pos(0, 0)).text, "late");
}

#[test]
fn an_undone_edit_copies_every_part() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    let base = dir.path().join("base.xlsx");
    std::fs::copy(&document, &base).unwrap();
    book.set_input(SheetId(0), pos(0, 0), "99").unwrap();
    book.undo().unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    for name in part_names(&base) {
        assert_eq!(raw_part(&document, &name), raw_part(&base, &name), "{name}");
    }
}

#[test]
fn a_file_changed_on_disk_fails_once_then_is_written_whole() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    let mut other = Source::new();
    other
        .add_worksheet()
        .write_string(0, 0, "replaced")
        .unwrap();
    other.save(&document).unwrap();
    book.set_input(SheetId(2), pos(0, 0), "mine").unwrap();
    let refused = save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap());
    assert!(
        matches!(refused, Err(EngineError::ChangedOnDisk(_))),
        "{refused:?}"
    );
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.sheets().len(), SHEETS.len());
    assert_eq!(reopened.cell(SheetId(2), pos(0, 0)).text, "mine");
}

#[test]
fn a_change_on_disk_after_the_snapshot_fails_the_save_and_the_next_one_works() {
    let dir = tempfile::tempdir().unwrap();
    let (document, mut book) = opened(dir.path());
    book.set_input(SheetId(2), pos(0, 0), "mine").unwrap();
    let snapshot = book.save_snapshot(&document, SaveTarget::Document).unwrap();
    let mut other = Source::new();
    other
        .add_worksheet()
        .write_string(0, 0, "replaced")
        .unwrap();
    other.save(&document).unwrap();
    assert!(save_xlsx(snapshot).is_err());
    let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(leftovers, 1, "the temp file is gone");
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.cell(SheetId(2), pos(0, 0)).text, "mine");
}

// Rewrites one part of a zip in place, keeping the others.
fn replace_part(file: &Path, name: &str, edit: impl Fn(String) -> String) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        let entry_name = entry.name().to_string();
        let body = if entry_name == name { edit(text) } else { text };
        writer
            .start_file(entry_name, zip::write::FileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, body.as_bytes()).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    drop(archive);
    std::fs::write(file, bytes).unwrap();
}

#[test]
fn styles_laid_out_with_white_space_keep_every_cell_style() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    base_file(&document);
    replace_part(&document, "xl/styles.xml", |styles| {
        styles.replace("><xf ", ">\n  <xf ")
    });
    let base = dir.path().join("base.xlsx");
    std::fs::copy(&document, &base).unwrap();
    let mut book = open_xlsx(&document).unwrap().workbook;
    let bold = book.cell(SheetId(0), pos(0, 2)).style;
    book.set_input(SheetId(2), pos(0, 0), "kiwi").unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    // IronCalc counts the white space between styles as styles of its own, so a sheet it
    // writes would point at other styles than the copied part holds: the file is written
    // whole.
    assert_ne!(
        raw_part(&document, UNTOUCHED_PART),
        raw_part(&base, UNTOUCHED_PART)
    );
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.cell(SheetId(0), pos(0, 2)).style, bold);
    assert_eq!(cells(&reopened), cells(&book));
}
