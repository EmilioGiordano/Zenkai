use std::collections::HashMap;

use bitcode::{Decode, Encode};

use crate::types::Cell;

/// Cells of one row: `columns` is strictly increasing and `cells[i]` sits at `columns[i]`.
#[derive(Encode, Decode, Debug, PartialEq, Clone, Default)]
struct Row {
    columns: Vec<i32>,
    cells: Vec<Cell>,
}

impl Row {
    fn from_unsorted(cells: impl IntoIterator<Item = (i32, Cell)>) -> Self {
        let mut pairs: Vec<(i32, Cell)> = cells.into_iter().collect();
        // A stable sort keeps the last duplicate last, which wins as in a map.
        pairs.sort_by_key(|(column, _)| *column);
        let mut row = Row::default();
        for (column, cell) in pairs {
            if row.columns.last() == Some(&column) {
                if let Some(slot) = row.cells.last_mut() {
                    *slot = cell;
                }
            } else {
                row.columns.push(column);
                row.cells.push(cell);
            }
        }
        row.columns.shrink_to_fit();
        row.cells.shrink_to_fit();
        row
    }

    fn position(&self, column: i32) -> Result<usize, usize> {
        // Imports append in column order, so the last column is the common case.
        match self.columns.last() {
            Some(last) if *last < column => Err(self.columns.len()),
            _ => self.columns.binary_search(&column),
        }
    }
}

/// Cells of a worksheet, addressed by (row, column). It is row first, like the xlsx format.
/// A row can exist with no cells: the importer keeps the rows of the file, and the exporter
/// writes them back.
/// Each row keeps its cells sorted by column in two parallel vectors, so a stored cell costs
/// the size of a `Cell` plus four bytes.
// The encoding differs from the nested maps of earlier builds; nothing persists those bytes yet.
#[derive(Encode, Decode, Debug, PartialEq, Clone, Default)]
pub struct CellStore {
    rows: HashMap<i32, Row>,
}

