use zenkai_types::{CellPos, ColIdx, Range, RowIdx};

#[derive(Debug, PartialEq, Eq)]
pub struct Total {
    pub target: CellPos,
    pub sum: Range,
}

// Where Excel's AutoSum writes for a multi-cell selection: under each column, or after
// the row when the selection is a single row. An empty last row (or last column of a
// single row) receives the totals itself; otherwise they go just outside the selection.
pub fn totals(selection: Range, filled: impl Fn(CellPos) -> bool) -> Vec<Total> {
    if selection.start == selection.end {
        return Vec::new();
    }
    if selection.start.row == selection.end.row {
        row_totals(selection, filled)
    } else {
        column_totals(selection, filled)
    }
}

fn column_totals(selection: Range, filled: impl Fn(CellPos) -> bool) -> Vec<Total> {
    let (start, end) = (selection.start, selection.end);
    let last_row_empty = !(start.col.get()..=end.col.get())
        .filter_map(ColIdx::new)
        .any(|col| filled(CellPos::new(end.row, col)));
    let (sum_end, target_row) = if last_row_empty {
        (end.row.offset(-1), Some(end.row))
    } else {
        (end.row, RowIdx::new(end.row.get() + 1))
    };
    let Some(target_row) = target_row else {
        return Vec::new();
    };
    (start.col.get()..=end.col.get())
        .filter_map(ColIdx::new)
        .map(|col| Total {
            target: CellPos::new(target_row, col),
            sum: Range::new(CellPos::new(start.row, col), CellPos::new(sum_end, col)),
        })
        .collect()
}

fn row_totals(selection: Range, filled: impl Fn(CellPos) -> bool) -> Vec<Total> {
    let (start, end) = (selection.start, selection.end);
    let (sum_end, target_col) = if filled(end) {
        (end.col, ColIdx::new(end.col.get() + 1))
    } else {
        (end.col.offset(-1), Some(end.col))
    };
    let Some(target_col) = target_col else {
        return Vec::new();
    };
    vec![Total {
        target: CellPos::new(start.row, target_col),
        sum: Range::new(start, CellPos::new(start.row, sum_end)),
    }]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use zenkai_types::{MAX_COLS, MAX_ROWS};

    fn pos(row: u32, col: u16) -> CellPos {
        CellPos::new(RowIdx::new(row).unwrap(), ColIdx::new(col).unwrap())
    }

    fn range(a: (u32, u16), b: (u32, u16)) -> Range {
        Range::new(pos(a.0, a.1), pos(b.0, b.1))
    }

    fn fill(selection: Range, skip_row: Option<u32>, skip_col: Option<u16>) -> HashSet<CellPos> {
        let mut cells = HashSet::new();
        for row in selection.start.row.get()..=selection.end.row.get() {
            for col in selection.start.col.get()..=selection.end.col.get() {
                if Some(row) != skip_row && Some(col) != skip_col {
                    cells.insert(pos(row, col));
                }
            }
        }
        cells
    }

    #[test]
    fn single_cell_has_no_totals() {
        let selection = range((1, 1), (1, 1));
        assert!(totals(selection, |_| true).is_empty());
    }

    #[test]
    fn full_columns_get_a_total_in_the_row_below() {
        let selection = range((1, 1), (3, 2));
        let cells = fill(selection, None, None);
        let result = totals(selection, |p| cells.contains(&p));
        assert_eq!(
            result,
            vec![
                Total {
                    target: pos(4, 1),
                    sum: range((1, 1), (3, 1))
                },
                Total {
                    target: pos(4, 2),
                    sum: range((1, 2), (3, 2))
                },
            ]
        );
    }

    #[test]
    fn empty_last_row_receives_the_totals() {
        let selection = range((1, 1), (4, 2));
        let cells = fill(selection, Some(4), None);
        let result = totals(selection, |p| cells.contains(&p));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].target, pos(4, 1));
        assert_eq!(result[0].sum, range((1, 1), (3, 1)));
        assert_eq!(result[1].target, pos(4, 2));
    }

    #[test]
    fn single_row_gets_a_total_after_the_row() {
        let selection = range((2, 1), (2, 3));
        let cells = fill(selection, None, None);
        let result = totals(selection, |p| cells.contains(&p));
        assert_eq!(
            result,
            vec![Total {
                target: pos(2, 4),
                sum: range((2, 1), (2, 3))
            }]
        );
    }

    #[test]
    fn single_row_with_empty_last_cell_receives_the_total() {
        let selection = range((2, 1), (2, 3));
        let cells = fill(selection, None, Some(3));
        let result = totals(selection, |p| cells.contains(&p));
        assert_eq!(
            result,
            vec![Total {
                target: pos(2, 3),
                sum: range((2, 1), (2, 2))
            }]
        );
    }

    #[test]
    fn selection_touching_the_bottom_edge_needs_an_empty_last_row() {
        let last = MAX_ROWS - 1;
        let selection = range((last - 2, 0), (last, 1));
        assert!(totals(selection, |_| true).is_empty());
        let result = totals(selection, |p| p.row.get() != last);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].target, pos(last, 0));
    }

    #[test]
    fn selection_touching_the_right_edge_has_no_room_for_a_row_total() {
        let last = MAX_COLS - 1;
        let selection = range((0, last - 1), (0, last));
        assert!(totals(selection, |_| true).is_empty());
    }
}
