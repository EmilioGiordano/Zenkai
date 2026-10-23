// Spec: a workbook that went to disk and came back is the workbook it was. Two copies of one
// file get the same random edits; one of them hibernates and wakes at random points. They
// must agree on every cell, style, sheet and size, step by step through the whole undo and
// redo history, and must save the same file with the same warnings.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use rust_xlsxwriter::Workbook as Source;
use zenkai_engine::{Engine, EngineError, SaveTarget, Workbook, open_xlsx, save_xlsx};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, StyleChange};

const SIDE: i64 = 5;

#[derive(Clone, Debug)]
enum Op {
    Set(usize, i64, i64, String),
    Block(usize, i64, i64, Vec<Vec<String>>),
    Clear(usize, i64, i64, i64, i64),
    Fill(usize, i64, i64, i64, i64, bool),
    FillWith(usize, i64, i64, i64, i64, String),
    Extend(usize, i64, i64, i64, i64),
    Sort(usize, i64, i64, i64, i64, bool),
    Bold(usize, i64, i64, i64, i64, bool),
    NumberFormat(usize, i64, i64, i64, i64),
    InsertRows(usize, i64, u32),
    DeleteRows(usize, i64, u32),
    InsertColumns(usize, i64, u16),
    DeleteColumns(usize, i64, u16),
    ColumnWidth(usize, i64, u8),
    RowHeight(usize, i64, u8),
    Frozen(usize, u32, u16),
    AddSheet,
    DuplicateSheet(usize),
    RenameSheet(usize, u8),
    MoveSheet(usize, u32),
    DeleteSheet(usize),
    Undo,
    Redo,
    Hibernate,
}

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn range(a: i64, b: i64, c: i64, d: i64) -> Range {
    Range::new(pos(a, b), pos(c, d))
}

fn cell_name(row: i64, col: i64) -> String {
    let letter = char::from(b'A' + u8::try_from(col).unwrap());
    format!("{letter}{}", row + 1)
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (-50i32..50).prop_map(|n| n.to_string()),
        // Inputs that also set a number format on the cell.
        prop::sample::select(vec!["5%", "2024-01-15", "$5"]).prop_map(str::to_string),
        "[a-z]{1,4}",
        (0..SIDE, 0..SIDE).prop_map(|(r, c)| format!("=A1+{}", cell_name(r, c))),
        Just("=SUM(A1:C3)".to_string()),
        Just("=1/0".to_string()),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let at = 0..SIDE;
    let sheet = 0usize..3;
    prop_oneof![
        4 => (sheet.clone(), at.clone(), at.clone(), value()).prop_map(|(s, r, c, v)| Op::Set(s, r, c, v)),
        2 => (
            sheet.clone(),
            at.clone(),
            at.clone(),
            prop::collection::vec(prop::collection::vec(value(), 1..3), 1..3)
        )
            .prop_map(|(s, r, c, rows)| Op::Block(s, r, c, rows)),
        1 => (sheet.clone(), at.clone(), at.clone(), at.clone(), at.clone())
            .prop_map(|(s, a, b, c, d)| Op::Clear(s, a, b, c, d)),
        1 => (sheet.clone(), 1..SIDE, 1..SIDE, 1..SIDE, 1..SIDE, any::<bool>())
            .prop_map(|(s, a, b, c, d, down)| Op::Fill(s, a, b, c, d, down)),
        1 => (sheet.clone(), at.clone(), at.clone(), at.clone(), at.clone(), value())
            .prop_map(|(s, a, b, c, d, v)| Op::FillWith(s, a, b, c, d, v)),
        1 => (sheet.clone(), at.clone(), at.clone(), at.clone(), at.clone())
            .prop_map(|(s, a, b, c, d)| Op::Extend(s, a, b, c, d)),
        1 => (sheet.clone(), at.clone(), at.clone(), at.clone(), at.clone(), any::<bool>())
            .prop_map(|(s, a, b, c, d, down)| Op::Sort(s, a, b, c, d, down)),
        2 => (sheet.clone(), at.clone(), at.clone(), at.clone(), at.clone(), any::<bool>())
            .prop_map(|(s, a, b, c, d, on)| Op::Bold(s, a, b, c, d, on)),
        1 => (sheet.clone(), at.clone(), at.clone(), at.clone(), at.clone())
            .prop_map(|(s, a, b, c, d)| Op::NumberFormat(s, a, b, c, d)),
        2 => (sheet.clone(), 0i64..12, 1u32..3).prop_map(|(s, r, n)| Op::InsertRows(s, r, n)),
        2 => (sheet.clone(), 0i64..12, 1u32..3).prop_map(|(s, r, n)| Op::DeleteRows(s, r, n)),
        1 => (sheet.clone(), 0i64..SIDE, 1u16..3).prop_map(|(s, c, n)| Op::InsertColumns(s, c, n)),
        1 => (sheet.clone(), 0i64..SIDE, 1u16..3).prop_map(|(s, c, n)| Op::DeleteColumns(s, c, n)),
        1 => (sheet.clone(), at.clone(), 20u8..200).prop_map(|(s, c, w)| Op::ColumnWidth(s, c, w)),
        1 => (sheet.clone(), at, 10u8..100).prop_map(|(s, r, h)| Op::RowHeight(s, r, h)),
        1 => (sheet.clone(), 0u32..4, 0u16..3).prop_map(|(s, r, c)| Op::Frozen(s, r, c)),
        1 => Just(Op::AddSheet),
        1 => sheet.clone().prop_map(Op::DuplicateSheet),
        1 => (sheet.clone(), any::<u8>()).prop_map(|(s, n)| Op::RenameSheet(s, n)),
        1 => (sheet.clone(), 0u32..4).prop_map(|(s, to)| Op::MoveSheet(s, to)),
        1 => sheet.prop_map(Op::DeleteSheet),
        3 => Just(Op::Undo),
        2 => Just(Op::Redo),
        3 => Just(Op::Hibernate),
    ]
}

