use std::collections::{HashMap, HashSet};

use crate::{expressions::parser::Node, functions::Function};

// (sheet, row, column), the key `Model::cells` uses.
pub(crate) type CellKey = (u32, i32, i32);

// Ranges wider than this are kept per sheet instead of once per column they cover.
const MAX_INDEXED_COLUMNS: i32 = 16;

// Nested ranges down a column (`=SUM($A$1:A9)` below `=SUM($A$1:A8)`...), or many wide
// ranges over a row that a large paste edits, make the entries examined grow with the
// square of the formulas; past this, a full evaluation is the cheaper way. An entry costs
// tens of ns, so this is about 0.1 to 0.2 s, against 2 s to evaluate 1M formulas.
const MAX_DEPENDENT_VISITS: usize = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reference {
    Cell(CellKey),
    Range {
        sheet: u32,
        first_row: i32,
        first_column: i32,
        last_row: i32,
        last_column: i32,
    },
}

struct References {
    found: Vec<Reference>,
    volatile: bool,
}

impl References {
    fn of(node: &Node, cell: CellKey) -> References {
        let mut references = References {
            found: Vec::new(),
            volatile: false,
        };
        references.visit(node, cell);
        references
    }

    fn visit_all(&mut self, nodes: &[Node], cell: CellKey) {
        for node in nodes {
            self.visit(node, cell);
        }
    }

    fn visit(&mut self, node: &Node, cell: CellKey) {
        let (_, row, column) = cell;
        let resolve = |absolute: bool, value: i32, origin: i32| {
            if absolute {
                value
            } else {
                value.saturating_add(origin)
            }
        };
        match node {
            Node::ReferenceKind {
                sheet_index,
                absolute_row,
                absolute_column,
                row: target_row,
                column: target_column,
                ..
            } => self.found.push(Reference::Cell((
                *sheet_index,
                resolve(*absolute_row, *target_row, row),
                resolve(*absolute_column, *target_column, column),
            ))),
            Node::RangeKind {
                sheet_index,
                absolute_row1,
                absolute_column1,
                row1,
                column1,
                absolute_row2,
                absolute_column2,
                row2,
                column2,
                ..
            } => {
                let row1 = resolve(*absolute_row1, *row1, row);
                let row2 = resolve(*absolute_row2, *row2, row);
                let column1 = resolve(*absolute_column1, *column1, column);
                let column2 = resolve(*absolute_column2, *column2, column);
                self.found.push(Reference::Range {
                    sheet: *sheet_index,
                    first_row: row1.min(row2),
                    first_column: column1.min(column2),
                    last_row: row1.max(row2),
                    last_column: column1.max(column2),
                });
            }
            // The extent of `A1:INDEX(...)` is only known once evaluated.
            Node::OpRangeKind { left, right } => {
                self.visit(left, cell);
                self.visit(right, cell);
                self.volatile = true;
            }
            Node::OpConcatenateKind { left, right }
            | Node::OpSumKind { left, right, .. }
            | Node::OpProductKind { left, right, .. }
            | Node::OpPowerKind { left, right }
            | Node::CompareKind { left, right, .. } => {
                self.visit(left, cell);
                self.visit(right, cell);
            }
            Node::FunctionKind { kind, args } => {
                let first = self.found.len();
                self.visit_all(args, cell);
                if reads_ranges_in_sum_range_shape(kind) {
                    widen_to_largest_shape(&mut self.found[first..]);
                }
                self.volatile |= is_volatile(kind);
            }
            Node::LambdaDefKind { body, .. } => self.visit(body, cell),
            Node::LambdaCallKind { lambda, args } => {
                self.visit(lambda, cell);
                self.visit_all(args, cell);
            }
            // Names, tables and named functions may point anywhere and can be redefined.
            Node::NamedFunctionKind { args, .. } => {
                self.visit_all(args, cell);
                self.volatile = true;
            }
            Node::DefinedNameKind(_) | Node::TableNameKind(_) => self.volatile = true,
            Node::ImplicitIntersection { child, .. }
            | Node::SpillRangeOperator { child }
            | Node::UnaryKind { right: child, .. } => self.visit(child, cell),
            Node::BooleanKind(_)
            | Node::NumberKind(_)
            | Node::StringKind(_)
            | Node::WrongReferenceKind { .. }
            | Node::WrongRangeKind { .. }
            | Node::ArrayKind(_)
            | Node::NamedVariableKind { .. }
            | Node::ErrorKind(_)
            | Node::ParseErrorKind { .. }
            | Node::EmptyArgKind => {}
        }
    }
}

fn is_volatile(kind: &Function) -> bool {
    matches!(
        kind,
        Function::Now
            | Function::Today
            | Function::Rand
            | Function::Randbetween
            | Function::Randarray
            | Function::Indirect
            | Function::Offset
            | Function::Cell
            | Function::Info
    )
}

// These walk the sum range and read every criteria range at the same offsets, so a
// criteria range smaller than the sum range is read past its end.
fn reads_ranges_in_sum_range_shape(kind: &Function) -> bool {
    matches!(
        kind,
        Function::Sumif
            | Function::Sumifs
            | Function::Averageif
            | Function::Averageifs
            | Function::Countif
            | Function::Countifs
            | Function::Minifs
            | Function::Maxifs
    )
}

