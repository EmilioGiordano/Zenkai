use zenkai_grid::Layout;
use zenkai_types::{CellPos, ColIdx, Range, RowIdx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyScope {
    Plain,
    VisibleOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub source: Range,
    pub row_offset: u32,
    pub col_offset: u16,
}

// The rows and columns a copy takes, as runs of consecutive lines; they paste next to each
// other with no gap where a hidden line was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyShape {
    rows: Vec<(RowIdx, RowIdx)>,
    cols: Vec<(ColIdx, ColIdx)>,
}

impl CopyShape {
    // The one place that says which hidden lines a copy leaves out. Excel's plain copy takes
    // manually hidden lines and skips rows a filter hides, which `leaving_out_rows` removes.
    pub fn of(range: Range, scope: CopyScope, layout: &Layout) -> CopyShape {
        match scope {
            CopyScope::Plain => CopyShape::whole(range),
            CopyScope::VisibleOnly => CopyShape {
                rows: layout.shown_row_runs(range.start.row, range.end.row),
                cols: layout.shown_col_runs(range.start.col, range.end.col),
            },
        }
    }

    pub fn whole(range: Range) -> CopyShape {
        CopyShape {
            rows: vec![(range.start.row, range.end.row)],
            cols: vec![(range.start.col, range.end.col)],
        }
    }

    // What of the shape lies inside the used area; the lines past it hold nothing to copy.
    pub fn within(&self, end: CellPos) -> CopyShape {
        CopyShape {
            rows: self
                .rows
                .iter()
                .filter(|(first, _)| *first <= end.row)
                .map(|(first, last)| (*first, (*last).min(end.row)))
                .collect(),
            cols: self
                .cols
                .iter()
                .filter(|(first, _)| *first <= end.col)
                .map(|(first, last)| (*first, (*last).min(end.col)))
                .collect(),
        }
    }

    // Rows the filter hides stay out of a copy and a cut, as in Excel.
    pub fn leaving_out_rows(&self, hidden: &[(RowIdx, RowIdx)]) -> CopyShape {
        let mut rows = self.rows.clone();
        for (from, to) in hidden {
            rows = rows
                .into_iter()
                .flat_map(|(first, last)| {
                    if *to < first || *from > last {
                        return vec![(first, last)];
                    }
                    let mut kept = Vec::new();
                    if *from > first {
                        kept.push((first, from.offset(-1)));
                    }
                    if *to < last {
                        kept.push((to.offset(1), last));
                    }
                    kept
                })
                .collect();
        }
        CopyShape {
            rows,
            cols: self.cols.clone(),
        }
    }

    pub fn first_cell(&self) -> Option<CellPos> {
        let (row, _) = self.rows.first()?;
        let (col, _) = self.cols.first()?;
        Some(CellPos::new(*row, *col))
    }

    // The number of rectangles `blocks` would build, known without building them.
    pub fn block_count(&self) -> u64 {
        self.rows.len() as u64 * self.cols.len() as u64
    }

    pub fn cell_count(&self) -> u64 {
        u64::from(self.row_count()) * u64::from(self.col_count())
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() || self.cols.is_empty()
    }

    pub fn row_count(&self) -> u32 {
        self.rows
            .iter()
            .map(|(first, last)| last.get() - first.get() + 1)
            .sum()
    }

    pub fn col_count(&self) -> u16 {
        self.cols
            .iter()
            .map(|(first, last)| last.get() - first.get() + 1)
            .sum()
    }

    pub fn rows(&self) -> impl Iterator<Item = RowIdx> + '_ {
        self.rows.iter().flat_map(|(first, last)| {
            (first.get()..=last.get()).map(|row| RowIdx::clamped(row.into()))
        })
    }

    pub fn cols(&self) -> impl Iterator<Item = ColIdx> + '_ {
        self.cols.iter().flat_map(|(first, last)| {
            (first.get()..=last.get()).map(|col| ColIdx::clamped(col.into()))
        })
    }

    pub fn blocks(&self) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut row_offset = 0;
        for (first_row, last_row) in &self.rows {
            let mut col_offset = 0;
            for (first_col, last_col) in &self.cols {
                blocks.push(Block {
                    source: Range::new(
                        CellPos::new(*first_row, *first_col),
                        CellPos::new(*last_row, *last_col),
                    ),
                    row_offset,
                    col_offset,
                });
                col_offset += last_col.get() - first_col.get() + 1;
            }
            row_offset += last_row.get() - first_row.get() + 1;
        }
        blocks
    }
}