fn sheet_at(book: &Workbook, index: usize) -> SheetId {
    let sheets = book.sheets();
    sheets[index % sheets.len()].id
}

// Applies one edit; the result is only compared, never unwrapped, as an edit may be refused.
fn apply(book: &mut Workbook, op: &Op) -> Result<(), EngineError> {
    match op {
        Op::Set(s, r, c, v) => book.set_input(sheet_at(book, *s), pos(*r, *c), v),
        Op::Block(s, r, c, rows) => book.set_inputs(sheet_at(book, *s), pos(*r, *c), rows),
        Op::Clear(s, a, b, c, d) => book.clear(sheet_at(book, *s), range(*a, *b, *c, *d)),
        Op::Fill(s, a, b, c, d, down) => {
            book.fill(sheet_at(book, *s), range(*a, *b, *c, *d), *down)
        }
        Op::FillWith(s, a, b, c, d, text) => {
            let target = range(*a, *b, *c, *d);
            book.fill_with(sheet_at(book, *s), target.start, text, target)
        }
        Op::Extend(s, a, b, c, d) => {
            let target = range(*a, *b, *c, *d);
            let source = Range::new(target.start, target.start);
            book.extend(sheet_at(book, *s), source, target)
        }
        Op::Sort(s, a, b, c, d, descending) => {
            let target = range(*a, *b, *c, *d);
            book.sort(sheet_at(book, *s), target, target.start.col, *descending)
        }
        Op::Bold(s, a, b, c, d, on) => book.apply_style(
            sheet_at(book, *s),
            range(*a, *b, *c, *d),
            StyleChange::Bold(*on),
        ),
        Op::NumberFormat(s, a, b, c, d) => {
            book.set_number_format(sheet_at(book, *s), range(*a, *b, *c, *d), "0.00")
        }
        Op::InsertRows(s, at, n) => book.insert_rows(sheet_at(book, *s), RowIdx::clamped(*at), *n),
        Op::DeleteRows(s, at, n) => book.delete_rows(sheet_at(book, *s), RowIdx::clamped(*at), *n),
        Op::InsertColumns(s, at, n) => {
            book.insert_columns(sheet_at(book, *s), ColIdx::clamped(*at), *n)
        }
        Op::DeleteColumns(s, at, n) => {
            book.delete_columns(sheet_at(book, *s), ColIdx::clamped(*at), *n)
        }
        Op::ColumnWidth(s, c, w) => {
            book.set_column_width(sheet_at(book, *s), ColIdx::clamped(*c), f32::from(*w))
        }
        Op::RowHeight(s, r, h) => {
            book.set_row_height(sheet_at(book, *s), RowIdx::clamped(*r), f32::from(*h))
        }
        Op::Frozen(s, rows, cols) => book.set_frozen(sheet_at(book, *s), *rows, *cols),
        Op::AddSheet => book.add_sheet().map(|_| ()),
        Op::DuplicateSheet(s) => book.duplicate_sheet(sheet_at(book, *s)),
        Op::RenameSheet(s, n) => book.rename_sheet(sheet_at(book, *s), &format!("Sheet {n}")),
        Op::MoveSheet(s, to) => book.move_sheet(sheet_at(book, *s), *to),
        Op::DeleteSheet(s) => book.delete_sheet(sheet_at(book, *s)),
        Op::Undo => book.undo(),
        Op::Redo => book.redo(),
        Op::Hibernate => Ok(()),
    }
}

