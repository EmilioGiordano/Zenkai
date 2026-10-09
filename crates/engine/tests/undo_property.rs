// Spec: any sequence of edits followed by as many undos leaves the workbook as it started,
// and as many redos brings back the edited state.
#![allow(clippy::unwrap_used)]

use proptest::prelude::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, StyleChange};

const SIDE: i64 = 5;
const SHEET: SheetId = SheetId(0);

#[derive(Clone, Debug)]
enum Op {
    Set(i64, i64, String),
    Block(i64, i64, Vec<Vec<String>>),
    Clear(i64, i64, i64, i64),
    Fill(i64, i64, i64, i64, bool),
    Bold(i64, i64, i64, i64, bool),
}

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn range(a: i64, b: i64, c: i64, d: i64) -> Range {
    Range::new(pos(a, b), pos(c, d))
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (-50i32..50).prop_map(|n| n.to_string()),
        "[a-z]{1,4}",
        (0..SIDE, 0..SIDE).prop_map(|(r, c)| format!("=A1+{}", cell_name(r, c))),
        Just("=SUM(A1:C3)".to_string()),
        Just("=1/0".to_string()),
    ]
}

fn cell_name(row: i64, col: i64) -> String {
    let letter = char::from(b'A' + u8::try_from(col).unwrap());
    format!("{letter}{}", row + 1)
}

fn op() -> impl Strategy<Value = Op> {
    let at = 0..SIDE;
    prop_oneof![
        (at.clone(), at.clone(), value()).prop_map(|(r, c, v)| Op::Set(r, c, v)),
        (
            at.clone(),
            at.clone(),
            prop::collection::vec(prop::collection::vec(value(), 1..3), 1..3)
        )
            .prop_map(|(r, c, rows)| Op::Block(r, c, rows)),
        (at.clone(), at.clone(), at.clone(), at.clone())
            .prop_map(|(a, b, c, d)| Op::Clear(a, b, c, d)),
        // Starting below row 1 and right of column A, so there is always a source line
        // (filling from nothing is a no-op that adds no undo step, as in Excel).
        (1..SIDE, 1..SIDE, 1..SIDE, 1..SIDE, any::<bool>())
            .prop_map(|(a, b, c, d, down)| Op::Fill(a, b, c, d, down)),
        (at.clone(), at.clone(), at.clone(), at, any::<bool>())
            .prop_map(|(a, b, c, d, on)| Op::Bold(a, b, c, d, on)),
    ]
}

fn apply(book: &mut Workbook, op: &Op) -> bool {
    let result = match op {
        Op::Set(r, c, v) => book.set_input(SHEET, pos(*r, *c), v),
        Op::Block(r, c, rows) => book.set_inputs(SHEET, pos(*r, *c), rows),
        // A clear over cells without contents changes nothing and records no undo step.
        Op::Clear(a, b, c, d) => {
            let target = range(*a, *b, *c, *d);
            if !book
                .filled_cells(SHEET)
                .iter()
                .any(|pos| target.contains(*pos))
            {
                return false;
            }
            book.clear(SHEET, target)
        }
        Op::Fill(a, b, c, d, down) => book.fill(SHEET, range(*a, *b, *c, *d), *down),
        Op::Bold(a, b, c, d, on) => {
            book.apply_style(SHEET, range(*a, *b, *c, *d), StyleChange::Bold(*on))
        }
    };
    result.is_ok()
}

fn snapshot(book: &Workbook) -> Vec<String> {
    let mut cells = Vec::new();
    for row in 0..SIDE + 2 {
        for col in 0..SIDE + 2 {
            let at = pos(row, col);
            cells.push(format!(
                "{}|{:?}",
                book.input(SHEET, at),
                book.cell(SHEET, at)
            ));
        }
    }
    cells
}

