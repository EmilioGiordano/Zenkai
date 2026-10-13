use std::collections::HashMap;

use bitcode::{Decode, Encode};

use crate::types::Cell;

/// Cells of a worksheet, addressed by (row, column). It is row first, like the xlsx format.
/// A row can exist with no cells: the importer keeps the rows of the file, and the exporter
/// writes them back.
#[derive(Encode, Decode, Debug, PartialEq, Clone, Default)]
pub struct CellStore {
    rows: HashMap<i32, HashMap<i32, Cell>>,
}

impl CellStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, row: i32, column: i32) -> Option<&Cell> {
        self.rows.get(&row)?.get(&column)
    }

    pub fn get_mut(&mut self, row: i32, column: i32) -> Option<&mut Cell> {
        self.rows.get_mut(&row)?.get_mut(&column)
    }

    /// Stores the cell, creating its row if needed, and returns the cell it replaces.
    pub fn insert(&mut self, row: i32, column: i32, cell: Cell) -> Option<Cell> {
        self.rows.entry(row).or_default().insert(column, cell)
    }

    /// Removes the cell. A row left without cells is removed too.
    pub fn remove(&mut self, row: i32, column: i32) -> Option<Cell> {
        let cells = self.rows.get_mut(&row)?;
        let removed = cells.remove(&column);
        if cells.is_empty() {
            self.rows.remove(&row);
        }
        removed
    }

    /// Replaces the whole row, keeping it even when `cells` is empty.
    pub fn insert_row(&mut self, row: i32, cells: impl IntoIterator<Item = (i32, Cell)>) {
        self.rows.insert(row, cells.into_iter().collect());
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

    /// Cells of one row in no particular order; empty when the row does not exist.
    pub fn row_cells(&self, row: i32) -> impl Iterator<Item = (i32, &Cell)> + '_ {
        self.rows
            .get(&row)
            .into_iter()
            .flat_map(|cells| cells.iter().map(|(column, cell)| (*column, cell)))
    }

    pub fn row_cells_sorted(&self, row: i32) -> Vec<(i32, &Cell)> {
        let mut cells: Vec<(i32, &Cell)> = self.row_cells(row).collect();
        cells.sort_unstable_by_key(|(column, _)| *column);
        cells
    }

    pub fn row_columns(&self, row: i32) -> impl Iterator<Item = i32> + '_ {
        self.row_cells(row).map(|(column, _)| column)
    }

    pub fn row_columns_sorted(&self, row: i32) -> Vec<i32> {
        let mut columns: Vec<i32> = self.row_columns(row).collect();
        columns.sort_unstable();
        columns
    }

    pub fn row_len(&self, row: i32) -> usize {
        self.rows.get(&row).map_or(0, HashMap::len)
    }

    /// Every cell as (row, column, cell), in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32, &Cell)> + '_ {
        self.rows.iter().flat_map(|(row, cells)| {
            cells
                .iter()
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
    fn the_encoding_matches_the_nested_maps_it_replaced() {
        let mut store = CellStore::new();
        store.insert(1, 1, cell(3));
        let mut nested: HashMap<i32, HashMap<i32, Cell>> = HashMap::new();
        nested.entry(1).or_default().insert(1, cell(3));
        assert_eq!(bitcode::encode(&store), bitcode::encode(&nested));
        let decoded: CellStore = bitcode::decode(&bitcode::encode(&nested)).unwrap();
        assert_eq!(decoded, store);
    }
}
