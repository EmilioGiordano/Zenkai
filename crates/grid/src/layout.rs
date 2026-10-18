use std::collections::BTreeMap;

use zenkai_types::{
    ColIdx, DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, MAX_COLS, MAX_ROWS, RowIdx, SheetSizes,
};

// Enough for any real window; a size that is tiny but positive must not make a viewport
// fill with millions of lines.
const MAX_SHOWN_LINES: usize = 4096;

// Hidden lines are stored as maximal runs (start to end), so skipping one is a lookup
// instead of a walk over every hidden index.
#[derive(Clone, Debug)]
struct Lines {
    sizes: BTreeMap<u32, f32>,
    hidden: BTreeMap<u32, u32>,
    default: f32,
    count: u32,
}

impl Lines {
    fn new(default: f32, count: u32) -> Lines {
        Lines {
            sizes: BTreeMap::new(),
            hidden: BTreeMap::new(),
            default,
            count,
        }
    }

    fn run_containing(&self, index: u32) -> Option<(u32, u32)> {
        let (&start, &end) = self.hidden.range(..=index).next_back()?;
        (end >= index).then_some((start, end))
    }

    fn size(&self, index: u32) -> f32 {
        if self.run_containing(index).is_some() {
            return 0.0;
        }
        self.sizes.get(&index).copied().unwrap_or(self.default)
    }

    // A file can carry negative or non-finite sizes; they hide the line like a zero does.
    fn set(&mut self, index: u32, size: f32) {
        if size.is_finite() && size > 0.0 {
            self.show(index);
            self.sizes.insert(index, size);
        } else {
            self.sizes.remove(&index);
            self.hide(index);
        }
    }

    fn hide(&mut self, index: u32) {
        if self.run_containing(index).is_some() {
            return;
        }
        let mut start = index;
        let mut end = index;
        if let Some(before) = index.checked_sub(1)
            && let Some((run_start, _)) = self.run_containing(before)
        {
            start = run_start;
        }
        if let Some(run_end) = self.hidden.remove(&(index + 1)) {
            end = run_end;
        }
        self.hidden.insert(start, end);
    }

    fn show(&mut self, index: u32) {
        let Some((start, end)) = self.run_containing(index) else {
            return;
        };
        self.hidden.remove(&start);
        if start < index {
            self.hidden.insert(start, index - 1);
        }
        if index < end {
            self.hidden.insert(index + 1, end);
        }
    }

    fn next_shown(&self, from: u32) -> Option<u32> {
        let index = self.run_containing(from).map_or(from, |(_, end)| end + 1);
        (index < self.count).then_some(index)
    }

    fn prev_shown(&self, from: u32) -> Option<u32> {
        match self.run_containing(from) {
            Some((start, _)) => start.checked_sub(1),
            None => Some(from),
        }
    }

    fn shown_filling(&self, first: u32, extent: f32) -> Vec<u32> {
        let mut shown = Vec::new();
        let mut filled = 0.0;
        let mut next = first;
        while filled < extent
            && shown.len() < MAX_SHOWN_LINES
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

    // The first line to scroll to so that `last` ends inside `extent`, never before `floor`.
    fn first_ending_at(&self, last: u32, extent: f32, floor: u32) -> u32 {
        let mut first = last;
        let mut filled = self.size(last);
        let mut steps = 0;
        while steps < MAX_SHOWN_LINES
            && let Some(before) = first.checked_sub(1)
            && let Some(index) = self.prev_shown(before)
            && index >= floor
            && filled + self.size(index) <= extent
        {
            filled += self.size(index);
            first = index;
            steps += 1;
        }
        first
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
            columns: Lines::new(DEFAULT_COL_WIDTH, u32::from(MAX_COLS)),
            rows: Lines::new(DEFAULT_ROW_HEIGHT, MAX_ROWS),
        }
    }
}

