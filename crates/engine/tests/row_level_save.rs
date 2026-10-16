// Spec: a save after a few edits writes only the rows that changed and copies the others
// from the file on disk. The file must reopen exactly as one written whole from the same
// workbook, rows no edit touched stay byte for byte, and whatever the copy cannot vouch
// for is written whole instead.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use rust_xlsxwriter::{Format, Formula, Table, TableColumn, Workbook as Source};
use zenkai_engine::{
    Engine, EngineError, SaveTarget, Workbook, XlsxReader, open_xlsx, open_xlsx_with, save_xlsx,
};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, StyleChange};

const ROWS: u32 = 200;
const COLUMNS: i64 = 5;
const ROWS_PART: &str = "xl/worksheets/sheet1.xml";
const FRUITS: [&str; 3] = ["apple", "pear", "fig"];

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

// Written by another application, with every row tagged `spans`, which IronCalc never
// writes: a row that still has it was copied from this file.
fn excel_base(path: &Path) {
    let mut book = Source::new();
    let bold = Format::new().set_bold();
    let rows = book.add_worksheet().set_name("Rows").unwrap();
    for row in 0..ROWS {
        let line = row + 1;
        rows.write_number(row, 0, f64::from(line)).unwrap();
        rows.write_string(row, 1, FRUITS[row as usize % 3]).unwrap();
        rows.write_formula(
            row,
            2,
            Formula::new(format!("=A{line}*2")).set_result((line * 2).to_string()),
        )
        .unwrap();
        rows.write_formula(
            row,
            3,
            Formula::new(format!("=A{line}+Other!$A$1")).set_result((line + 10).to_string()),
        )
        .unwrap();
    }
    rows.write_number_with_format(20, 4, 1.5, &bold).unwrap();
    rows.set_row_height(30, 30).unwrap();
    rows.set_row_hidden(40).unwrap();
    rows.group_rows(99, 109).unwrap();
    let other = book.add_worksheet().set_name("Other").unwrap();
    other.write_number(0, 0, 10.0).unwrap();
    book.save(path).unwrap();
}

// Written whole by Zenkai and remembered as the workbook's file, so its formulas down a
// column are shared formulas and its own metadata part is there.
fn zenkai_base(path: &Path) -> Workbook {
    let source = path.with_extension("source.xlsx");
    excel_base(&source);
    let book = Workbook::from_xlsx_bytes(&std::fs::read(&source).unwrap(), "book").unwrap();
    save_xlsx(book.save_snapshot(path, SaveTarget::Document).unwrap()).unwrap();
    book
}

// As the app does: a save that cannot copy from the file is made again, whole.
fn save(book: &Workbook, path: &Path) {
    match save_xlsx(book.save_snapshot(path, SaveTarget::Document).unwrap()) {
        Err(EngineError::BaseUnusable { .. }) => {
            save_xlsx(book.save_snapshot(path, SaveTarget::Document).unwrap()).unwrap()
        }
        written => written.unwrap(),
    }
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

// Each <row> element of a sheet part, by number, as written.
fn row_elements(xml: &str) -> BTreeMap<u32, String> {
    let mut rows = BTreeMap::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<row ") {
        rest = &rest[start..];
        let tag_end = rest.find('>').unwrap();
        let end = if rest[..tag_end].ends_with('/') {
            tag_end + 1
        } else {
            rest.find("</row>").unwrap() + "</row>".len()
        };
        let element = &rest[..end];
        let number = element.split("r=\"").nth(1).unwrap();
        let number = number[..number.find('"').unwrap()].parse().unwrap();
        rows.insert(number, element.to_string());
        rest = &rest[end..];
    }
    rows
}

// The rows another application wrote that a save left exactly as they were.
fn copied_rows(base: &Path, saved: &Path) -> Vec<u32> {
    let before = row_elements(&part(base, ROWS_PART));
    let after = row_elements(&part(saved, ROWS_PART));
    before
        .iter()
        .filter(|(row, element)| element.contains("spans=") && after.get(row) == Some(element))
        .map(|(row, _)| *row)
        .collect()
}