fn describe(result: &Result<(), EngineError>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(error) => error.to_string(),
    }
}

// Everything a user can see or edit, as text.
fn snapshot(book: &Workbook) -> String {
    let mut out = String::new();
    for info in book.sheets() {
        let id = info.id;
        writeln!(out, "{info:?}").unwrap();
        writeln!(
            out,
            "{:?} {:?} {:?} {:?}",
            book.sizes(id),
            book.frozen(id),
            book.merged(id),
            book.used_end(id)
        )
        .unwrap();
        for row in 0..SIDE + 8 {
            for col in 0..SIDE + 3 {
                let at = pos(row, col);
                writeln!(
                    out,
                    "{row},{col}|{}|{:?}",
                    book.input(id, at),
                    book.cell(id, at)
                )
                .unwrap();
            }
        }
        let mut filled = book.filled_cells(id);
        filled.sort_by_key(|at| (at.row, at.col));
        writeln!(out, "{filled:?}").unwrap();
    }
    out
}

// A file with settings IronCalc alone drops, so what a hibernated workbook keeps outside
// the model matters to the save.
fn source_file(path: &Path) {
    let mut book = Source::new();
    let first = book.add_worksheet();
    for row in 0..SIDE as u32 {
        for col in 0..SIDE as u16 {
            first
                .write_number(row, col, f64::from(row * 10 + u32::from(col)))
                .unwrap();
        }
    }
    first.write_formula(0, 6, "=SUM(A1:C3)").unwrap();
    first.set_tab_color("#00B050");
    first.set_zoom(85);
    let second = book.add_worksheet().set_name("Report").unwrap();
    for row in 0..12u32 {
        second.write_number(row, 0, f64::from(row)).unwrap();
    }
    second.group_rows(1, 4).unwrap();
    second.group_columns(1, 2).unwrap();
    second.set_landscape();
    second.set_print_gridlines(true);
    second.set_header("&LQ3 &RPage &P");
    second.set_page_breaks(&[6]).unwrap();
    second
        .merge_range(8, 2, 9, 3, "merged", &rust_xlsxwriter::Format::new())
        .unwrap();
    book.save(path).unwrap();
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            (entry.name().to_string(), content)
        })
        .collect()
}

fn hibernate_and_wake(book: &mut Workbook, dump: &Path) {
    let kept = book.hibernate(dump).unwrap();
    let woken = Workbook::wake(dump, &kept).unwrap();
    std::fs::remove_file(dump).unwrap();
    *book = woken;
}

