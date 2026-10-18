// Spec: undoing a row or column insert or delete brings back the sheet elements the move
// dropped, and redoing drops them again, so a save after open, edit and undo matches a save
// after open alone.
#![allow(
    clippy::unwrap_used,
    reason = "a failed unwrap in a test is a failed test"
)]

use std::io::Read;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use rust_xlsxwriter::{DataValidation, DataValidationRule, Workbook as Source};
use zenkai_engine::{Engine, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

const SHEET: SheetId = SheetId(0);
const KEPT: [&str; 2] = ["dataValidations", "hyperlinks"];

fn source(dir: &Path) -> PathBuf {
    let path = dir.join("source.xlsx");
    let mut book = Source::new();
    let sheet = book.add_worksheet();
    for row in 0..12 {
        for col in 0..4 {
            sheet
                .write_number(row, col, f64::from(row * 4 + u32::from(col)))
                .unwrap();
        }
    }
    sheet.write_url(8, 0, "https://example.com").unwrap();
    let rule = DataValidation::new().allow_whole_number(DataValidationRule::Between(1, 9));
    sheet.add_data_validation(7, 1, 8, 1, &rule).unwrap();
    let other = book.add_worksheet();
    other.write_number(0, 0, 1.0).unwrap();
    other.write_url(3, 0, "https://example.org").unwrap();
    book.save(&path).unwrap();
    path
}

fn saved_elements(book: &Workbook, path: &Path) -> Vec<String> {
    let snapshot = book.save_snapshot(path, SaveTarget::Document).unwrap();
    save_xlsx(snapshot).unwrap();
    let mut held = elements(path, "xl/worksheets/sheet1.xml");
    held.extend(elements(path, "xl/worksheets/sheet2.xml"));
    held
}

fn elements(path: &Path, part: &str) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name(part)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    let document = roxmltree::Document::parse(&text).unwrap();
    document
        .root_element()
        .children()
        .filter(|node| KEPT.contains(&node.tag_name().name()))
        .map(|node| text[node.range()].to_string())
        .collect()
}

fn row(index: i64) -> RowIdx {
    RowIdx::clamped(index)
}

fn column(index: i64) -> ColIdx {
    ColIdx::clamped(index)
}

fn baseline(dir: &Path) -> (PathBuf, Vec<String>) {
    let path = source(dir);
    let book = open_xlsx(&path).unwrap().workbook;
    let held = saved_elements(&book, &dir.join("baseline.xlsx"));
    assert_eq!(held.len(), 3, "{held:?}");
    (path, held)
}

#[test]
fn undoing_a_row_delete_above_the_elements_brings_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let (path, held) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.delete_rows(SHEET, row(1), 1).unwrap();
    book.undo().unwrap();
    assert_eq!(saved_elements(&book, &dir.path().join("undone.xlsx")), held);
    open_xlsx(&dir.path().join("undone.xlsx")).unwrap();
}

#[test]
fn undoing_a_column_insert_brings_the_elements_back() {
    let dir = tempfile::tempdir().unwrap();
    let (path, held) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.insert_columns(SHEET, column(0), 1).unwrap();
    book.undo().unwrap();
    assert_eq!(saved_elements(&book, &dir.path().join("undone.xlsx")), held);
}

// A save between would write the file without the hyperlink's relationship, which an undo
// cannot bring back, so only the last state is saved.
#[test]
fn redo_drops_the_elements_again_and_undo_restores_them_again() {
    let dir = tempfile::tempdir().unwrap();
    let (path, held) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.delete_rows(SHEET, row(1), 1).unwrap();
    book.undo().unwrap();
    book.redo().unwrap();
    book.undo().unwrap();
    assert_eq!(saved_elements(&book, &dir.path().join("undone.xlsx")), held);

    let mut redone = open_xlsx(&path).unwrap().workbook;
    redone.delete_rows(SHEET, row(1), 1).unwrap();
    redone.undo().unwrap();
    redone.redo().unwrap();
    assert!(saved_elements(&redone, &dir.path().join("redone.xlsx")).is_empty());
}

// The second sheet's hyperlink is dropped by a move on the first, which cannot tell what
// the relationships part points at.
#[test]
fn a_move_on_one_sheet_that_drops_the_elements_of_another_is_undone_and_redone() {
    let dir = tempfile::tempdir().unwrap();
    let (path, held) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.insert_rows(SheetId(1), row(100), 1).unwrap();
    book.undo().unwrap();
    assert_eq!(saved_elements(&book, &dir.path().join("undone.xlsx")), held);

    let mut redone = open_xlsx(&path).unwrap().workbook;
    redone.insert_rows(SheetId(1), row(100), 1).unwrap();
    redone.undo().unwrap();
    redone.redo().unwrap();
    let kept = saved_elements(&redone, &dir.path().join("redone.xlsx"));
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].contains("dataValidations"), "{kept:?}");
}

