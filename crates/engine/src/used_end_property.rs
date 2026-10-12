// The cached used end of every sheet equals a fresh walk of the sheet after any edit,
// including spills that grow or shrink on another sheet.
#![allow(clippy::unwrap_used)]

use proptest::prelude::*;
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, StyleChange};

use crate::workbook::{Engine, Workbook};

const ROWS: i64 = 8;
const COLS: i64 = 6;

#[derive(Clone, Debug)]
enum Op {
    Set(u32, i64, i64, String),
    Block(u32, i64, i64, Vec<Vec<String>>),
    Clear(u32, i64, i64, i64, i64),
    ClearAll(u32, i64, i64, i64, i64),
    Bold(u32, i64, i64, i64, i64),
    InsertRow(u32, i64),
    DeleteRow(u32, i64),
    Undo,
    Redo,
}

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (-5i32..5).prop_map(|n| n.to_string()),
        Just("=A1+1".to_string()),
        (1i32..4).prop_map(|n| format!("=SEQUENCE({n})")),
        Just("=IF(Sheet2!A1>0,SEQUENCE(3),1)".to_string()),
        Just("=SEQUENCE(MAX(1,Sheet1!A1))".to_string()),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let sheet = 0u32..2;
    let row = 0..ROWS;
    let col = 0..COLS;
    let area = (row.clone(), col.clone(), row.clone(), col.clone());
    prop_oneof![
        4 => (sheet.clone(), row.clone(), col.clone(), value())
            .prop_map(|(s, r, c, v)| Op::Set(s, r, c, v)),
        1 => (
            sheet.clone(),
            row.clone(),
            col.clone(),
            prop::collection::vec(prop::collection::vec(value(), 1..3), 1..3)
        )
            .prop_map(|(s, r, c, rows)| Op::Block(s, r, c, rows)),
        1 => (sheet.clone(), area.clone()).prop_map(|(s, (a, b, c, d))| Op::Clear(s, a, b, c, d)),
        1 => (sheet.clone(), area.clone()).prop_map(|(s, (a, b, c, d))| Op::ClearAll(s, a, b, c, d)),
        1 => (sheet.clone(), area).prop_map(|(s, (a, b, c, d))| Op::Bold(s, a, b, c, d)),
        1 => (sheet.clone(), row.clone()).prop_map(|(s, r)| Op::InsertRow(s, r)),
        1 => (sheet, row).prop_map(|(s, r)| Op::DeleteRow(s, r)),
        1 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

fn apply(book: &mut Workbook, op: &Op) {
    let span = |a, b, c, d| Range::new(pos(a, b), pos(c, d));
    // Refused edits are fine here: only the used end afterwards is under test.
    let _refused = match op {
        Op::Set(s, r, c, v) => book.set_input(SheetId(*s), pos(*r, *c), v),
        Op::Block(s, r, c, rows) => book.set_inputs(SheetId(*s), pos(*r, *c), rows),
        Op::Clear(s, a, b, c, d) => book.clear(SheetId(*s), span(*a, *b, *c, *d)),
        Op::ClearAll(s, a, b, c, d) => book.clear_all(SheetId(*s), span(*a, *b, *c, *d)),
        Op::Bold(s, a, b, c, d) => {
            book.apply_style(SheetId(*s), span(*a, *b, *c, *d), StyleChange::Bold(true))
        }
        Op::InsertRow(s, r) => book.insert_rows(SheetId(*s), RowIdx::clamped(*r), 1),
        Op::DeleteRow(s, r) => book.delete_rows(SheetId(*s), RowIdx::clamped(*r), 1),
        Op::Undo => book.undo(),
        Op::Redo => book.redo(),
    };
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn cached_end_matches_a_fresh_walk(ops in prop::collection::vec(op(), 1..16)) {
        let mut book = Workbook::new_empty().unwrap();
        book.add_sheet().unwrap();
        // Undo must not take the second sheet away again.
        book = Workbook::from_xlsx_bytes(&book.to_xlsx().unwrap(), "two sheets").unwrap();
        for op in &ops {
            apply(&mut book, op);
            book.warm_used_areas();
            for sheet in book.sheets() {
                prop_assert_eq!(
                    Some(book.used_end(sheet.id)),
                    book.walked_end(sheet.id),
                    "after {:?} on sheet {}", op, sheet.id.0
                );
            }
        }
    }
}
