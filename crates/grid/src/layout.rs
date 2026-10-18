use std::collections::BTreeMap;

use zenkai_types::{
    ColIdx, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, MAX_COLS, MAX_ROWS, RowIdx, SheetSizes,
};

#[derive(Clone, Debug)]
struct Lines {
    sizes: BTreeMap<u32, f32>,
    default: f32,
    count: u32,
}

impl Lines {
    fn size(&self, index: u32) -> f32 {
        self.sizes.get(&index).copied().unwrap_or(self.default)
    }

    // Hidden lines are consecutive zero-size entries, so skipping them walks the map keys
    // instead of probing every index.
    fn next_shown(&self, from: u32) -> Option<u32> {
        let mut index = from;
        for (&key, &size) in self.sizes.range(from..) {
            if key != index || size != 0.0 {
                break;
            }
            index += 1;
        }
        (index < self.count).then_some(index)
    }

    fn prev_shown(&self, from: u32) -> Option<u32> {
        let mut index = from;
        for (&key, &size) in self.sizes.range(..=from).rev() {
            if key != index || size != 0.0 {
                break;
            }
            index = index.checked_sub(1)?;
        }
        Some(index)
    }

    fn shown_filling(&self, first: u32, extent: f32) -> Vec<u32> {
        let mut shown = Vec::new();
        let mut filled = 0.0;
        let mut next = first;
        while filled < extent
            && let Some(index) = self.next_shown(next)
        {
            filled += self.size(index);
            shown.push(index);
            next = index + 1;
        }
        shown
    }

    fn shown_after(&self, last: u32, count: u32) -> Vec<u32> {
        let mut shown = Vec::new();
        let mut next = last + 1;
        while shown.len() < count as usize
            && let Some(index) = self.next_shown(next)
        {
            shown.push(index);
            next = index + 1;
        }
        shown
    }

    fn shown_before(&self, first: u32, count: u32, floor: u32) -> Vec<u32> {
        let mut shown = Vec::new();
        let mut upper = first;
        while shown.len() < count as usize
            && let Some(before) = upper.checked_sub(1)
            && let Some(index) = self.prev_shown(before)
            && index >= floor
        {
            shown.push(index);
            upper = index;
        }
        shown.reverse();
        shown
    }

    fn index_at(&self, first: u32, offset: f32) -> u32 {
        let mut edge = 0.0;
        let mut index = first;
        loop {
            let Some(shown) = self.next_shown(index) else {
                return self.count - 1;
            };
            edge += self.size(shown);
            if offset < edge || shown == self.count - 1 {
                return shown;
            }
            index = shown + 1;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    columns: Lines,
    rows: Lines,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            columns: Lines {
                sizes: BTreeMap::new(),
                default: DEFAULT_COL_WIDTH,
                count: u32::from(MAX_COLS),
            },
            rows: Lines {
                sizes: BTreeMap::new(),
                default: DEFAULT_ROW_HEIGHT,
                count: MAX_ROWS,
            },
        }
    }
}

impl Layout {
    pub fn from_sizes(sizes: &SheetSizes) -> Layout {
        let mut layout = Layout::default();
        for span in &sizes.columns {
            for col in span.first.get()..=span.last.get() {
                layout.columns.sizes.insert(u32::from(col), span.width);
            }
        }
        layout.rows.sizes = sizes.rows.iter().map(|(row, h)| (row.get(), *h)).collect();
        layout
    }

    pub fn col_width(&self, col: ColIdx) -> f32 {
        self.columns.size(u32::from(col.get()))
    }

    pub fn row_height(&self, row: RowIdx) -> f32 {
        self.rows.size(row.get())
    }

    pub fn shown_cols(&self, left: ColIdx, width: f32) -> Vec<ColIdx> {
        let shown = self.columns.shown_filling(u32::from(left.get()), width);
        shown.into_iter().map(col_of).collect()
    }

    pub fn shown_rows(&self, top: RowIdx, height: f32) -> Vec<RowIdx> {
        let shown = self.rows.shown_filling(top.get(), height);
        shown.into_iter().map(row_of).collect()
    }

    pub fn cols_before(&self, col: ColIdx, count: u32, floor: ColIdx) -> Vec<ColIdx> {
        let shown = self
            .columns
            .shown_before(u32::from(col.get()), count, u32::from(floor.get()));
        shown.into_iter().map(col_of).collect()
    }

    pub fn rows_before(&self, row: RowIdx, count: u32, floor: RowIdx) -> Vec<RowIdx> {
        let shown = self.rows.shown_before(row.get(), count, floor.get());
        shown.into_iter().map(row_of).collect()
    }

