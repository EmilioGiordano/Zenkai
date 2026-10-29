use std::collections::HashMap;
use std::mem::size_of;

use crate::{
    calc_result::CalcResult, expressions::types::CellReferenceIndex,
    functions::util::has_wildcards, model::Model, number_format::to_excel_precision, types::Cell,
};

// Bounds what the indexes of one evaluation hold. An entry costs `ENTRY_BYTES` plus its
// text, so a 200k-row column of distinct 8-character codes costs about 13 MB and this
// fits about five of them; past it the functions search cell by cell as without indexes.
const INDEX_BYTES: usize = 64 << 20;
// A hash map slot holds the key, the span and a control byte, and the map grows once it
// is 7/8 full, so a slot is at worst this much per entry.
const ENTRY_BYTES: usize = (size_of::<LookupKey>() + size_of::<Span>() + 1) * 8 / 7;
// What remembering a vector without an index costs.
const VECTOR_BYTES: usize = (size_of::<(Vector, Equality)>() + size_of::<Kept>() + 1) * 8 / 7;

/// A single row or column of cells searched by a lookup function.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Vector {
    sheet: u32,
    row: i32,
    column: i32,
    length: i32,
    along: Along,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Along {
    Rows,
    Columns,
}

impl Vector {
    /// `length` cells down from `start`.
    pub(crate) fn down(start: CellReferenceIndex, length: i32) -> Vector {
        Vector::new(start, length, Along::Rows)
    }

    /// `length` cells right from `start`.
    pub(crate) fn across(start: CellReferenceIndex, length: i32) -> Vector {
        Vector::new(start, length, Along::Columns)
    }

    /// The vector covering a range one row high or one column wide.
    pub(crate) fn from_range(
        left: CellReferenceIndex,
        right: CellReferenceIndex,
    ) -> Option<Vector> {
        if left.sheet != right.sheet {
            None
        } else if left.row == right.row {
            Some(Vector::across(left, right.column - left.column + 1))
        } else if left.column == right.column {
            Some(Vector::down(left, right.row - left.row + 1))
        } else {
            None
        }
    }

    fn new(start: CellReferenceIndex, length: i32, along: Along) -> Vector {
        Vector {
            sheet: start.sheet,
            row: start.row,
            column: start.column,
            length,
            along,
        }
    }
}

/// How a lookup function decides that a value in the vector equals the value it looks for.
/// Each matches the comparison the function makes when it searches cell by cell.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Equality {
    /// VLOOKUP and HLOOKUP: text as `wildcard_matcher`, anything else as `compare_values`.
    Lookup,
    /// MATCH: text as `wildcard_matcher`, anything else as `values_are_equal`.
    Match,
    /// XLOOKUP: everything as `compare_values`.
    Xlookup,
}

impl Equality {
    fn text_key(self, text: &str) -> Box<str> {
        match self {
            Equality::Lookup | Equality::Match => text.to_lowercase().into(),
            Equality::Xlookup => text.to_uppercase().into(),
        }
    }

    // Both comparisons hold numbers within `f64::EPSILON` equal, which is not transitive near
    // zero. From these magnitudes on, distinct values are at least that far apart, so equal
    // means equal bits; smaller numbers are searched cell by cell. So are infinities, which
    // equal nothing there because their difference is NaN.
    fn number_key(self, value: f64) -> Option<u64> {
        if !value.is_finite() {
            return None;
        }
        match self {
            Equality::Lookup | Equality::Xlookup => {
                let rounded = to_excel_precision(value, 15);
                (rounded.abs() >= 1.0).then(|| rounded.to_bits())
            }
            Equality::Match => (value.abs() >= 2.0).then(|| value.to_bits()),
        }
    }