fn seeded() -> Workbook {
    let mut book = Workbook::new_empty().unwrap();
    let rows = vec![
        vec!["1".to_string(), "x".to_string(), "=A1*2".to_string()],
        vec!["3".to_string(), String::new(), "=SUM(A1:A2)".to_string()],
    ];
    book.set_inputs(SHEET, pos(0, 0), &rows).unwrap();
    book
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn undo_all_restores_start_and_redo_all_restores_end(ops in prop::collection::vec(op(), 1..12)) {
        let mut book = seeded();
        let start = snapshot(&book);
        let applied = ops.iter().filter(|op| apply(&mut book, op)).count();
        let end = snapshot(&book);
        for _ in 0..applied {
            book.undo().unwrap();
        }
        prop_assert_eq!(snapshot(&book), start);
        for _ in 0..applied {
            book.redo().unwrap();
        }
        prop_assert_eq!(snapshot(&book), end);
    }
}

fn every_cell(book: &Workbook) -> Vec<(CellPos, String)> {
    let mut cells: Vec<(CellPos, String)> = book
        .filled_cells(SHEET)
        .into_iter()
        .map(|at| {
            let view = format!("{}|{:?}", book.input(SHEET, at), book.cell(SHEET, at));
            (at, view)
        })
        .collect();
    cells.sort_by_key(|(at, _)| (at.row, at.col));
    cells
}

fn far_value() -> impl Strategy<Value = String> {
    prop_oneof![
        (-50i32..50).prop_map(|n| n.to_string()),
        "[a-z]{1,4}",
        Just("=A1+1".to_string()),
        Just("=SUM(A:A)".to_string()),
        Just("=SEQUENCE(2,2)".to_string()),
    ]
}

// Cells spread over the whole sheet, so the box around them is far past a million cells,
// next to a dense block.
fn scattered() -> impl Strategy<Value = Vec<(u32, u16, String)>> {
    prop::collection::vec((0u32..1_048_576, 0u16..40, far_value()), 1..25)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn one_undo_restores_every_cleared_cell(
        far in scattered(),
        block_rows in 1u32..60,
        (top, left, bottom, right) in (0u32..1_048_576, 0u16..40, 0u32..1_048_576, 0u16..40),
        whole_sheet in any::<bool>(),
    ) {
        let mut book = seeded();
        let block: Vec<Vec<String>> = (0..block_rows)
            .map(|row| (0..6).map(|col| format!("{}", row * 6 + col)).collect())
            .collect();
        book.set_inputs(SHEET, pos(10, 0), &block).unwrap();
        for (row, col, text) in &far {
            let at = CellPos::new(RowIdx::new(*row).unwrap(), ColIdx::new(*col).unwrap());
            book.set_input(SHEET, at, text).unwrap();
        }
        let target = if whole_sheet {
            Range::new(CellPos::default(), CellPos::new(RowIdx::LAST, ColIdx::LAST))
        } else {
            Range::new(
                CellPos::new(RowIdx::new(top).unwrap(), ColIdx::new(left).unwrap()),
                CellPos::new(RowIdx::new(bottom).unwrap(), ColIdx::new(right).unwrap()),
            )
        };
        let before = every_cell(&book);
        if !before.iter().any(|(at, _)| target.contains(*at)) {
            // Nothing to clear records no undo step, as in Excel.
            book.clear(SHEET, target).unwrap();
            prop_assert_eq!(every_cell(&book), before);
            return Ok(());
        }
        if book.clear(SHEET, target).is_err() {
            // Only an array formula reaching out of the selection refuses, changing nothing.
            prop_assert_eq!(every_cell(&book), before);
            return Ok(());
        }
        let cleared = every_cell(&book);
        prop_assert!(cleared.iter().all(|(at, _)| !target.contains(*at)));
        book.undo().unwrap();
        prop_assert_eq!(every_cell(&book), before.clone());
        book.redo().unwrap();
        prop_assert_eq!(every_cell(&book), cleared);
    }
}