fn widen_to_largest_shape(references: &mut [Reference]) {
    let mut height = 0;
    let mut width = 0;
    for reference in references.iter() {
        if let Reference::Range {
            first_row,
            first_column,
            last_row,
            last_column,
            ..
        } = reference
        {
            height = height.max(last_row.saturating_sub(*first_row));
            width = width.max(last_column.saturating_sub(*first_column));
        }
    }
    for reference in references.iter_mut() {
        if let Reference::Range {
            first_row,
            first_column,
            last_row,
            last_column,
            ..
        } = reference
        {
            *last_row = first_row.saturating_add(height);
            *last_column = first_column.saturating_add(width);
        }
    }
}

// Finds the intervals containing a row in logarithmic time plus the number of matches.
struct RowIntervals<T> {
    // Sorted by first row. `max_last_row[mid]` holds the largest last row in the slice
    // that `mid` is the middle of, as in an implicit balanced search tree.
    intervals: Vec<(i32, i32, T)>,
    max_last_row: Vec<i32>,
}

impl<T: Copy + Ord> RowIntervals<T> {
    fn new() -> RowIntervals<T> {
        RowIntervals {
            intervals: Vec::new(),
            max_last_row: Vec::new(),
        }
    }

    fn push(&mut self, first_row: i32, last_row: i32, item: T) {
        self.intervals.push((first_row, last_row, item));
    }

    fn sort(&mut self) {
        self.intervals.sort();
        self.intervals.dedup();
        self.max_last_row.clear();
        self.max_last_row.resize(self.intervals.len(), i32::MIN);
        self.compute_max_last_row(0, self.intervals.len());
    }

    fn shrink_to_fit(&mut self) {
        self.intervals.shrink_to_fit();
        self.max_last_row.shrink_to_fit();
    }

    fn compute_max_last_row(&mut self, start: usize, end: usize) -> i32 {
        if start >= end {
            return i32::MIN;
        }
        let middle = start + (end - start) / 2;
        let left = self.compute_max_last_row(start, middle);
        let right = self.compute_max_last_row(middle + 1, end);
        let max = self.intervals[middle].1.max(left).max(right);
        self.max_last_row[middle] = max;
        max
    }

    fn for_each_containing(&self, row: i32, visit: &mut impl FnMut(T)) {
        self.visit_containing(0, self.intervals.len(), row, visit);
    }

    fn visit_containing(&self, start: usize, end: usize, row: i32, visit: &mut impl FnMut(T)) {
        if start >= end {
            return;
        }
        let middle = start + (end - start) / 2;
        if self.max_last_row[middle] < row {
            return;
        }
        self.visit_containing(start, middle, row, visit);
        let (first_row, last_row, item) = self.intervals[middle];
        if first_row <= row {
            if row <= last_row {
                visit(item);
            }
            self.visit_containing(middle + 1, end, row, visit);
        }
    }
}

struct ColumnDependents {
    // (row, formula), sorted.
    cells: Vec<(i32, CellKey)>,
    ranges: RowIntervals<CellKey>,
}

// Entries of a formula that changed since the index was built stay until the next full
// evaluation rebuilds it: a stale entry only makes a recalculation do needless work.
pub(crate) struct DependencyIndex {
    columns: HashMap<(u32, i32), ColumnDependents>,
    wide_ranges: HashMap<u32, RowIntervals<(i32, i32, CellKey)>>,
    volatile: HashSet<CellKey>,
    unsorted_columns: HashSet<(u32, i32)>,
    unsorted_sheets: HashSet<u32>,
}

impl DependencyIndex {
    pub(crate) fn new() -> DependencyIndex {
        DependencyIndex {
            columns: HashMap::new(),
            wide_ranges: HashMap::new(),
            volatile: HashSet::new(),
            unsorted_columns: HashSet::new(),
            unsorted_sheets: HashSet::new(),
        }
    }

    fn column(&mut self, sheet: u32, column: i32) -> &mut ColumnDependents {
        self.unsorted_columns.insert((sheet, column));
        self.columns
            .entry((sheet, column))
            .or_insert_with(|| ColumnDependents {
                cells: Vec::new(),
                ranges: RowIntervals::new(),
            })
    }

    // Lookups need a `sort` after the last `add`.
    pub(crate) fn add(&mut self, formula: CellKey, node: &Node) {
        let mut references = References::of(node, formula);
        references.found.sort_unstable();
        references.found.dedup();
        if references.volatile {
            self.volatile.insert(formula);
        }
        for reference in references.found {
            match reference {
                Reference::Cell((sheet, row, column)) => {
                    self.column(sheet, column).cells.push((row, formula));
                }
                Reference::Range {
                    sheet,
                    first_row,
                    first_column,
                    last_row,
                    last_column,
                } if last_column.saturating_sub(first_column) < MAX_INDEXED_COLUMNS => {
                    for column in first_column..=last_column {
                        self.column(sheet, column)
                            .ranges
                            .push(first_row, last_row, formula);
                    }
                }
                Reference::Range {
                    sheet,
                    first_row,
                    first_column,
                    last_row,
                    last_column,
                } => {
                    self.unsorted_sheets.insert(sheet);
                    self.wide_ranges
                        .entry(sheet)
                        .or_insert_with(RowIntervals::new)
                        .push(first_row, last_row, (first_column, last_column, formula));
                }
            }
        }
    }

