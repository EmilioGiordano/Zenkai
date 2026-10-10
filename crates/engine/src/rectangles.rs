use std::collections::HashMap;

use zenkai_types::{CellPos, ColIdx, Range};

// Covers exactly `cells`, which must be distinct, with rectangles: runs of adjacent cells in a row, stacked while
// the rows below have the same run. A dense block becomes one rectangle.
pub fn covering_rectangles(mut cells: Vec<CellPos>) -> Vec<Range> {
    cells.sort_unstable_by_key(|pos| (pos.row, pos.col));
    let mut closed = Vec::new();
    // Rectangles that end on the last row seen, keyed by their column span.
    let mut open: HashMap<(ColIdx, ColIdx), Range> = HashMap::new();
    for row_cells in cells.chunk_by(|a, b| a.row == b.row) {
        let row = row_cells[0].row;
        let mut next = HashMap::new();
        for run in row_cells.chunk_by(|a, b| u32::from(a.col.get()) + 1 == u32::from(b.col.get())) {
            let span = (run[0].col, run[run.len() - 1].col);
            let start = match open.remove(&span) {
                Some(above) if above.end.row.get() + 1 == row.get() => above.start,
                Some(above) => {
                    closed.push(above);
                    CellPos::new(row, span.0)
                }
                None => CellPos::new(row, span.0),
            };
            next.insert(span, Range::new(start, CellPos::new(row, span.1)));
        }
        closed.extend(open.into_values());
        open = next;
    }
    closed.extend(open.into_values());
    closed.sort_unstable_by_key(|range| (range.start.row, range.start.col));
    closed
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;
    use zenkai_types::RowIdx;

    fn pos(row: i64, col: i64) -> CellPos {
        CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col))
    }

    #[test]
    fn dense_block_is_one_rectangle() {
        let cells = (0..200)
            .flat_map(|r| (0..6).map(move |c| pos(r, c)))
            .collect();
        assert_eq!(
            covering_rectangles(cells),
            vec![Range::new(pos(0, 0), pos(199, 5))]
        );
    }

    #[test]
    fn gaps_split_rectangles() {
        let cells = vec![
            pos(0, 0),
            pos(0, 1),
            pos(1, 0),
            pos(1, 1),
            pos(3, 0),
            pos(3, 1),
        ];
        assert_eq!(
            covering_rectangles(cells),
            vec![
                Range::new(pos(0, 0), pos(1, 1)),
                Range::new(pos(3, 0), pos(3, 1)),
            ]
        );
    }

    proptest! {
        #[test]
        fn rectangles_cover_exactly_the_cells(
            cells in prop::collection::vec((0i64..12, 0i64..8), 0..60)
        ) {
            let wanted: HashSet<CellPos> = cells.into_iter().map(|(r, c)| pos(r, c)).collect();
            let mut covered = Vec::new();
            for range in covering_rectangles(wanted.iter().copied().collect()) {
                covered.extend(range.positions());
            }
            let unique: HashSet<CellPos> = covered.iter().copied().collect();
            prop_assert_eq!(unique.len(), covered.len(), "rectangles overlap");
            prop_assert_eq!(unique, wanted);
        }
    }
}
