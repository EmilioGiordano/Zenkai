use std::collections::BTreeMap;

use zenkai_types::{
    ColIdx, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, MAX_COLS, MAX_ROWS, RowIdx, SheetSizes,
};

#[derive(Clone, Debug, Default)]
pub struct Layout {
    columns: BTreeMap<u16, f32>,
    rows: BTreeMap<u32, f32>,
}

impl Layout {
    pub fn from_sizes(sizes: &SheetSizes) -> Layout {
        let mut columns = BTreeMap::new();
        for span in &sizes.columns {
            for col in span.first.get()..=span.last.get() {
                columns.insert(col, span.width);
            }
        }
        let rows = sizes.rows.iter().map(|(row, h)| (row.get(), *h)).collect();
        Layout { columns, rows }
    }

    pub fn col_width(&self, col: ColIdx) -> f32 {
        self.columns
            .get(&col.get())
            .copied()
            .unwrap_or(DEFAULT_COL_WIDTH)
    }

    pub fn row_height(&self, row: RowIdx) -> f32 {
        self.rows
            .get(&row.get())
            .copied()
            .unwrap_or(DEFAULT_ROW_HEIGHT)
    }

    pub fn visible_cols(&self, left: ColIdx, width: f32) -> u16 {
        let mut x = 0.0;
        let mut count = 0u16;
        let mut col = left.get();
        while x < width && col < MAX_COLS {
            x += self.col_width(ColIdx::clamped(i64::from(col)));
            count += 1;
            col += 1;
        }
        count.max(1)
    }

    pub fn visible_rows(&self, top: RowIdx, height: f32) -> u32 {
        let mut y = 0.0;
        let mut count = 0u32;
        let mut row = top.get();
        while y < height && row < MAX_ROWS {
            y += self.row_height(RowIdx::clamped(i64::from(row)));
            count += 1;
            row += 1;
        }
        count.max(1)
    }

    pub fn col_at(&self, left: ColIdx, x: f32) -> ColIdx {
        let mut edge = 0.0;
        let mut col = left;
        loop {
            edge += self.col_width(col);
            if x < edge || col == ColIdx::LAST {
                return col;
            }
            col = col.offset(1);
        }
    }

    pub fn row_at(&self, top: RowIdx, y: f32) -> RowIdx {
        let mut edge = 0.0;
        let mut row = top;
        loop {
            edge += self.row_height(row);
            if y < edge || row == RowIdx::LAST {
                return row;
            }
            row = row.offset(1);
        }
    }

    pub fn set_col_width(&mut self, col: ColIdx, width: f32) {
        self.columns.insert(col.get(), width);
    }
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
        assert_eq!(layout.visible_rows(top, 100.0), 4);
    }
}