    pub fn rows_after(&self, row: RowIdx, count: u32) -> Vec<RowIdx> {
        let shown = self.rows.shown_after(row.get(), count);
        shown.into_iter().map(row_of).collect()
    }

    pub fn col_at(&self, left: ColIdx, x: f32) -> ColIdx {
        col_of(self.columns.index_at(u32::from(left.get()), x))
    }

    pub fn row_at(&self, top: RowIdx, y: f32) -> RowIdx {
        row_of(self.rows.index_at(top.get(), y))
    }

    pub fn set_col_width(&mut self, col: ColIdx, width: f32) {
        self.columns.sizes.insert(u32::from(col.get()), width);
    }

    pub fn set_row_height(&mut self, row: RowIdx, height: f32) {
        self.rows.sizes.insert(row.get(), height);
    }
}

fn col_of(index: u32) -> ColIdx {
    ColIdx::clamped(i64::from(index))
}

fn row_of(index: u32) -> RowIdx {
    RowIdx::clamped(i64::from(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_types::ColumnSpan;

    #[test]
    fn hit_test_respects_custom_widths() {
        let sizes = SheetSizes {
            columns: vec![ColumnSpan {
                first: ColIdx::new(1).unwrap(),
                last: ColIdx::new(2).unwrap(),
                width: 100.0,
            }],
            rows: vec![(RowIdx::new(0).unwrap(), 40.0)],
        };
        let layout = Layout::from_sizes(&sizes);
        let left = ColIdx::new(0).unwrap();
        assert_eq!(layout.col_at(left, 63.0).get(), 0);
        assert_eq!(layout.col_at(left, 65.0).get(), 1);
        assert_eq!(layout.col_at(left, 263.0).get(), 2);
        assert_eq!(layout.col_at(left, 265.0).get(), 3);
        let top = RowIdx::new(0).unwrap();
        assert_eq!(layout.row_at(top, 39.0).get(), 0);
        assert_eq!(layout.row_at(top, 41.0).get(), 1);
        assert_eq!(layout.shown_rows(top, 100.0).len(), 4);
    }

    fn rows_hidden(first: u32, last: u32) -> Layout {
        let mut layout = Layout::default();
        for row in first..=last {
            layout.set_row_height(row_of(row), 0.0);
        }
        layout
    }

    #[test]
    fn a_huge_hidden_span_does_not_count_as_visible_rows() {
        let layout = rows_hidden(2, 1_000_000);
        let shown = layout.shown_rows(RowIdx::default(), 800.0);
        assert!(shown.len() <= 800 / DEFAULT_ROW_HEIGHT as usize + 1);
        assert_eq!(shown[..3], [0, 1, 1_000_001].map(row_of));
    }

    #[test]
    fn a_huge_hidden_span_does_not_count_as_visible_cols() {
        let mut layout = Layout::default();
        for col in 1..16_000 {
            layout.set_col_width(col_of(col), 0.0);
        }
        let shown = layout.shown_cols(ColIdx::default(), 1000.0);
        assert!(shown.len() <= 1000 / DEFAULT_COL_WIDTH as usize + 1);
        assert_eq!(shown[..2], [0, 16_000].map(col_of));
    }

    #[test]
    fn rows_hidden_to_the_end_leave_nothing_to_show() {
        let layout = rows_hidden(5, MAX_ROWS - 1);
        assert!(layout.shown_rows(row_of(5), 800.0).is_empty());
    }

    #[test]
    fn hit_testing_skips_a_huge_hidden_span() {
        let layout = rows_hidden(2, 1_000_000);
        let top = RowIdx::default();
        assert_eq!(layout.row_at(top, 3.0), row_of(0));
        assert_eq!(
            layout.row_at(top, DEFAULT_ROW_HEIGHT * 2.5),
            row_of(1_000_001)
        );
    }

    #[test]
    fn rows_before_and_after_skip_hidden_rows() {
        let layout = rows_hidden(2, 1_000_000);
        let after = layout.rows_after(row_of(1), 2);
        assert_eq!(after, [1_000_001, 1_000_002].map(row_of));
        let before = layout.rows_before(row_of(1_000_002), 3, RowIdx::default());
        assert_eq!(before, [0, 1, 1_000_001].map(row_of));
        let floored = layout.rows_before(row_of(1_000_002), 3, row_of(1));
        assert_eq!(floored, [1, 1_000_001].map(row_of));
    }
}
