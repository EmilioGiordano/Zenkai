use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;

use crate::constants::{self, LAST_COLUMN, LAST_ROW};
use crate::expressions::utils::{is_valid_column_number, is_valid_row};
use crate::types::{
    AutoFilter, FilterColumn, FilterCriteria, Row, SortCondition, SortState, Worksheet,
};

/// Rows or columns inserted or deleted, `at` being the first one (1-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMove {
    Insert { at: i32, count: i32 },
    Delete { at: i32, count: i32 },
}

impl LineMove {
    /// Where a row or column ends up, `None` if it was deleted.
    fn index(self, index: i32) -> Option<i32> {
        match self {
            LineMove::Insert { at, count } if index >= at => Some(index + count),
            LineMove::Delete { at, count } if index >= at + count => Some(index - count),
            LineMove::Delete { at, .. } if index >= at => None,
            _ => Some(index),
        }
    }

    /// What a range from `first` to `last` becomes, as a reference in a formula would:
    /// rows inserted inside it widen it, deleted ones narrow it. `None` once nothing of it
    /// is left in the sheet, whose last line is `limit`.
    fn span(self, first: i32, last: i32, limit: i32) -> Option<(i32, i32)> {
        let (first, last) = match self {
            LineMove::Insert { at, count } if at <= first => (first + count, last + count),
            LineMove::Insert { at, count } if at <= last => (first, last + count),
            LineMove::Insert { .. } => (first, last),
            LineMove::Delete { at, .. } => (
                self.index(first).unwrap_or(at),
                self.index(last).unwrap_or(at - 1),
            ),
        };
        let last = last.min(limit);
        (first <= last).then_some((first, last))
    }
}

impl AutoFilter {
    /// Whether some column hides rows.
    pub fn has_criteria(&self) -> bool {
        self.columns
            .iter()
            .any(|column| column.criteria != FilterCriteria::None)
    }

    /// The rows below the header.
    pub fn data_rows(&self) -> RangeInclusive<i32> {
        self.first_row + 1..=self.last_row
    }

    /// Whether the filter hid `row`.
    pub fn is_filtered(&self, row: i32) -> bool {
        self.filtered_rows.binary_search(&row).is_ok()
    }

    /// Whether the range, its columns and its sort are inside the sheet, and the columns
    /// and filtered rows sorted, unique and inside the range.
    pub fn is_valid(&self) -> bool {
        let width = self.last_column - self.first_column + 1;
        is_area(
            self.first_row,
            self.first_column,
            self.last_row,
            self.last_column,
        ) && self.sort_state.as_ref().is_none_or(SortState::is_valid)
            && self
                .columns
                .iter()
                .all(|column| (0..width).contains(&column.column))
            && self.columns.windows(2).all(|w| w[0].column < w[1].column)
            && self
                .filtered_rows
                .iter()
                .all(|row| self.data_rows().contains(row))
            && self.filtered_rows.windows(2).all(|w| w[0] < w[1])
    }

    /// The filter once rows moved; `None` when every row of it was deleted.
    pub fn with_rows_moved(&self, moved: LineMove) -> Option<AutoFilter> {
        let (first_row, last_row) = moved.span(self.first_row, self.last_row, LAST_ROW)?;
        let filtered_rows = self
            .filtered_rows
            .iter()
            .filter_map(|row| moved.index(*row))
            .filter(|row| (first_row + 1..=last_row).contains(row))
            .collect();
        let sort_state = self
            .sort_state
            .as_ref()
            .and_then(|state| state.with_rows_moved(moved));
        Some(AutoFilter {
            first_row,
            last_row,
            filtered_rows,
            sort_state,
            ..self.clone()
        })
    }

    /// The filter once columns moved; `None` when every column of it was deleted. The
    /// criteria of a deleted column go with it, as in Excel.
    pub fn with_columns_moved(&self, moved: LineMove) -> Option<AutoFilter> {
        let (first_column, last_column) =
            moved.span(self.first_column, self.last_column, LAST_COLUMN)?;
        let columns = self
            .columns
            .iter()
            .filter_map(|column| {
                let index = moved.index(self.first_column + column.column)?;
                (first_column..=last_column)
                    .contains(&index)
                    .then(|| FilterColumn {
                        column: index - first_column,
                        ..column.clone()
                    })
            })
            .collect();
        let sort_state = self
            .sort_state
            .as_ref()
            .and_then(|state| state.with_columns_moved(moved));
        Some(AutoFilter {
            first_column,
            last_column,
            columns,
            sort_state,
            ..self.clone()
        })
    }
}