impl Layout {
    pub fn from_sizes(sizes: &SheetSizes) -> Layout {
        let mut layout = Layout::default();
        for span in &sizes.columns {
            for col in span.first.get()..=span.last.get() {
                layout.columns.set(u32::from(col), span.width);
            }
        }
        for (row, height) in &sizes.rows {
            layout.rows.set(row.get(), *height);
        }
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

    pub fn col_ending_at(&self, last: ColIdx, width: f32, floor: ColIdx) -> ColIdx {
        let first =
            self.columns
                .first_ending_at(u32::from(last.get()), width, u32::from(floor.get()));
        col_of(first)
    }

    pub fn row_ending_at(&self, last: RowIdx, height: f32, floor: RowIdx) -> RowIdx {
        row_of(self.rows.first_ending_at(last.get(), height, floor.get()))
    }

    pub fn cols_after(&self, col: ColIdx, count: u32) -> Vec<ColIdx> {
        let shown = self.columns.shown_after(u32::from(col.get()), count);
        shown.into_iter().map(col_of).collect()
    }

    // The first line that takes space at or after the given one, so a scroll origin never
    // rests inside a hidden run.
    pub fn row_from(&self, row: RowIdx) -> RowIdx {
        self.rows.next_shown(row.get()).map_or(row, row_of)
    }

    pub fn col_from(&self, col: ColIdx) -> ColIdx {
        self.columns
            .next_shown(u32::from(col.get()))
            .map_or(col, col_of)
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
        self.columns.set(u32::from(col.get()), width);
    }

    pub fn set_row_height(&mut self, row: RowIdx, height: f32) {
        self.rows.set(row.get(), height);
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
    fn negative_and_non_finite_sizes_hide_the_line() {
        let mut layout = Layout::default();
        layout.set_row_height(row_of(3), -5.0);
        layout.set_row_height(row_of(4), f32::NAN);
        layout.set_col_width(col_of(1), f32::NEG_INFINITY);
        assert_eq!(layout.row_height(row_of(3)), 0.0);
        assert_eq!(layout.row_height(row_of(4)), 0.0);
        let rows = layout.shown_rows(RowIdx::default(), 1.0e9);
        assert!(rows.len() <= MAX_SHOWN_LINES);
        assert!(!rows.contains(&row_of(3)) && !rows.contains(&row_of(4)));
        assert!(
            !layout
                .shown_cols(ColIdx::default(), 100.0)
                .contains(&col_of(1))
        );
    }

    #[test]
    fn tiny_positive_sizes_cannot_fill_the_sheet() {
        let mut layout = Layout::default();
        for row in 0..10_000 {
            layout.set_row_height(row_of(row), 1.0e-30);
        }
        let rows = layout.shown_rows(RowIdx::default(), 800.0);
        assert_eq!(rows.len(), MAX_SHOWN_LINES);
    }

    #[test]
    fn a_hidden_run_is_one_entry_and_skipping_it_is_a_lookup() {
        let layout = rows_hidden(2, 1_000_000);
        assert_eq!(layout.rows.hidden.len(), 1);
        let mut total = 0u64;
        for from in 2..20_000 {
            total += u64::from(layout.rows.next_shown(from).unwrap());
            total += u64::from(layout.rows.prev_shown(from).unwrap());
        }
        assert_eq!(total, 19_998 * 1_000_002);
    }

    #[test]
    fn showing_a_line_splits_its_hidden_run() {
        let mut layout = rows_hidden(2, 10);
        layout.set_row_height(row_of(5), 30.0);
        assert_eq!(layout.rows.hidden.len(), 2);
        assert_eq!(layout.row_height(row_of(5)), 30.0);
        assert_eq!(layout.row_height(row_of(4)), 0.0);
        assert_eq!(layout.row_height(row_of(6)), 0.0);
        layout.set_row_height(row_of(5), 0.0);
        assert_eq!(layout.rows.hidden.len(), 1);
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

    #[test]
    fn scrolling_to_a_far_column_fits_wide_columns_and_skips_hidden_ones() {
        let mut layout = Layout::default();
        for col in 10..=30 {
            layout.set_col_width(col_of(col), 120.0);
        }
        for col in 12..=14 {
            layout.set_col_width(col_of(col), 0.0);
        }
        let width = 500.0;
        let target = col_of(31);
        let left = layout.col_ending_at(target, width, ColIdx::default());
        let shown = layout.shown_cols(left, width);
        assert!(shown.contains(&target));
        assert!(!shown.contains(&col_of(13)));
        let fitted: f32 = (left.get()..=target.get())
            .map(|col| layout.col_width(col_of(u32::from(col))))
            .sum();
        assert!(fitted <= width);
        let wider = layout.col_ending_at(target, width + 120.0, ColIdx::default());
        assert!(wider < left);
    }

    #[test]
    fn scrolling_to_a_column_never_passes_the_floor() {
        let layout = Layout::default();
        let floor = col_of(3);
        assert_eq!(layout.col_ending_at(col_of(5), 10_000.0, floor), floor);
        assert_eq!(layout.col_ending_at(col_of(5), 1.0, floor), col_of(5));
    }

    #[test]
    fn scrolling_to_a_far_row_fits_tall_rows_and_skips_hidden_ones() {
        let mut layout = rows_hidden(40, 60);
        for row in 30..=70 {
            if !(40..=60).contains(&row) {
                layout.set_row_height(row_of(row), 50.0);
            }
        }
        let height = 300.0;
        let target = row_of(70);
        let top = layout.row_ending_at(target, height, RowIdx::default());
        let shown = layout.shown_rows(top, height);
        assert!(shown.contains(&target));
        assert!(!shown.contains(&row_of(45)));
        let fitted: f32 = shown.iter().map(|row| layout.row_height(*row)).sum();
        assert!(fitted <= height);
    }
}