fn save_both(
    steady: &Workbook,
    rested: &Workbook,
    dir: &Path,
    target: SaveTarget,
) -> (PathBuf, PathBuf) {
    let (a, b) = (dir.join("steady-out.xlsx"), dir.join("rested-out.xlsx"));
    let (a, b) = if target == SaveTarget::Document {
        (dir.join("steady.xlsx"), dir.join("rested.xlsx"))
    } else {
        (a, b)
    };
    let first = steady.save_snapshot(&a, target).unwrap();
    let second = rested.save_snapshot(&b, target).unwrap();
    assert_eq!(first.dropped(), second.dropped());
    assert_eq!(first.kept(), second.kept());
    save_xlsx(first).unwrap();
    save_xlsx(second).unwrap();
    (a, b)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn a_hibernated_workbook_is_the_workbook_it_was(ops in prop::collection::vec(op(), 1..24)) {
        let dir = tempfile::tempdir().unwrap();
        let (steady_file, rested_file) = (dir.path().join("steady.xlsx"), dir.path().join("rested.xlsx"));
        source_file(&steady_file);
        std::fs::copy(&steady_file, &rested_file).unwrap();
        let dump = dir.path().join("rested.zkh");
        let mut steady = open_xlsx(&steady_file).unwrap().workbook;
        let mut rested = open_xlsx(&rested_file).unwrap().workbook;
        prop_assert_eq!(snapshot(&steady), snapshot(&rested));

        for op in &ops {
            if matches!(op, Op::Hibernate) {
                hibernate_and_wake(&mut rested, &dump);
            }
            let first = apply(&mut steady, op);
            let second = apply(&mut rested, op);
            prop_assert_eq!(describe(&first), describe(&second));
            prop_assert_eq!(snapshot(&steady), snapshot(&rested), "after {:?}", op);
        }

        // The whole history, down to the bottom and back up, one step at a time. An undo or
        // redo with nothing left is a no-op, so a bound past the longest history is enough.
        hibernate_and_wake(&mut rested, &dump);
        let steps = ops.len() * 4 + 4;
        for step in 0..steps {
            if step == steps / 2 {
                // With both an undo and a redo side to carry.
                hibernate_and_wake(&mut rested, &dump);
            }
            prop_assert_eq!(describe(&steady.undo()), describe(&rested.undo()));
            prop_assert_eq!(snapshot(&steady), snapshot(&rested), "undo {}", step);
        }
        for step in 0..steps {
            prop_assert_eq!(describe(&steady.redo()), describe(&rested.redo()));
            prop_assert_eq!(snapshot(&steady), snapshot(&rested), "redo {}", step);
        }

        // The same file in both places, whether the save copies from the file or writes it whole.
        let (a, b) = save_both(&steady, &rested, dir.path(), SaveTarget::Copy);
        prop_assert_eq!(parts(&a), parts(&b));
        let (a, b) = save_both(&steady, &rested, dir.path(), SaveTarget::Document);
        prop_assert_eq!(parts(&a), parts(&b));
    }
}

#[test]
fn a_dump_that_is_damaged_never_wakes_a_different_workbook() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("book.xlsx");
    source_file(&file);
    let mut book = open_xlsx(&file).unwrap().workbook;
    book.set_input(SheetId(0), pos(0, 0), "edited").unwrap();
    let dump = dir.path().join("book.zkh");
    let kept = book.hibernate(&dump).unwrap();
    let good = std::fs::read(&dump).unwrap();
    for index in (0..good.len()).step_by(good.len() / 200 + 1) {
        let mut damaged = good.clone();
        damaged[index] ^= 0x10;
        std::fs::write(&dump, &damaged).unwrap();
        assert!(Workbook::wake(&dump, &kept).is_err(), "byte {index}");
    }
    std::fs::write(&dump, &good[..good.len() / 2]).unwrap();
    assert!(Workbook::wake(&dump, &kept).is_err());
    std::fs::write(&dump, &good).unwrap();
    let woken = Workbook::wake(&dump, &kept).unwrap();
    assert_eq!(snapshot(&woken), snapshot(&book));
}

#[test]
fn a_failed_write_leaves_the_workbook_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Workbook::new_empty().unwrap();
    book.set_input(SheetId(0), pos(0, 0), "1").unwrap();
    let before = snapshot(&book);
    let missing_folder = dir.path().join("none").join("book.zkh");
    assert!(book.hibernate(&missing_folder).is_err());
    assert_eq!(snapshot(&book), before);
    book.undo().unwrap();
}

#[test]
fn a_fill_with_is_undone_the_same_after_a_rest() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("book.zkh");
    let mut steady = Workbook::new_empty().unwrap();
    let target = range(0, 0, 2, 0);
    steady
        .fill_with(SheetId(0), target.start, "=B1*2", target)
        .unwrap();
    let mut rested = Workbook::new_empty().unwrap();
    rested
        .fill_with(SheetId(0), target.start, "=B1*2", target)
        .unwrap();
    hibernate_and_wake(&mut rested, &dump);
    steady.undo().unwrap();
    rested.undo().unwrap();
    assert_eq!(snapshot(&steady), snapshot(&rested));
    assert_eq!(rested.input(SheetId(0), pos(0, 0)), "");
}