fn block_of(row: u32) -> u32 {
    (row - 1) / 16
}

fn cells(book: &Workbook) -> Vec<String> {
    let mut lines = Vec::new();
    for sheet in book.sheets() {
        lines.push(format!("sheet {sheet:?} {:?}", book.sizes(sheet.id)));
        for row in 0..i64::from(ROWS) + 24 {
            for col in 0..COLUMNS + 1 {
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

fn reopened(file: &Path) -> Vec<Vec<String>> {
    [XlsxReader::Fast, XlsxReader::IronCalc]
        .map(|reader| cells(&open_xlsx_with(file, reader).unwrap().workbook))
        .into()
}

fn copy_of(path: &Path) -> PathBuf {
    let copy = path.with_extension("base.xlsx");
    std::fs::copy(path, &copy).unwrap();
    copy
}

#[test]
fn rows_outside_the_edited_blocks_are_copied_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    book.set_input(SheetId(0), pos(104, 1), "kiwi").unwrap();
    book.set_input(SheetId(0), pos(149, 2), "=A150*5").unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let edited = [block_of(50), block_of(105), block_of(150)];
    let copied = copied_rows(&base, &document);
    let expected: Vec<u32> = (1..=ROWS)
        .filter(|row| !edited.contains(&block_of(*row)))
        .collect();
    assert_eq!(copied, expected);
    let written = row_elements(&part(&document, ROWS_PART));
    assert!(written[&50].contains("<v>2000</v>"), "{}", written[&50]);
    assert!(
        written[&105].contains("outlineLevel=\"1\""),
        "{}",
        written[&105]
    );
    assert!(written[&150].contains("<f>A150*5</f>"), "{}", written[&150]);
    let whole = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "book").unwrap();
    let expected = cells(&whole);
    for cells in reopened(&document) {
        assert!(cells == expected, "{}", first_difference(&expected, &cells));
    }
}

#[test]
fn rows_inserted_near_the_top_rewrite_the_sheet_whole() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.insert_rows(SheetId(0), RowIdx::clamped(2), 1).unwrap();
    save(&book, &document);
    assert!(copied_rows(&base, &document).is_empty());
    assert_eq!(book.input(SheetId(0), pos(10, 2)), "=A11*2");
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.input(SheetId(0), pos(10, 2)), "=A11*2");
}

// A validation below the moved rows that the model does not hold; a copy of the part
// around the rows would keep it on its old row.
fn with_validation_below(document: &Path) {
    replace_part(document, ROWS_PART, |sheet| {
        let validation = "<dataValidations count=\"1\"><dataValidation type=\"whole\" \
                          sqref=\"A195\"><formula1>1</formula1></dataValidation>\
                          </dataValidations>";
        sheet.replacen("<pageMargins", &format!("{validation}<pageMargins"), 1)
    });
    assert!(part(document, ROWS_PART).contains("sqref=\"A195\""));
}

#[test]
fn a_row_deleted_near_the_end_rewrites_the_sheet_whole() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    with_validation_below(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.delete_rows(SheetId(0), RowIdx::clamped(189), 1)
        .unwrap();
    save(&book, &document);
    assert!(copied_rows(&base, &document).is_empty());
    assert!(!part(&document, ROWS_PART).contains("sqref=\"A195\""));
    let whole = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "book").unwrap();
    let expected = cells(&whole);
    for cells in reopened(&document) {
        assert!(cells == expected, "{}", first_difference(&expected, &cells));
    }
}

#[test]
fn a_move_redone_after_a_save_rewrites_the_sheet_whole() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    with_validation_below(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.delete_rows(SheetId(0), RowIdx::clamped(189), 1)
        .unwrap();
    book.undo().unwrap();
    save(&book, &document);
    assert!(part(&document, ROWS_PART).contains("sqref=\"A195\""));
    book.redo().unwrap();
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    save(&book, &document);
    assert!(copied_rows(&base, &document).is_empty());
    assert!(!part(&document, ROWS_PART).contains("sqref=\"A195\""));
}