impl CellStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, row: i32, column: i32) -> Option<&Cell> {
        let row = self.rows.get(&row)?;
        row.position(column).ok().map(|index| &row.cells[index])
    }

    pub fn get_mut(&mut self, row: i32, column: i32) -> Option<&mut Cell> {
        let row = self.rows.get_mut(&row)?;
        let index = row.position(column).ok()?;
        Some(&mut row.cells[index])
    }

    pub fn insert(&mut self, row: i32, column: i32, cell: Cell) -> Option<Cell> {
        let row = self.rows.entry(row).or_default();
        match row.position(column) {
            Ok(index) => Some(std::mem::replace(&mut row.cells[index], cell)),
            Err(index) => {
                row.columns.insert(index, column);
                row.cells.insert(index, cell);
                None
            }
        }
    }

    /// Removes the cell. A row left without cells is removed too.
    pub fn remove(&mut self, row: i32, column: i32) -> Option<Cell> {
        let cells = self.rows.get_mut(&row)?;
        let index = cells.position(column).ok()?;
        cells.columns.remove(index);
        let removed = cells.cells.remove(index);
        if cells.columns.is_empty() {
            self.rows.remove(&row);
        }
        Some(removed)
    }

    /// Removes the cells from `column` on and returns them by column. The row is kept even
    /// when it ends up empty.
    pub fn take_from_column(&mut self, row: i32, column: i32) -> Vec<(i32, Cell)> {
        let Some(cells) = self.rows.get_mut(&row) else {
            return Vec::new();
        };
        let at = cells.columns.partition_point(|existing| *existing < column);
        let columns = cells.columns.split_off(at);
        let taken = cells.cells.split_off(at);
        columns.into_iter().zip(taken).collect()
    }

    /// Replaces the whole row, keeping it even when `cells` is empty.
    pub fn insert_row(&mut self, row: i32, cells: impl IntoIterator<Item = (i32, Cell)>) {
        self.rows.insert(row, Row::from_unsorted(cells));
    }

    pub fn remove_row(&mut self, row: i32) {
        self.rows.remove(&row);
    }

    pub fn has_row(&self, row: i32) -> bool {
        self.rows.contains_key(&row)
    }

    /// Number of rows, including rows with no cells.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Row numbers in no particular order.
    pub fn rows(&self) -> impl Iterator<Item = i32> + '_ {
        self.rows.keys().copied()
    }

    pub fn rows_sorted(&self) -> Vec<i32> {
        let mut rows: Vec<i32> = self.rows().collect();
        rows.sort_unstable();
        rows
    }

    /// Cells of one row by column; empty when the row does not exist.
    pub fn row_cells(&self, row: i32) -> impl Iterator<Item = (i32, &Cell)> + '_ {
        self.rows
            .get(&row)
            .into_iter()
            .flat_map(|cells| cells.columns.iter().copied().zip(cells.cells.iter()))
    }

    pub fn row_cells_sorted(&self, row: i32) -> Vec<(i32, &Cell)> {
        self.row_cells(row).collect()
    }

    pub fn row_columns(&self, row: i32) -> impl Iterator<Item = i32> + '_ {
        self.rows
            .get(&row)
            .into_iter()
            .flat_map(|cells| cells.columns.iter().copied())
    }

    pub fn row_columns_sorted(&self, row: i32) -> Vec<i32> {
        self.row_columns(row).collect()
    }

    pub fn row_len(&self, row: i32) -> usize {
        self.rows.get(&row).map_or(0, |cells| cells.columns.len())
    }

    /// Every cell as (row, column, cell), in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32, &Cell)> + '_ {
        self.rows.iter().flat_map(|(row, cells)| {
            cells
                .columns
                .iter()
                .zip(cells.cells.iter())
                .map(move |(column, cell)| (*row, *column, cell))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(s: i32) -> Cell {
        Cell::EmptyCell { s }
    }

    #[test]
    fn insert_get_replace_and_remove() {
        let mut store = CellStore::new();
        assert!(store.is_empty());
        assert_eq!(store.insert(2, 3, cell(1)), None);
        assert_eq!(store.insert(2, 3, cell(2)), Some(cell(1)));
        assert_eq!(store.get(2, 3), Some(&cell(2)));
        assert_eq!(store.get(2, 4), None);
        assert_eq!(store.get(3, 3), None);
        if let Some(found) = store.get_mut(2, 3) {
            *found = cell(5);
        }
        assert_eq!(store.get(2, 3), Some(&cell(5)));
        assert_eq!(store.remove(2, 4), None);
        assert_eq!(store.remove(2, 3), Some(cell(5)));
        assert!(store.is_empty());
    }

    #[test]
    fn removing_the_last_cell_removes_the_row() {
        let mut store = CellStore::new();
        store.insert(1, 1, cell(0));
        store.insert(1, 2, cell(0));
        store.remove(1, 1);
        assert!(store.has_row(1));
        store.remove(1, 2);
        assert!(!store.has_row(1));
        assert_eq!(store.row_count(), 0);
    }

    #[test]
    fn an_inserted_empty_row_is_kept() {
        let mut store = CellStore::new();
        store.insert_row(7, []);
        assert!(store.has_row(7));
        assert_eq!(store.row_count(), 1);
        assert_eq!(store.row_len(7), 0);
        assert_eq!(store.iter().count(), 0);
        store.remove_row(7);
        assert!(!store.has_row(7));
    }

    #[test]
    fn take_from_column_keeps_the_row_and_the_earlier_cells() {
        let mut store = CellStore::new();
        for column in [1, 3, 5, 7] {
            store.insert(2, column, cell(column));
        }
        let taken = store.take_from_column(2, 4);
        assert_eq!(taken, vec![(5, cell(5)), (7, cell(7))]);
        assert_eq!(store.row_columns_sorted(2), vec![1, 3]);
        assert_eq!(store.take_from_column(2, 1).len(), 2);
        assert!(store.has_row(2));
        assert_eq!(store.row_len(2), 0);
        assert!(store.take_from_column(9, 1).is_empty());
    }

    #[test]
    fn insert_row_replaces_the_previous_row() {
        let mut store = CellStore::new();
        store.insert(4, 1, cell(1));
        store.insert_row(4, [(2, cell(2)), (3, cell(3))]);
        assert_eq!(store.get(4, 1), None);
        assert_eq!(store.row_columns_sorted(4), vec![2, 3]);
    }

    #[test]
    fn rows_and_cells_come_back_sorted_on_request() {
        let mut store = CellStore::new();
        for (row, column) in [(9, 2), (1, 5), (9, 1), (4, 4), (1, 1)] {
            store.insert(row, column, cell(row * 10 + column));
        }
        assert_eq!(store.rows_sorted(), vec![1, 4, 9]);
        assert_eq!(store.row_columns_sorted(9), vec![1, 2]);
        assert_eq!(
            store.row_cells_sorted(1),
            vec![(1, &cell(11)), (5, &cell(15))]
        );
        assert_eq!(store.row_cells(8).count(), 0);
        assert_eq!(store.row_columns(8).count(), 0);
        assert_eq!(store.row_len(9), 2);
    }

    #[test]
    fn iter_visits_every_cell_once() {
        let mut store = CellStore::new();
        store.insert(1, 1, cell(0));
        store.insert(1, 2, cell(0));
        store.insert(3, 1, cell(0));
        let mut seen: Vec<(i32, i32)> =
            store.iter().map(|(row, column, _)| (row, column)).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![(1, 1), (1, 2), (3, 1)]);
        assert_eq!(store.rows().count(), 2);
    }

    #[test]
    fn the_encoding_round_trips() {
        let mut store = CellStore::new();
        store.insert(1, 1, cell(3));
        store.insert(1, 9, cell(4));
        store.insert_row(5, []);
        let decoded: CellStore = bitcode::decode(&bitcode::encode(&store)).unwrap();
        assert_eq!(decoded, store);
    }

    #[test]
    fn a_cell_is_32_bytes() {
        assert_eq!(std::mem::size_of::<Cell>(), 32);
    }

    #[test]
    fn boxed_cell_payloads_encode_like_inline_fields() {
        use crate::expressions::token::Error;
        use crate::types::{ArrayKind, FormulaValue, SpillValue};

        // Only encoded, never read: the fields exist to be serialized.
        #[derive(bitcode::Encode)]
        #[allow(dead_code)]
        enum InlineCell {
            EmptyCell {
                s: i32,
            },
            BooleanCell {
                v: bool,
                s: i32,
            },
            NumberCell {
                v: f64,
                s: i32,
            },
            ErrorCell {
                ei: Error,
                s: i32,
            },
            SharedString {
                si: i32,
                s: i32,
            },
            CellFormula {
                f: i32,
                s: i32,
                v: FormulaValue,
            },
            ArrayFormula {
                f: i32,
                s: i32,
                r: (i32, i32),
                kind: ArrayKind,
                v: FormulaValue,
            },
            SpillCell {
                s: i32,
                a: (i32, i32),
                v: SpillValue,
            },
        }

        let array =
            Cell::new_array_formula(3, 4, (2, 5), ArrayKind::Dynamic, FormulaValue::Number(1.5));
        let inline = InlineCell::ArrayFormula {
            f: 3,
            s: 4,
            r: (2, 5),
            kind: ArrayKind::Dynamic,
            v: FormulaValue::Number(1.5),
        };
        assert_eq!(bitcode::encode(&array), bitcode::encode(&inline));
        let spill = Cell::new_spill_cell(7, (1, 2), SpillValue::Number(2.5));
        let inline = InlineCell::SpillCell {
            s: 7,
            a: (1, 2),
            v: SpillValue::Number(2.5),
        };
        assert_eq!(bitcode::encode(&spill), bitcode::encode(&inline));
    }

    type Oracle = HashMap<i32, HashMap<i32, Cell>>;

    fn assert_same(store: &CellStore, oracle: &Oracle) {
        assert_eq!(store.row_count(), oracle.len());
        for (row, cells) in oracle {
            assert!(store.has_row(*row));
            assert_eq!(store.row_len(*row), cells.len());
            for (column, expected) in cells {
                assert_eq!(store.get(*row, *column), Some(expected));
            }
            let columns = store.row_columns_sorted(*row);
            assert!(columns.windows(2).all(|pair| pair[0] < pair[1]));
        }
        assert_eq!(
            store.iter().count(),
            oracle.values().map(HashMap::len).sum::<usize>()
        );
    }

    #[test]
    fn random_operations_agree_with_nested_maps() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound) as i32
        };
        for _ in 0..20 {
            let mut store = CellStore::new();
            let mut oracle = Oracle::new();
            for step in 0..2000 {
                let row = next(6);
                let column = next(12);
                let value = cell(next(1000));
                match next(7) {
                    0..=2 => {
                        let got = store.insert(row, column, value.clone());
                        let expected = oracle.entry(row).or_default().insert(column, value);
                        assert_eq!(got, expected);
                    }
                    3 => {
                        let got = store.remove(row, column);
                        let expected = oracle.get_mut(&row).and_then(|cells| cells.remove(&column));
                        if expected.is_some() && oracle.get(&row).is_some_and(HashMap::is_empty) {
                            oracle.remove(&row);
                        }
                        assert_eq!(got, expected);
                    }
                    4 => {
                        let cells: Vec<(i32, Cell)> =
                            (0..next(5)).map(|_| (next(12), cell(next(1000)))).collect();
                        store.insert_row(row, cells.clone());
                        oracle.insert(row, cells.into_iter().collect());
                    }
                    5 => {
                        store.remove_row(row);
                        oracle.remove(&row);
                    }
                    _ => {
                        let expected = oracle.get(&row).and_then(|cells| cells.get(&column));
                        assert_eq!(store.get(row, column), expected);
                        if let Some(found) = store.get_mut(row, column) {
                            *found = cell(-1);
                            oracle.entry(row).or_default().insert(column, cell(-1));
                        }
                    }
                }
                if step % 50 == 0 {
                    assert_same(&store, &oracle);
                    let decoded: CellStore = bitcode::decode(&bitcode::encode(&store)).unwrap();
                    assert_eq!(decoded, store);
                }
            }
            assert_same(&store, &oracle);
        }
    }
}
