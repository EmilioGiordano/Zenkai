use serde::{Deserialize, Serialize};
use zenkai_types::{CellPos, ColIdx, LineAxis, LineChange, LineOp, RowIdx};

pub const MAX_LINE_OPS: usize = 16;

// The rows and columns inserted or deleted since a CSV was read, in order, up to a bound: past
// it the cells count as moved instead. A fixed array keeps the binding `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<LineOp>", into = "Vec<LineOp>")]
pub struct LineOps {
    ops: [LineOp; MAX_LINE_OPS],
    len: usize,
}

impl Default for LineOps {
    fn default() -> Self {
        LineOps {
            ops: [LineOp::default(); MAX_LINE_OPS],
            len: 0,
        }
    }
}

impl From<Vec<LineOp>> for LineOps {
    fn from(list: Vec<LineOp>) -> Self {
        let mut ops = LineOps::default();
        for op in list {
            if !ops.push(op) {
                break;
            }
        }
        ops
    }
}

impl From<LineOps> for Vec<LineOp> {
    fn from(ops: LineOps) -> Self {
        ops.as_slice().to_vec()
    }
}

impl LineOps {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[LineOp] {
        &self.ops[..self.len]
    }

    // Whether there was room.
    pub fn push(&mut self, op: LineOp) -> bool {
        if self.len == MAX_LINE_OPS {
            return false;
        }
        self.ops[self.len] = op;
        self.len += 1;
        true
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

// Where the cells of a sheet were in the file before those operations.
pub struct LineMap {
    ops: Vec<LineOp>,
}

impl LineMap {
    pub fn new(ops: &[LineOp], sheet: u32) -> LineMap {
        LineMap {
            ops: ops.iter().filter(|op| op.sheet == sheet).copied().collect(),
        }
    }

    pub fn is_identity(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn moves_columns(&self) -> bool {
        self.ops.iter().any(|op| op.axis == LineAxis::Column)
    }

    // `None` for a line that was inserted, which the file has no counterpart of.
    fn origin(&self, axis: LineAxis, mut index: u32) -> Option<u32> {
        for op in self.ops.iter().rev().filter(|op| op.axis == axis) {
            match op.change {
                LineChange::Insert if index >= op.at.saturating_add(op.count) => {
                    index -= op.count;
                }
                LineChange::Insert if index >= op.at => return None,
                LineChange::Insert => {}
                LineChange::Delete if index >= op.at => {
                    index = index.saturating_add(op.count);
                }
                LineChange::Delete => {}
            }
        }
        Some(index)
    }

    pub fn file_row(&self, row: RowIdx) -> Option<RowIdx> {
        RowIdx::new(self.origin(LineAxis::Row, row.get())?)
    }

    pub fn file_position(&self, pos: CellPos) -> Option<CellPos> {
        let col = ColIdx::new(
            u16::try_from(self.origin(LineAxis::Column, u32::from(pos.col.get()))?).ok()?,
        )?;
        Some(CellPos::new(self.file_row(pos.row)?, col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(axis: LineAxis, change: LineChange, at: u32, count: u32) -> LineOp {
        LineOp {
            sheet: 0,
            axis,
            change,
            at,
            count,
        }
    }

    fn row(map: &LineMap, row: u32) -> Option<u32> {
        map.file_row(RowIdx::new(row).unwrap()).map(RowIdx::get)
    }

    #[test]
    fn rows_below_an_insertion_come_from_above_and_the_new_ones_from_nowhere() {
        let map = LineMap::new(&[op(LineAxis::Row, LineChange::Insert, 1, 2)], 0);
        let found: Vec<Option<u32>> = (0..5).map(|r| row(&map, r)).collect();
        assert_eq!(found, [Some(0), None, None, Some(1), Some(2)]);
    }

    #[test]
    fn rows_below_a_deletion_come_from_below() {
        let map = LineMap::new(&[op(LineAxis::Row, LineChange::Delete, 1, 2)], 0);
        let found: Vec<Option<u32>> = (0..3).map(|r| row(&map, r)).collect();
        assert_eq!(found, [Some(0), Some(3), Some(4)]);
    }

    #[test]
    fn operations_compose_in_the_order_they_were_made() {
        let map = LineMap::new(
            &[
                op(LineAxis::Row, LineChange::Insert, 0, 1),
                op(LineAxis::Row, LineChange::Delete, 2, 1),
            ],
            0,
        );
        let found: Vec<Option<u32>> = (0..4).map(|r| row(&map, r)).collect();
        assert_eq!(found, [None, Some(0), Some(2), Some(3)]);
    }

    #[test]
    fn rows_and_columns_are_mapped_apart_and_other_sheets_are_ignored() {
        let other = LineOp {
            sheet: 1,
            ..op(LineAxis::Row, LineChange::Insert, 0, 5)
        };
        let map = LineMap::new(&[op(LineAxis::Column, LineChange::Insert, 0, 1), other], 0);
        assert!(map.moves_columns());
        let at =
            |row: u32, col: u16| CellPos::new(RowIdx::new(row).unwrap(), ColIdx::new(col).unwrap());
        assert_eq!(map.file_position(at(3, 0)), None);
        assert_eq!(map.file_position(at(3, 2)), Some(at(3, 1)));
        assert!(LineMap::new(&[other], 0).is_identity());
    }

    #[test]
    fn the_list_has_a_bound_and_survives_the_session_file() {
        let mut ops = LineOps::default();
        assert!(ops.is_empty());
        let one = op(LineAxis::Column, LineChange::Delete, 4, 2);
        for _ in 0..MAX_LINE_OPS {
            assert!(ops.push(one));
        }
        assert!(!ops.push(one));
        let json = serde_json::to_string(&ops).unwrap();
        assert_eq!(serde_json::from_str::<LineOps>(&json).unwrap(), ops);
        ops.clear();
        assert!(ops.is_empty());
        assert_eq!(
            serde_json::from_str::<LineOps>("[]").unwrap(),
            LineOps::default()
        );
    }
}