// Each move is one undo step, so undoing the later of two leaves the earlier one's drop.
#[test]
fn each_move_is_undone_by_its_own_step() {
    let dir = tempfile::tempdir().unwrap();
    let (path, held) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.delete_rows(SHEET, row(1), 1).unwrap();
    book.insert_rows(SHEET, row(0), 1).unwrap();
    book.undo().unwrap();
    book.undo().unwrap();
    assert_eq!(saved_elements(&book, &dir.path().join("undone.xlsx")), held);

    let mut once = open_xlsx(&path).unwrap().workbook;
    once.delete_rows(SHEET, row(1), 1).unwrap();
    once.insert_rows(SHEET, row(0), 1).unwrap();
    once.undo().unwrap();
    assert!(saved_elements(&once, &dir.path().join("once.xlsx")).is_empty());
}

#[test]
fn an_edit_after_an_undo_forgets_the_undone_move() {
    let dir = tempfile::tempdir().unwrap();
    let (path, held) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.delete_rows(SHEET, row(1), 1).unwrap();
    book.undo().unwrap();
    book.set_input(SHEET, CellPos::new(row(0), column(3)), "7")
        .unwrap();
    book.undo().unwrap();
    book.redo().unwrap();
    assert_eq!(saved_elements(&book, &dir.path().join("edited.xlsx")), held);
}

const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn part_text(path: &Path, name: &str) -> Option<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .ok()?
        .read_to_string(&mut text)
        .unwrap();
    Some(text)
}

// The relationship ids a sheet part uses that its relationships part does not declare.
fn dangling_ids(path: &Path, sheet: u32) -> Vec<String> {
    let sheet_text = part_text(path, &format!("xl/worksheets/sheet{sheet}.xml")).unwrap();
    let rels = part_text(path, &format!("xl/worksheets/_rels/sheet{sheet}.xml.rels"));
    let declared: Vec<String> = rels
        .iter()
        .flat_map(|text| {
            roxmltree::Document::parse(text)
                .unwrap()
                .descendants()
                .filter(|node| node.has_tag_name("Relationship"))
                .map(|node| node.attribute("Id").unwrap().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    roxmltree::Document::parse(&sheet_text)
        .unwrap()
        .descendants()
        .flat_map(|node| node.attributes())
        .filter(|attribute| attribute.namespace() == Some(RELATIONSHIPS))
        .map(|attribute| attribute.value().to_string())
        .filter(|id| !declared.contains(id))
        .collect()
}

// A save between a move and its undo writes the file without the hyperlink's
// relationship; the undo must not bring back a hyperlink that points at nothing.
#[test]
fn a_save_between_a_move_and_its_undo_never_leaves_a_dangling_relationship() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = baseline(dir.path());
    let mut book = open_xlsx(&path).unwrap().workbook;
    book.delete_rows(SHEET, row(1), 1).unwrap();
    saved_elements(&book, &dir.path().join("deleted.xlsx"));
    book.undo().unwrap();
    let undone = dir.path().join("undone.xlsx");
    saved_elements(&book, &undone);
    for sheet in [1, 2] {
        assert_eq!(dangling_ids(&undone, sheet), Vec::<String>::new());
    }
    open_xlsx(&undone).unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Move {
    InsertRows(i64, u32),
    DeleteRows(i64, u32),
    InsertColumns(i64, u16),
    DeleteColumns(i64, u16),
}

fn apply(book: &mut Workbook, step: Move) {
    match step {
        Move::InsertRows(at, n) => book.insert_rows(SHEET, row(at), n),
        Move::DeleteRows(at, n) => book.delete_rows(SHEET, row(at), n),
        Move::InsertColumns(at, n) => book.insert_columns(SHEET, column(at), n),
        Move::DeleteColumns(at, n) => book.delete_columns(SHEET, column(at), n),
    }
    .unwrap();
}

fn step() -> impl Strategy<Value = Move> {
    prop_oneof![
        (0..14i64, 1..3u32).prop_map(|(at, n)| Move::InsertRows(at, n)),
        (0..14i64, 1..3u32).prop_map(|(at, n)| Move::DeleteRows(at, n)),
        (0..5i64, 1..3u16).prop_map(|(at, n)| Move::InsertColumns(at, n)),
        (0..5i64, 1..3u16).prop_map(|(at, n)| Move::DeleteColumns(at, n)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn undoing_every_move_restores_the_elements_and_redoing_matches_the_edits(
        steps in prop::collection::vec(step(), 1..6)
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (path, held) = baseline(dir.path());
        let mut edited = open_xlsx(&path).unwrap().workbook;
        let mut book = open_xlsx(&path).unwrap().workbook;
        for step in &steps {
            apply(&mut edited, *step);
            apply(&mut book, *step);
        }
        let edited = saved_elements(&edited, &dir.path().join("edited.xlsx"));
        for _ in &steps {
            book.undo().unwrap();
        }
        prop_assert_eq!(&saved_elements(&book, &dir.path().join("undone.xlsx")), &held);
        for _ in &steps {
            book.redo().unwrap();
        }
        prop_assert_eq!(&saved_elements(&book, &dir.path().join("redone.xlsx")), &edited);
    }
}