fn is_area(first_row: i32, first_column: i32, last_row: i32, last_column: i32) -> bool {
    is_valid_row(first_row)
        && is_valid_row(last_row)
        && is_valid_column_number(first_column)
        && is_valid_column_number(last_column)
        && first_row <= last_row
        && first_column <= last_column
}

impl SortState {
    /// Whether the sorted range and every condition's range are inside the sheet.
    pub fn is_valid(&self) -> bool {
        is_area(
            self.first_row,
            self.first_column,
            self.last_row,
            self.last_column,
        ) && self.conditions.iter().all(|condition| {
            is_area(
                condition.first_row,
                condition.first_column,
                condition.last_row,
                condition.last_column,
            )
        })
    }

    // A sort that names a deleted line no longer sorts what it did, so it goes whole.
    fn with_rows_moved(&self, moved: LineMove) -> Option<SortState> {
        let (first_row, last_row) = moved.span(self.first_row, self.last_row, LAST_ROW)?;
        let conditions = self
            .conditions
            .iter()
            .map(|condition| {
                let (first_row, last_row) =
                    moved.span(condition.first_row, condition.last_row, LAST_ROW)?;
                Some(SortCondition {
                    first_row,
                    last_row,
                    ..condition.clone()
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(SortState {
            first_row,
            last_row,
            conditions,
            ..self.clone()
        })
    }

    fn with_columns_moved(&self, moved: LineMove) -> Option<SortState> {
        let (first_column, last_column) =
            moved.span(self.first_column, self.last_column, LAST_COLUMN)?;
        let conditions = self
            .conditions
            .iter()
            .map(|condition| {
                let (first_column, last_column) =
                    moved.span(condition.first_column, condition.last_column, LAST_COLUMN)?;
                Some(SortCondition {
                    first_column,
                    last_column,
                    ..condition.clone()
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(SortState {
            first_column,
            last_column,
            conditions,
            ..self.clone()
        })
    }
}

impl Worksheet {
    /// Counts every hidden data row of an AutoFilter that has criteria as hidden by it, as
    /// a file stores them; with no criteria the filter hides nothing.
    pub fn mark_filtered_rows(&mut self) {
        let Some(filter) = &self.auto_filter else {
            return;
        };
        let filtered_rows = if filter.has_criteria() {
            let data_rows = filter.data_rows();
            let mut rows: Vec<i32> = self
                .rows
                .iter()
                .filter(|row| row.hidden && data_rows.contains(&row.r))
                .map(|row| row.r)
                .collect();
            rows.sort_unstable();
            rows.dedup();
            rows
        } else {
            Vec::new()
        };
        if let Some(filter) = &mut self.auto_filter {
            filter.filtered_rows = filtered_rows;
        }
    }

    /// The hidden flag of each row with an entry, as `is_row_hidden` reads it: the first
    /// entry of a row counts.
    pub(crate) fn hidden_by_row(&self) -> HashMap<i32, bool> {
        let mut hidden = HashMap::with_capacity(self.rows.len());
        for row in &self.rows {
            hidden.entry(row.r).or_insert(row.hidden);
        }
        hidden
    }

    /// Gives each of `rows`, which have no entry, the entry hiding a row would give it.
    pub(crate) fn add_default_rows(&mut self, rows: &[i32]) {
        self.rows.extend(rows.iter().map(|r| Row {
            height: constants::DEFAULT_ROW_HEIGHT / constants::ROW_HEIGHT_FACTOR,
            r: *r,
            custom_format: false,
            custom_height: false,
            s: 0,
            hidden: false,
        }));
    }

    pub(crate) fn remove_row_entries(&mut self, rows: &[i32]) {
        let removed: HashSet<i32> = rows.iter().copied().collect();
        self.rows.retain(|row| !removed.contains(&row.r));
    }

    /// Flips the hidden flag of every entry of `rows`, in time proportional to the rows of
    /// the sheet and those listed.
    pub(crate) fn flip_rows_hidden(&mut self, rows: &[i32]) {
        let flipped: HashSet<i32> = rows.iter().copied().collect();
        for row in &mut self.rows {
            if flipped.contains(&row.r) {
                row.hidden = !row.hidden;
            }
        }
    }
}