    // The key a lookup value is found by, and whether a blank cell equals it too, or None
    // when an index cannot answer.
    fn wanted(self, value: &CalcResult) -> Option<(LookupKey, bool)> {
        match value {
            CalcResult::String(text) => match self {
                Equality::Lookup | Equality::Match if has_wildcards(text) => None,
                Equality::Lookup | Equality::Match => {
                    Some((LookupKey::Text(self.text_key(text)), false))
                }
                // `compare_values` reads a blank as "". Kept as the cell by cell search does.
                Equality::Xlookup => Some((LookupKey::Text(self.text_key(text)), text.is_empty())),
            },
            CalcResult::Number(number) => {
                Some((LookupKey::Number(self.number_key(*number)?), false))
            }
            // `compare_values` reads a blank as FALSE. Kept as the cell by cell search does.
            CalcResult::Boolean(boolean) => Some((
                LookupKey::Boolean(*boolean),
                !boolean && self != Equality::Match,
            )),
            _ => None,
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
enum LookupKey {
    Text(Box<str>),
    Number(u64),
    Boolean(bool),
}

/// Which match a lookup function returns when several values are equal.
#[derive(Clone, Copy)]
pub(crate) enum SearchFrom {
    First,
    Last,
}

// The first and last 0-based positions holding a value.
#[derive(Clone, Copy)]
struct Span {
    first: usize,
    last: usize,
}

impl Span {
    fn add(span: &mut Option<Span>, position: usize) {
        match span {
            Some(span) => span.last = position,
            None => {
                *span = Some(Span {
                    first: position,
                    last: position,
                })
            }
        }
    }

    fn pick(spans: [Option<Span>; 2], from: SearchFrom) -> Option<usize> {
        let positions = spans.into_iter().flatten();
        match from {
            SearchFrom::First => positions.map(|span| span.first).min(),
            SearchFrom::Last => positions.map(|span| span.last).max(),
        }
    }
}

struct LookupIndex {
    spans: HashMap<LookupKey, Span>,
    blanks: Option<Span>,
}

enum Kept {
    // Searched once; a second search builds the index, so a vector searched only once, like
    // the growing `$A$1:A1` of a running count, costs no more than before.
    Seen,
    // Holds a formula, or did not fit the budget.
    Unindexed,
    Indexed(LookupIndex),
}

/// Indexes of the vectors that lookup functions search for an exact match, built from cells
/// that hold no formula, so they hold for the whole evaluation.
///
/// They live only while an evaluation runs: nothing that changes cells runs then, except a
/// spill, which calls `forget`.
pub(crate) struct LookupIndexes {
    active: bool,
    enabled: bool,
    kept: HashMap<(Vector, Equality), Kept>,
    budget: usize,
    bytes_left: usize,
}

impl LookupIndexes {
    pub(crate) fn new() -> LookupIndexes {
        LookupIndexes::with_budget(INDEX_BYTES)
    }

    // A separate constructor so tests can exhaust a small budget.
    fn with_budget(budget: usize) -> LookupIndexes {
        LookupIndexes {
            active: false,
            enabled: true,
            kept: HashMap::new(),
            budget,
            bytes_left: budget,
        }
    }

    pub(crate) fn start(&mut self) {
        self.forget();
        self.active = self.enabled;
    }

    pub(crate) fn finish(&mut self) {
        self.forget();
        self.active = false;
    }

    // A spill writes or clears cells an index may cover.
    pub(crate) fn forget(&mut self) {
        self.kept = HashMap::new();
        self.bytes_left = self.budget;
    }

    #[cfg(test)]
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn spend(&mut self, bytes: usize) -> bool {
        if self.bytes_left >= bytes {
            self.bytes_left -= bytes;
            true
        } else {
            false
        }
    }
}

impl Model<'_> {
    /// The position of the first or last value of `vector` equal to `value`, `Some(None)`
    /// when there is none, or `None` when no index can answer and the caller must search
    /// cell by cell.
    pub(crate) fn indexed_position(
        &mut self,
        vector: Vector,
        equality: Equality,
        value: &CalcResult,
        from: SearchFrom,
    ) -> Option<Option<usize>> {
        if !self.lookup_indexes.active || vector.length < 1 {
            return None;
        }
        let (key, blanks_match) = equality.wanted(value)?;
        let indexes = &mut self.lookup_indexes;
        match indexes.kept.get(&(vector, equality)) {
            Some(Kept::Indexed(index)) => {
                let blanks = if blanks_match { index.blanks } else { None };
                return Some(Span::pick([index.spans.get(&key).copied(), blanks], from));
            }
            Some(Kept::Unindexed) => return None,
            Some(Kept::Seen) => {}
            None => {
                if indexes.spend(VECTOR_BYTES) {
                    indexes.kept.insert((vector, equality), Kept::Seen);
                }
                return None;
            }
        }
        let built = self.build_lookup_index(vector, equality);
        let indexes = &mut self.lookup_indexes;
        match built {
            Some(index) => {
                let blanks = if blanks_match { index.blanks } else { None };
                let position = Span::pick([index.spans.get(&key).copied(), blanks], from);
                indexes
                    .kept
                    .insert((vector, equality), Kept::Indexed(index));
                Some(position)
            }
            None => {
                indexes.kept.insert((vector, equality), Kept::Unindexed);
                None
            }
        }
    }

    // None when a cell holds a formula, whose value may still change during the evaluation,
    // or when the index does not fit the budget.
    fn build_lookup_index(&mut self, vector: Vector, equality: Equality) -> Option<LookupIndex> {
        let worksheet = self.workbook.worksheets.get(vector.sheet as usize)?;
        let store = &worksheet.sheet_data;
        let length = vector.length as usize;
        let cells: Vec<(usize, &Cell)> = match vector.along {
            Along::Rows if length <= store.row_count() => (0..vector.length)
                .filter_map(|offset| {
                    let cell = store.get(vector.row + offset, vector.column)?;
                    Some((offset as usize, cell))
                })
                .collect(),
            Along::Rows => store
                .rows_sorted()
                .into_iter()
                .filter(|row| (vector.row..vector.row + vector.length).contains(row))
                .filter_map(|row| {
                    let cell = store.get(row, vector.column)?;
                    Some(((row - vector.row) as usize, cell))
                })
                .collect(),
            Along::Columns => store
                .row_cells(vector.row)
                .filter(|(column, _)| {
                    (vector.column..vector.column + vector.length).contains(column)
                })
                .map(|(column, cell)| ((column - vector.column) as usize, cell))
                .collect(),
        };
        let mut spans: HashMap<LookupKey, Span> = HashMap::new();
        let mut blanks = None;
        let mut bytes = 0;
        let mut next = 0;
        for (position, cell) in cells {
            if position > next {
                Span::add(&mut blanks, next);
                Span::add(&mut blanks, position - 1);
            }
            next = position + 1;
            let key = match cell {
                Cell::EmptyCell { .. } => {
                    Span::add(&mut blanks, position);
                    continue;
                }
                Cell::BooleanCell { v, .. } => LookupKey::Boolean(*v),
                Cell::NumberCell { v, .. } => match equality.number_key(*v) {
                    Some(bits) => LookupKey::Number(bits),
                    None => continue,
                },
                Cell::SharedString { si, .. } => {
                    match self.workbook.shared_strings.get(*si as usize) {
                        Some(text) => LookupKey::Text(equality.text_key(text)),
                        // Reads as an error, which equals no lookup value an index answers.
                        None => continue,
                    }
                }
                // An error equals no lookup value an index answers.
                Cell::ErrorCell { .. } => continue,
                Cell::CellFormula { .. } | Cell::ArrayFormula(_) | Cell::SpillCell(_) => {
                    return None
                }
            };
            if let Some(span) = spans.get_mut(&key) {
                span.last = position;
                continue;
            }
            bytes += ENTRY_BYTES
                + match &key {
                    LookupKey::Text(text) => text.len(),
                    _ => 0,
                };
            if bytes > self.lookup_indexes.bytes_left {
                return None;
            }
            spans.insert(
                key,
                Span {
                    first: position,
                    last: position,
                },
            );
        }
        if next < length {
            Span::add(&mut blanks, next);
            Span::add(&mut blanks, length - 1);
        }
        if !self.lookup_indexes.spend(bytes) {
            return None;
        }
        Some(LookupIndex { spans, blanks })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Equality, LookupIndexes, SearchFrom, Vector, ENTRY_BYTES, VECTOR_BYTES};
    use crate::calc_result::CalcResult;
    use crate::expressions::types::CellReferenceIndex;
    use crate::test::util::new_empty_model;
    use crate::Model;

    const ROWS: i32 = 100;

    fn column_a() -> Vector {
        let start = CellReferenceIndex {
            sheet: 0,
            row: 1,
            column: 1,
        };
        Vector::down(start, ROWS)
    }

    // A1:A100 holds 2 to 101; the indexes are started with `budget` bytes.
    fn numbers(budget: usize) -> Model<'static> {
        let mut model = new_empty_model();
        for row in 1..=ROWS {
            model._set(&format!("A{row}"), &(row + 1).to_string());
        }
        model.evaluate();
        model.lookup_indexes = LookupIndexes::with_budget(budget);
        model.lookup_indexes.start();
        model
    }

    fn find(model: &mut Model, number: f64) -> Option<Option<usize>> {
        let value = CalcResult::Number(number);
        model.indexed_position(column_a(), Equality::Match, &value, SearchFrom::First)
    }

    #[test]
    fn a_vector_is_indexed_when_searched_a_second_time() {
        let mut model = numbers(super::INDEX_BYTES);
        assert_eq!(find(&mut model, 50.0), None);
        assert_eq!(find(&mut model, 50.0), Some(Some(48)));
        assert_eq!(find(&mut model, 7.0), Some(Some(5)));
        assert_eq!(find(&mut model, 500.0), Some(None));
        model.lookup_indexes.finish();
        assert_eq!(find(&mut model, 50.0), None);
    }

    #[test]
    fn an_index_is_kept_only_within_the_budget() {
        let needed = VECTOR_BYTES + ROWS as usize * ENTRY_BYTES;
        let mut model = numbers(needed);
        find(&mut model, 50.0);
        assert_eq!(find(&mut model, 50.0), Some(Some(48)));
        let mut model = numbers(needed - 1);
        find(&mut model, 50.0);
        assert_eq!(find(&mut model, 50.0), None);
        assert_eq!(find(&mut model, 50.0), None);
    }

    #[test]
    fn small_numbers_are_searched_cell_by_cell() {
        let mut model = numbers(super::INDEX_BYTES);
        find(&mut model, 50.0);
        assert_eq!(find(&mut model, 1.5), None);
        assert_eq!(find(&mut model, 0.0), None);
    }
}