// Select Visible Cells Only marks the current selection; the workspace clears the mark
// whenever the selection changes, even to the same cells.
#[derive(Debug, Default)]
pub struct VisibleMark(bool);

impl VisibleMark {
    pub fn set(&mut self) {
        self.0 = true;
    }

    pub fn clear(&mut self) {
        self.0 = false;
    }

    pub fn copy_scope(&self) -> CopyScope {
        if self.0 {
            CopyScope::VisibleOnly
        } else {
            CopyScope::Plain
        }
    }
}

#[cfg(test)]
pub fn layout_hiding(rows: &[u32], cols: &[u16]) -> Layout {
    use zenkai_types::{ColumnSpan, SheetSizes};
    Layout::from_sizes(&SheetSizes {
        columns: cols
            .iter()
            .map(|col| ColumnSpan {
                first: ColIdx::clamped(i64::from(*col)),
                last: ColIdx::clamped(i64::from(*col)),
                width: 0.0,
            })
            .collect(),
        rows: rows
            .iter()
            .map(|row| (RowIdx::clamped(i64::from(*row)), 0.0))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    fn row_numbers(shape: &CopyShape) -> Vec<u32> {
        shape.rows().map(|row| row.get() + 1).collect()
    }

    fn col_numbers(shape: &CopyShape) -> Vec<u16> {
        shape.cols().map(|col| col.get() + 1).collect()
    }

    #[test]
    fn hidden_rows_in_the_middle_leave_seven_of_ten() {
        let layout = layout_hiding(&[2, 3, 4], &[]);
        let shape = CopyShape::of(range("A1:A10"), CopyScope::VisibleOnly, &layout);
        assert_eq!(shape.row_count(), 7);
        assert_eq!(row_numbers(&shape), vec![1, 2, 6, 7, 8, 9, 10]);
        assert_eq!(shape.col_count(), 1);
    }

    #[test]
    fn hidden_rows_at_the_edges_are_dropped() {
        let layout = layout_hiding(&[0, 1, 9], &[]);
        let shape = CopyShape::of(range("A1:A10"), CopyScope::VisibleOnly, &layout);
        assert_eq!(row_numbers(&shape), vec![3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(shape.blocks().len(), 1);
    }

    #[test]
    fn hidden_columns_are_dropped() {
        let layout = layout_hiding(&[], &[1, 3]);
        let shape = CopyShape::of(range("A1:E2"), CopyScope::VisibleOnly, &layout);
        assert_eq!(col_numbers(&shape), vec![1, 3, 5]);
        assert_eq!(shape.row_count(), 2);
        assert_eq!(shape.col_count(), 3);
    }

    #[test]
    fn rows_and_columns_hidden_together_leave_one_block_per_pair_of_runs() {
        let layout = layout_hiding(&[1], &[1]);
        let shape = CopyShape::of(range("A1:C3"), CopyScope::VisibleOnly, &layout);
        let blocks: Vec<(Range, u32, u16)> = shape
            .blocks()
            .into_iter()
            .map(|block| (block.source, block.row_offset, block.col_offset))
            .collect();
        assert_eq!(
            blocks,
            vec![
                (range("A1"), 0, 0),
                (range("C1"), 0, 1),
                (range("A3"), 1, 0),
                (range("C3"), 1, 1),
            ]
        );
    }

    #[test]
    fn every_row_hidden_leaves_nothing() {
        let layout = layout_hiding(&[0, 1, 2], &[]);
        let shape = CopyShape::of(range("A1:B3"), CopyScope::VisibleOnly, &layout);
        assert!(shape.is_empty());
        assert_eq!(shape.row_count(), 0);
    }

    #[test]
    fn every_column_hidden_leaves_nothing() {
        let layout = layout_hiding(&[], &[0, 1]);
        let shape = CopyShape::of(range("A1:B3"), CopyScope::VisibleOnly, &layout);
        assert!(shape.is_empty());
        assert_eq!(shape.col_count(), 0);
    }

    #[test]
    fn nothing_hidden_is_the_whole_selection_as_one_block() {
        let layout = layout_hiding(&[], &[]);
        let selection = range("B2:D5");
        let shape = CopyShape::of(selection, CopyScope::VisibleOnly, &layout);
        assert_eq!(
            shape.blocks(),
            vec![Block {
                source: selection,
                row_offset: 0,
                col_offset: 0
            }]
        );
        assert_eq!(shape, CopyShape::of(selection, CopyScope::Plain, &layout));
    }

    #[test]
    fn a_plain_copy_takes_manually_hidden_lines() {
        let layout = layout_hiding(&[2, 3, 4], &[1]);
        let shape = CopyShape::of(range("A1:C10"), CopyScope::Plain, &layout);
        assert_eq!(shape.row_count(), 10);
        assert_eq!(shape.col_count(), 3);
        assert_eq!(shape.blocks().len(), 1);
    }

    #[test]
    fn a_plain_copy_leaves_out_the_rows_a_filter_hides() {
        let layout = layout_hiding(&[], &[]);
        let shape = CopyShape::of(range("A1:B10"), CopyScope::Plain, &layout);
        let hidden = [
            (RowIdx::clamped(2), RowIdx::clamped(4)),
            (RowIdx::clamped(7), RowIdx::clamped(9)),
        ];
        let kept = shape.leaving_out_rows(&hidden);
        assert_eq!(row_numbers(&kept), vec![1, 2, 6, 7]);
        assert_eq!(kept.blocks().len(), 2);
        assert_eq!(kept.col_count(), 2);
    }

    #[test]
    fn a_filter_hiding_the_edges_leaves_one_block() {
        let layout = layout_hiding(&[], &[]);
        let shape = CopyShape::of(range("A1:A10"), CopyScope::Plain, &layout);
        let kept = shape.leaving_out_rows(&[(RowIdx::clamped(0), RowIdx::clamped(2))]);
        assert_eq!(row_numbers(&kept), vec![4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(kept.blocks().len(), 1);
        let none = shape.leaving_out_rows(&[(RowIdx::clamped(0), RowIdx::clamped(9))]);
        assert!(none.is_empty());
    }

    #[test]
    fn a_filter_that_hides_nothing_in_the_selection_changes_nothing() {
        let layout = layout_hiding(&[], &[]);
        let shape = CopyShape::of(range("A1:A5"), CopyScope::Plain, &layout);
        assert_eq!(
            shape.leaving_out_rows(&[(RowIdx::clamped(20), RowIdx::clamped(30))]),
            shape
        );
        assert_eq!(shape.leaving_out_rows(&[]), shape);
    }

    #[test]
    fn within_cuts_runs_at_the_end_of_the_used_area() {
        let layout = layout_hiding(&[2], &[]);
        let shape = CopyShape::of(range("A1:C10"), CopyScope::VisibleOnly, &layout);
        let used = shape.within(CellPos::parse_a1("B5").unwrap());
        assert_eq!(row_numbers(&used), vec![1, 2, 4, 5]);
        assert_eq!(col_numbers(&used), vec![1, 2]);
        assert_eq!(used.cell_count(), 8);
        assert_eq!(
            shape.within(CellPos::parse_a1("A1").unwrap()).cell_count(),
            1
        );
    }

    #[test]
    fn within_is_empty_when_the_shape_starts_past_the_used_area() {
        let shape = CopyShape::whole(range("D4:E6"));
        assert!(shape.within(CellPos::parse_a1("C9").unwrap()).is_empty());
        assert!(shape.within(CellPos::parse_a1("E3").unwrap()).is_empty());
    }

    #[test]
    fn a_marked_selection_copies_the_visible_cells_until_the_mark_clears() {
        let mut mark = VisibleMark::default();
        assert_eq!(mark.copy_scope(), CopyScope::Plain);
        mark.set();
        assert_eq!(mark.copy_scope(), CopyScope::VisibleOnly);
        mark.clear();
        assert_eq!(mark.copy_scope(), CopyScope::Plain);
    }

    #[test]
    fn block_count_is_the_product_of_the_runs_without_building_blocks() {
        let hidden_rows: Vec<u32> = (0..40_000).filter(|row| row % 2 == 1).collect();
        let hidden_cols: Vec<u16> = (0..200).filter(|col| col % 2 == 1).collect();
        let layout = layout_hiding(&hidden_rows, &hidden_cols);
        let shape = CopyShape::of(range("A1:GR40000"), CopyScope::VisibleOnly, &layout);
        assert_eq!(shape.block_count(), 20_000 * 100);
        assert_eq!(shape.row_count(), 20_000);
        assert_eq!(shape.col_count(), 100);
    }

    #[test]
    fn the_first_cell_is_the_first_visible_one() {
        let layout = layout_hiding(&[0, 1], &[0]);
        let shape = CopyShape::of(range("A1:C5"), CopyScope::VisibleOnly, &layout);
        assert_eq!(shape.first_cell(), CellPos::parse_a1("B3"));
        let hidden = layout_hiding(&[0, 1, 2, 3, 4], &[]);
        let shape = CopyShape::of(range("A1:C5"), CopyScope::VisibleOnly, &hidden);
        assert_eq!(shape.first_cell(), None);
    }
}