#[test]
fn an_undone_edit_does_not_stop_the_next_save_from_copying_rows() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    book.undo().unwrap();
    book.set_input(SheetId(0), pos(104, 1), "kiwi").unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let copied = copied_rows(&base, &document);
    let expected: Vec<u32> = (1..=ROWS)
        .filter(|row| block_of(*row) != block_of(105))
        .collect();
    assert_eq!(copied, expected);
    let whole = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "book").unwrap();
    let expected = cells(&whole);
    for cells in reopened(&document) {
        assert!(cells == expected, "{}", first_difference(&expected, &cells));
    }
}

#[test]
fn rows_inserted_then_undone_before_a_save_still_save_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    with_validation_below(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.insert_rows(SheetId(0), RowIdx::clamped(2), 1).unwrap();
    book.undo().unwrap();
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    save(&book, &document);
    assert!(copied_rows(&base, &document).is_empty());
    let whole = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "book").unwrap();
    let expected = cells(&whole);
    for cells in reopened(&document) {
        assert!(cells == expected, "{}", first_difference(&expected, &cells));
    }
}

#[test]
fn a_change_around_the_rows_rewrites_the_sheet_whole() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    book.set_frozen(SheetId(0), 1, 0).unwrap();
    save(&book, &document);
    assert!(copied_rows(&base, &document).is_empty());
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.frozen(SheetId(0)), (1, 0));
    assert_eq!(reopened.cell(SheetId(0), pos(49, 2)).text, "2000");
}

#[test]
fn a_shared_formula_kept_rows_take_is_written_again_when_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    let mut book = zenkai_base(&document);
    assert!(part(&document, ROWS_PART).contains("t=\"shared\""));
    book.set_input(SheetId(0), pos(0, 0), "7").unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let written = part(&document, ROWS_PART);
    // Rows written row by row hold their formulas whole.
    assert!(written.contains("<f>A2*2</f>"), "{written}");
    for book in [XlsxReader::Fast, XlsxReader::IronCalc]
        .map(|reader| open_xlsx_with(&document, reader).unwrap().workbook)
    {
        assert_eq!(book.cell(SheetId(0), pos(0, 2)).text, "14");
        assert_eq!(book.input(SheetId(0), pos(0, 2)), "=A1*2");
        assert_eq!(book.input(SheetId(0), pos(99, 2)), "=A100*2");
        assert_eq!(book.cell(SheetId(0), pos(99, 2)).text, "200");
    }
}

#[test]
fn a_changed_shared_formula_kept_rows_take_fails_once_then_is_written_whole() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    let mut book = zenkai_base(&document);
    book.set_input(SheetId(0), pos(0, 2), "=A1*3").unwrap();
    let refused = save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap());
    assert!(
        matches!(refused, Err(EngineError::BaseUnusable { .. })),
        "{refused:?}"
    );
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    for book in [XlsxReader::Fast, XlsxReader::IronCalc]
        .map(|reader| open_xlsx_with(&document, reader).unwrap().workbook)
    {
        assert_eq!(book.input(SheetId(0), pos(0, 2)), "=A1*3");
        assert_eq!(book.input(SheetId(0), pos(99, 2)), "=A100*2");
        assert_eq!(book.cell(SheetId(0), pos(99, 2)).text, "200");
    }
}

// Rewrites one part of a zip, keeping the others.
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
fn rows_the_copy_cannot_read_fail_once_then_are_written_whole() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    replace_part(&document, ROWS_PART, |sheet| {
        sheet.replacen("<row r=\"100\"", "<!-- kept apart --><row r=\"100\"", 1)
    });
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    let refused = save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap());
    assert!(
        matches!(refused, Err(EngineError::BaseUnusable { .. })),
        "{refused:?}"
    );
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.cell(SheetId(0), pos(49, 2)).text, "2000");
    assert_eq!(reopened.cell(SheetId(0), pos(150, 2)).text, "302");
}

