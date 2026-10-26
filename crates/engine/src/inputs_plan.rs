// What a block of rows of mixed lengths writes, worked out before anything is written so a
// block the engine would refuse is refused whole. Every step follows the cells that exist
// or the fields in the block, never the area the widest row spans.
use ironcalc::base::cell_store::CellStore;
use ironcalc::base::types::{ArrayKind, Cell};
use zenkai_types::{CellPos, Range};

use crate::error::EngineError;
use crate::model::{for_each_within, position};
use crate::workbook::{col_i32, row_i32};

pub(crate) struct Run<'a> {
    pub origin: CellPos,
    pub rows: &'a [Vec<String>],
}

pub(crate) struct InputsPlan<'a> {
    pub runs: Vec<Run<'a>>,
    // Existing cells right of a short row, inside the block's width, which the block empties.
    pub tail: Vec<CellPos>,
}

pub(crate) fn plan_inputs<'a>(
    sheet_data: &CellStore,
    origin: CellPos,
    rows: &'a [Vec<String>],
    width: usize,
) -> Result<InputsPlan<'a>, EngineError> {
    let mut plan = InputsPlan {
        runs: Vec::new(),
        tail: Vec::new(),
    };
    let mut offset = 0i64;
    for run in rows.chunk_by(|a, b| a.len() == b.len()) {
        let len = run[0].len();
        let first = origin.row.offset(offset);
        let last = origin.row.offset(offset + run.len() as i64 - 1);
        offset += run.len() as i64;
        if len > 0 {
            let covered = Range::new(
                CellPos::new(first, origin.col),
                CellPos::new(last, origin.col.offset(len as i64 - 1)),
            );
            let mut refused = false;
            for_each_within(sheet_data, covered, |row, col, cell| {
                refused |= splits_array(sheet_data, cell, row, col, Some(covered));
            });
            if refused {
                return Err(splits_an_array());
            }
            plan.runs.push(Run {
                origin: CellPos::new(first, origin.col),
                rows: run,
            });
        }
        if len < width {
            let uncovered = Range::new(
                CellPos::new(first, origin.col.offset(len as i64)),
                CellPos::new(last, origin.col.offset(width as i64 - 1)),
            );
            let mut refused = false;
            for_each_within(sheet_data, uncovered, |row, col, cell| {
                refused |= splits_array(sheet_data, cell, row, col, None);
                if !matches!(cell, Cell::EmptyCell { .. }) {
                    plan.tail.push(position(row, col));
                }
            });
            if refused {
                return Err(splits_an_array());
            }
        }
    }
    Ok(plan)
}

fn splits_an_array() -> EngineError {
    EngineError::Rejected("the block would change part of an array formula".to_string())
}

// Mirrors the engine's own refusal to clear part of a Ctrl+Shift+Enter array: it must lie
// wholly inside `inside`. With no area, any cell of such an array is a split.
fn splits_array(
    sheet_data: &CellStore,
    cell: &Cell,
    row: i32,
    col: i32,
    inside: Option<Range>,
) -> bool {
    let outside = |anchor_row: i32, anchor_col: i32, (width, height): (i32, i32)| {
        inside.is_none_or(|area| {
            anchor_col < col_i32(area.start.col)
                || anchor_row < row_i32(area.start.row)
                || anchor_col + width > col_i32(area.end.col) + 1
                || anchor_row + height > row_i32(area.end.row) + 1
        })
    };
    match cell {
        Cell::ArrayFormula(array) if array.kind == ArrayKind::Cse => outside(row, col, array.r),
        Cell::SpillCell(spill) => match sheet_data.get(spill.a.0, spill.a.1) {
            Some(Cell::ArrayFormula(array)) if array.kind == ArrayKind::Cse => {
                outside(spill.a.0, spill.a.1, array.r)
            }
            Some(Cell::ArrayFormula(_)) => false,
            _ => true,
        },
        _ => false,
    }
}
