use ironcalc::base::cell_store::CellStore;
use ironcalc::base::types::Cell;
use proptest::prelude::*;
use zenkai_types::{CellPos, ColIdx, MAX_COLS, MAX_ROWS, Range, RowIdx};

use crate::model::content_cells_within;
use crate::workbook::{col_i32, row_i32};

const GRID: i64 = 12;

#[derive(Clone, Debug)]
enum Stored {
    Value(i32, i32),
    StyleOnly(i32, i32),
    EmptyRow(i32),
}

fn stored_row() -> impl Strategy<Value = i32> {
    prop_oneof![8 => 1..=GRID as i32, 1 => Just(MAX_ROWS as i32)]
}

fn stored_col() -> impl Strategy<Value = i32> {
    prop_oneof![8 => 1..=GRID as i32, 1 => Just(i32::from(MAX_COLS))]
}

fn stored() -> impl Strategy<Value = Stored> {
    prop_oneof![
        4 => (stored_row(), stored_col()).prop_map(|(row, col)| Stored::Value(row, col)),
        2 => (stored_row(), stored_col()).prop_map(|(row, col)| Stored::StyleOnly(row, col)),
        1 => stored_row().prop_map(Stored::EmptyRow),
    ]
}

fn pos(row: i64, col: i64) -> CellPos {
    CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
}

fn range() -> impl Strategy<Value = Range> {
    let near = 0..GRID + 4;
    let past = GRID..GRID * 3;
    let last_row = i64::from(MAX_ROWS - 1);
    let last_col = i64::from(MAX_COLS - 1);
    prop_oneof![
        4 => (near.clone(), near.clone(), near.clone(), near.clone())
            .prop_map(|(r1, c1, r2, c2)| Range::new(pos(r1, c1), pos(r2, c2))),
        1 => (past.clone(), past.clone(), past.clone(), past)
            .prop_map(|(r1, c1, r2, c2)| Range::new(pos(r1, c1), pos(r2, c2))),
        1 => (near.clone(), near.clone())
            .prop_map(move |(c1, c2)| Range::new(pos(0, c1), pos(last_row, c2))),
        1 => (near.clone(), near)
            .prop_map(move |(r1, r2)| Range::new(pos(r1, 0), pos(r2, last_col))),
        1 => Just(Range::new(pos(0, 0), pos(last_row, last_col))),
    ]
}

fn sheet(entries: &[Stored]) -> CellStore {
    let mut sheet_data = CellStore::new();
    for entry in entries {
        match *entry {
            Stored::Value(row, col) => {
                sheet_data.insert(row, col, Cell::NumberCell { v: 1.0, s: 0 });
            }
            Stored::StyleOnly(row, col) => {
                sheet_data.insert(row, col, Cell::EmptyCell { s: 1 });
            }
            Stored::EmptyRow(row) => {
                if !sheet_data.has_row(row) {
                    sheet_data.insert_row(row, []);
                }
            }
        }
    }
    sheet_data
}

fn full_scan(sheet_data: &CellStore, range: Range) -> Vec<CellPos> {
    let rows = row_i32(range.start.row)..=row_i32(range.end.row);
    let cols = col_i32(range.start.col)..=col_i32(range.end.col);
    sheet_data
        .iter()
        .filter(|(row, col, cell)| {
            rows.contains(row) && cols.contains(col) && !matches!(cell, Cell::EmptyCell { .. })
        })
        .map(|(row, col, _)| {
            CellPos::new(
                RowIdx::clamped(i64::from(row) - 1),
                ColIdx::clamped(i64::from(col) - 1),
            )
        })
        .collect()
}

fn sorted(mut cells: Vec<CellPos>) -> Vec<CellPos> {
    cells.sort_unstable_by_key(|pos| (pos.row, pos.col));
    cells
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn walking_the_range_finds_what_a_full_scan_finds(
        entries in prop::collection::vec(stored(), 0..40),
        ranges in prop::collection::vec(range(), 1..6),
    ) {
        let sheet_data = sheet(&entries);
        for range in ranges {
            prop_assert_eq!(
                sorted(content_cells_within(&sheet_data, range)),
                sorted(full_scan(&sheet_data, range)),
                "range {:?}", range
            );
        }
    }
}