fn add_part(file: &Path, name: &str, body: &str) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        writer
            .raw_copy_file(archive.by_index_raw(index).unwrap())
            .unwrap();
    }
    writer
        .start_file(name, zip::write::FileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut writer, body.as_bytes()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    drop(archive);
    std::fs::write(file, bytes).unwrap();
}

const EXTRA_PART: &str = "customXml/item1.xml";

fn has_part(file: &Path, name: &str) -> bool {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
    archive.by_name(name).is_ok()
}

const TABLE_PART: &str = "xl/tables/table1.xml";

// A table over A1:B100 on a sheet that goes on for 200 rows, written by another application.
fn table_base(path: &Path) {
    let mut book = Source::new();
    let sheet = book.add_worksheet().set_name("Rows").unwrap();
    sheet.write_string(0, 0, "Name").unwrap();
    sheet.write_string(0, 1, "Qty").unwrap();
    for row in 1..ROWS {
        sheet
            .write_string(row, 0, FRUITS[row as usize % 3])
            .unwrap();
        sheet.write_number(row, 1, f64::from(row)).unwrap();
    }
    let columns = [
        TableColumn::new().set_header("Name"),
        TableColumn::new().set_header("Qty"),
    ];
    let table = Table::new().set_columns(&columns);
    sheet.add_table(0, 0, 99, 1, &table).unwrap();
    book.save(path).unwrap();
}

// Excel repairs a table whose header cells differ from its part, so a header edit must not
// take the row-level patch, which keeps the table relationship; an edit below the table does.
#[test]
fn a_table_header_edit_is_written_whole_and_a_cell_below_the_table_row_by_row() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    table_base(&document);
    let base = copy_of(&document);
    let mut book = open_xlsx(&document).unwrap().workbook;

    book.set_input(SheetId(0), pos(149, 1), "1000").unwrap();
    save(&book, &document);
    let edited = block_of(150);
    let expected: Vec<u32> = (1..=ROWS).filter(|row| block_of(*row) != edited).collect();
    assert_eq!(copied_rows(&base, &document), expected);
    assert!(has_part(&document, TABLE_PART));

    let second_base = copy_of(&document);
    book.set_input(SheetId(0), pos(0, 0), "Renamed").unwrap();
    save(&book, &document);
    assert!(copied_rows(&second_base, &document).is_empty());
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.input(SheetId(0), pos(0, 0)), "Renamed");
    assert_eq!(reopened.input(SheetId(0), pos(149, 1)), "1000");
    if has_part(&document, TABLE_PART) {
        let table = part(&document, TABLE_PART);
        assert!(table.contains("name=\"Renamed\""), "{table}");
        assert!(!table.contains("name=\"Name\""), "{table}");
    }
}

// Only a save that copies from the file keeps a part Zenkai does not write itself.
#[test]
fn a_sheet_the_copy_refuses_is_the_only_one_written_whole_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("book.xlsx");
    excel_base(&document);
    replace_part(&document, ROWS_PART, |sheet| {
        sheet.replacen("<row r=\"100\"", "<!-- kept apart --><row r=\"100\"", 1)
    });
    add_part(&document, EXTRA_PART, "<x/>");
    let mut book = open_xlsx(&document).unwrap().workbook;
    book.set_input(SheetId(0), pos(49, 0), "1000").unwrap();
    let refused = save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap());
    assert!(
        matches!(refused, Err(EngineError::BaseUnusable { .. })),
        "{refused:?}"
    );
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    assert!(has_part(&document, EXTRA_PART));
    book.set_input(SheetId(0), pos(60, 0), "5").unwrap();
    save_xlsx(book.save_snapshot(&document, SaveTarget::Document).unwrap()).unwrap();
    assert!(has_part(&document, EXTRA_PART));
    let reopened = open_xlsx(&document).unwrap().workbook;
    assert_eq!(reopened.cell(SheetId(0), pos(49, 2)).text, "2000");
    assert_eq!(reopened.cell(SheetId(0), pos(60, 2)).text, "10");
}