    pub(crate) fn forget_volatile(&mut self, formula: CellKey) {
        self.volatile.remove(&formula);
    }

    // Only once built: shrinking after each edit would reallocate a large column twice.
    pub(crate) fn shrink_to_fit(&mut self) {
        for column in self.columns.values_mut() {
            column.cells.shrink_to_fit();
            column.ranges.shrink_to_fit();
        }
        for ranges in self.wide_ranges.values_mut() {
            ranges.shrink_to_fit();
        }
    }

    pub(crate) fn sort(&mut self) {
        for key in self.unsorted_columns.drain() {
            if let Some(column) = self.columns.get_mut(&key) {
                column.cells.sort();
                column.cells.dedup();
                column.ranges.sort();
            }
        }
        for sheet in self.unsorted_sheets.drain() {
            if let Some(ranges) = self.wide_ranges.get_mut(&sheet) {
                ranges.sort();
            }
        }
    }

    // Returns the entries examined, including wide ranges over other columns of the row.
    fn for_each_dependent(
        &self,
        (sheet, row, column): CellKey,
        visit: &mut impl FnMut(CellKey),
    ) -> usize {
        let mut examined = 0;
        if let Some(dependents) = self.columns.get(&(sheet, column)) {
            let start = dependents.cells.partition_point(|(r, _)| *r < row);
            for (_, formula) in dependents.cells[start..]
                .iter()
                .take_while(|(r, _)| *r == row)
            {
                examined += 1;
                visit(*formula);
            }
            dependents.ranges.for_each_containing(row, &mut |formula| {
                examined += 1;
                visit(formula);
            });
        }
        if let Some(wide) = self.wide_ranges.get(&sheet) {
            wide.for_each_containing(row, &mut |(first_column, last_column, formula)| {
                examined += 1;
                if (first_column..=last_column).contains(&column) {
                    visit(formula);
                }
            });
        }
        examined
    }

    // None when following the dependents would cost more than evaluating everything.
    pub(crate) fn affected_by(&self, edited: &[CellKey]) -> Option<HashSet<CellKey>> {
        let mut affected = HashSet::new();
        let mut pending: Vec<CellKey> = edited.iter().chain(&self.volatile).copied().collect();
        let mut visits = 0usize;
        while let Some(cell) = pending.pop() {
            if !affected.insert(cell) {
                continue;
            }
            visits += self.for_each_dependent(cell, &mut |formula| {
                if !affected.contains(&formula) {
                    pending.push(formula);
                }
            });
            if visits > MAX_DEPENDENT_VISITS {
                return None;
            }
        }
        Some(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::{DependencyIndex, RowIntervals};
    use crate::expressions::parser::Node;
    use crate::functions::Function;

    fn range(row1: i32, row2: i32, absolute: bool) -> Node {
        Node::RangeKind {
            sheet_name: None,
            sheet_index: 0,
            absolute_row1: absolute,
            absolute_column1: absolute,
            row1,
            column1: row1,
            absolute_row2: absolute,
            absolute_column2: absolute,
            row2,
            column2: row2,
        }
    }

    #[test]
    fn extreme_offsets_saturate_instead_of_overflowing() {
        let reference = Node::ReferenceKind {
            sheet_name: None,
            sheet_index: 0,
            absolute_row: false,
            absolute_column: false,
            row: i32::MAX,
            column: i32::MAX,
        };
        let sumif = Node::FunctionKind {
            kind: Function::Sumif,
            args: vec![range(i32::MIN, i32::MAX, true), range(i32::MAX, 1, false)],
        };
        let mut index = DependencyIndex::new();
        index.add((0, i32::MAX, i32::MAX), &reference);
        index.add((0, i32::MAX, i32::MAX), &sumif);
        index.sort();
        assert!(index.affected_by(&[(0, i32::MAX, i32::MAX)]).is_some());
    }

    #[test]
    fn row_intervals_find_exactly_the_intervals_containing_a_row() {
        let spans = [
            (1, 10),
            (2, 2),
            (3, 8),
            (5, 5),
            (1, 1_000),
            (9, 12),
            (20, 30),
        ];
        let mut intervals = RowIntervals::new();
        for (index, (first, last)) in spans.iter().enumerate() {
            intervals.push(*first, *last, index);
        }
        intervals.sort();
        for row in 0..40 {
            let mut found = Vec::new();
            intervals.for_each_containing(row, &mut |index| found.push(index));
            found.sort();
            let expected: Vec<usize> = (0..spans.len())
                .filter(|index| (spans[*index].0..=spans[*index].1).contains(&row))
                .collect();
            assert_eq!(found, expected, "row {row}");
        }
    }
}
