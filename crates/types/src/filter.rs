use crate::{ColIdx, Range, RowIdx};

// Excel lists at most this many values in a filter list, and says not all are showing.
pub const MAX_FILTER_ITEMS: usize = 10_000;

// The values a filter column shows, compared with each cell's shown text ignoring case,
// as Excel does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShownValues {
    pub values: Vec<String>,
    pub blanks: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnCriteria {
    Values(ShownValues),
    // A rule made in Excel that Zenkai keeps as it is but cannot edit: a custom, number,
    // date, colour or top 10 filter.
    Custom,
}

// What the grid draws for a sheet's AutoFilter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterView {
    // The header row first.
    pub range: Range,
    // Columns with criteria, in column order.
    pub criteria: Vec<(ColIdx, ColumnCriteria)>,
    // Columns the file asks to draw without a button.
    pub without_button: Vec<ColIdx>,
    // Runs of data rows the filter hides, first and last, in order.
    pub filtered_out: Vec<(RowIdx, RowIdx)>,
    // Excel's "N of M records found": visible data rows of all data rows.
    pub found: u32,
    pub total: u32,
}

impl FilterView {
    pub fn is_filtering(&self) -> bool {
        !self.criteria.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterItem {
    pub text: String,
    // Data rows holding it, among those the other columns' criteria let through.
    pub count: u32,
    // Whether the column's criteria show it: the list's check.
    pub shown: bool,
}

// The list of a filter column, in Excel's order: numbers, text, logicals, errors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterList {
    pub items: Vec<FilterItem>,
    // The "(Blanks)" entry, listed last, only when some row is empty there; its text is
    // empty.
    pub blanks: Option<FilterItem>,
    // More than MAX_FILTER_ITEMS values matched: only the first are listed.
    pub truncated: bool,
    // The column holds a rule made in Excel the list cannot show.
    pub custom: bool,
}