#[derive(Clone, Debug)]
enum Op {
    Set(i64, i64, String),
    Clear(i64),
    Bold(i64, i64, bool),
    Height(i64, f32),
    Hide(i64, bool),
    InsertRow(i64),
    DeleteRow(i64),
    Undo,
    Save,
}

fn row() -> impl Strategy<Value = i64> {
    prop_oneof![
        3 => 0..i64::from(ROWS) + 20,
        1 => 0i64..20,
        1 => i64::from(ROWS) - 20..i64::from(ROWS),
    ]
}

fn input() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (-500i32..500).prop_map(|n| n.to_string()),
        prop::sample::select(vec!["apple", "kiwi", "a & <b>", " padded "]).prop_map(str::to_string),
        (1..=ROWS).prop_map(|r| format!("=A{r}*2")),
        (1..=ROWS).prop_map(|r| format!("=A{r}*3")),
        (1..=ROWS).prop_map(|r| format!("=SUM(A1:A{r})")),
        Just("=Other!A1+1".to_string()),
        (1..=ROWS).prop_map(|r| format!("=A{r}:A{}*2", r + 20)),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let col = || 0..COLUMNS;
    prop_oneof![
        8 => (row(), col(), input()).prop_map(|(r, c, v)| Op::Set(r, c, v)),
        1 => row().prop_map(Op::Clear),
        1 => (row(), col(), any::<bool>()).prop_map(|(r, c, b)| Op::Bold(r, c, b)),
        1 => (row(), 4.0f32..80.0).prop_map(|(r, h)| Op::Height(r, h)),
        1 => (row(), any::<bool>()).prop_map(|(r, h)| Op::Hide(r, h)),
        1 => row().prop_map(Op::InsertRow),
        1 => row().prop_map(Op::DeleteRow),
        1 => Just(Op::Undo),
        2 => Just(Op::Save),
    ]
}

fn apply(book: &mut Workbook, op: &Op, document: &Path) {
    let rows = |r: i64| Range::new(pos(r, 0), pos(r, COLUMNS - 1));
    let sheet = SheetId(0);
    // An edit the engine refuses is not part of the state being checked.
    let _ = match op {
        Op::Set(r, c, v) => book.set_input(sheet, pos(*r, *c), v),
        Op::Clear(r) => book.clear(sheet, rows(*r)),
        Op::Bold(r, c, bold) => {
            let at = pos(*r, *c);
            book.apply_style(sheet, Range::new(at, at), StyleChange::Bold(*bold))
        }
        Op::Height(r, pixels) => book.set_row_height(sheet, RowIdx::clamped(*r), *pixels),
        Op::Hide(r, hidden) => book.set_rows_hidden(sheet, rows(*r), *hidden),
        Op::InsertRow(r) => book.insert_rows(sheet, RowIdx::clamped(*r), 1),
        Op::DeleteRow(r) => book.delete_rows(sheet, RowIdx::clamped(*r), 1),
        Op::Undo => book.undo(),
        Op::Save => {
            save(book, document);
            Ok(())
        }
    };
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn a_row_level_save_reopens_as_a_whole_save(
        from_zenkai in any::<bool>(),
        ops in prop::collection::vec(op(), 1..12),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let document = dir.path().join("book.xlsx");
        let mut book = if from_zenkai {
            zenkai_base(&document)
        } else {
            excel_base(&document);
            open_xlsx(&document).unwrap().workbook
        };
        for op in &ops {
            apply(&mut book, op, &document);
        }
        save(&book, &document);
        let whole = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "book").unwrap();
        let expected = cells(&whole);
        for cells in reopened(&document) {
            prop_assert!(cells == expected, "{}", first_difference(&expected, &cells));
        }
    }
}
