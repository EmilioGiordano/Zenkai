use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ironcalc::base::UserModel;
use ironcalc::base::types::{ArrayKind, Cell, SheetData};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

use crate::workbook::{col_i32, row_i32};

// IronCalc computes a sheet's dimension by walking every cell, which on a large sheet
// is too slow to repeat on every scroll or keystroke. Any mutable access to the model
// goes through `deref_mut`, which drops the cached ends, so they can never go stale;
// only `write_within` and `rewrite_values` keep them, for edits whose reach is known.
pub struct CachedModel {
    model: UserModel<'static>,
    extents: Mutex<HashMap<SheetId, Extent>>,
}

#[derive(Clone, Copy)]
struct Extent {
    // `None` for a sheet without cells, which reads as ending at A1.
    last: Option<CellPos>,
    // A dynamic array formula spills on evaluation, adding or removing cells wherever its
    // result reaches, even after an edit on another sheet.
    spills: bool,
}

fn is_dynamic(cell: &Cell) -> bool {
    matches!(
        cell,
        Cell::ArrayFormula {
            kind: ArrayKind::Dynamic,
            ..
        }
    )
}

fn position(row: i32, col: i32) -> CellPos {
    CellPos::new(
        RowIdx::clamped(i64::from(row) - 1),
        ColIdx::clamped(i64::from(col) - 1),
    )
}

// Matches IronCalc's `Worksheet::dimension`: rows without cells still count, but a sheet
// without any cell ends at A1.
fn extent_of(sheet_data: &SheetData) -> Extent {
    let mut last_row = None;
    let mut last_col = None;
    let mut spills = false;
    for (row, columns) in sheet_data {
        last_row = last_row.max(Some(*row));
        for (col, cell) in columns {
            last_col = last_col.max(Some(*col));
            spills |= is_dynamic(cell);
        }
    }
    let last = last_row.zip(last_col).map(|(row, col)| position(row, col));
    Extent { last, spills }
}

// Visits the cells of `sheet_data` inside `area`, walking whichever side is smaller.
fn for_each_within(sheet_data: &SheetData, area: Range, mut visit: impl FnMut(i32, i32, &Cell)) {
    let rows = row_i32(area.start.row)..=row_i32(area.end.row);
    let cols = col_i32(area.start.col)..=col_i32(area.end.col);
    let mut in_row = |row: i32, columns: &HashMap<i32, Cell>| {
        if usize::from(area.cols()) < columns.len() {
            for col in cols.clone() {
                if let Some(cell) = columns.get(&col) {
                    visit(row, col, cell);
                }
            }
        } else {
            for (col, cell) in columns.iter().filter(|(col, _)| cols.contains(*col)) {
                visit(row, *col, cell);
            }
        }
    };
    if usize::try_from(area.rows()).is_ok_and(|rows| rows < sheet_data.len()) {
        for row in rows {
            if let Some(columns) = sheet_data.get(&row) {
                in_row(row, columns);
            }
        }
    } else {
        for (row, columns) in sheet_data {
            if rows.contains(row) {
                in_row(*row, columns);
            }
        }
    }
}

impl CachedModel {
    pub fn new(model: UserModel<'static>) -> CachedModel {
        CachedModel {
            model,
            extents: Mutex::new(HashMap::new()),
        }
    }

    // The cache only holds derived values, so a lock poisoned by a panicked reader
    // still holds valid entries.
    fn cache(&self) -> MutexGuard<'_, HashMap<SheetId, Extent>> {
        self.extents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cache_mut(&mut self) -> &mut HashMap<SheetId, Extent> {
        self.extents
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn used_end(&self, sheet: SheetId) -> CellPos {
        if let Some(extent) = self.cache().get(&sheet) {
            return extent.last.unwrap_or_default();
        }
        let extent = match self.model.get_model().workbook.worksheet(sheet.0) {
            Ok(ws) => extent_of(&ws.sheet_data),
            Err(error) => {
                tracing::warn!(sheet = sheet.0, %error, "could not read the sheet dimension");
                return CellPos::default();
            }
        };
        self.cache().insert(sheet, extent);
        extent.last.unwrap_or_default()
    }

    // Every sheet is cached and none holds a dynamic array, so evaluation adds or removes
    // no cell and an edit changes only the cells it writes.
    fn evaluation_keeps_cells(&mut self) -> bool {
        let sheets = self.model.get_model().workbook.worksheets.len();
        let cache = self.cache_mut();
        cache.len() == sheets && cache.values().all(|extent| !extent.spills)
    }

    // For edits that write only inside `area` of `sheet` and remove no cell: the cached
    // end grows to the cells now there instead of walking the sheet again.
    pub(crate) fn write_within<T>(
        &mut self,
        sheet: SheetId,
        area: Range,
        edit: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        let kept = self.evaluation_keeps_cells();
        let result = edit(&mut self.model);
        let grown = match self.cache().get(&sheet).copied() {
            Some(Extent {
                last: Some(last), ..
            }) if kept => self.grown(sheet, area, last),
            _ => None,
        };
        let cache = self.cache_mut();
        match grown {
            Some(extent) if !extent.spills => {
                cache.insert(sheet, extent);
            }
            // A sheet that had no cells may still hold empty rows, which the walk counts.
            None if kept => {
                cache.remove(&sheet);
            }
            _ => cache.clear(),
        }
        result
    }

    fn grown(&self, sheet: SheetId, area: Range, last: CellPos) -> Option<Extent> {
        let ws = self.model.get_model().workbook.worksheet(sheet.0).ok()?;
        let mut end = last;
        let mut spills = false;
        for_each_within(&ws.sheet_data, area, |row, col, cell| {
            let at = position(row, col);
            end = CellPos::new(end.row.max(at.row), end.col.max(at.col));
            spills |= is_dynamic(cell);
        });
        Some(Extent {
            last: Some(end),
            spills,
        })
    }

    // For edits that change the values of existing cells only, adding or removing none.
    pub(crate) fn rewrite_values<T>(
        &mut self,
        edit: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        let kept = self.evaluation_keeps_cells();
        let result = edit(&mut self.model);
        if !kept {
            self.cache_mut().clear();
        }
        result
    }

    // Only for access that never changes cell content, such as moving the selection
    // to copy; it keeps the cached ends so the next read does not walk the sheet.
    pub(crate) fn select_without_invalidation<T>(
        &mut self,
        access: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        access(&mut self.model)
    }

    #[cfg(test)]
    pub(crate) fn walked_end(&self, sheet: SheetId) -> Option<CellPos> {
        let ws = self.model.get_model().workbook.worksheet(sheet.0).ok()?;
        let dimension = ws.dimension();
        Some(position(dimension.max_row, dimension.max_column))
    }
}

impl Deref for CachedModel {
    type Target = UserModel<'static>;

    fn deref(&self) -> &UserModel<'static> {
        &self.model
    }
}

impl DerefMut for CachedModel {
    fn deref_mut(&mut self) -> &mut UserModel<'static> {
        self.cache_mut().clear();
        &mut self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> CachedModel {
        CachedModel::new(UserModel::new_empty("Book1", "en", "UTC", "en").unwrap())
    }

    #[test]
    fn mutable_access_drops_the_cache_but_invalidation_free_access_keeps_it() {
        let mut model = model();
        model.used_end(SheetId(0));
        assert_eq!(model.cache().len(), 1);
        model.select_without_invalidation(|_| ());
        assert_eq!(model.cache().len(), 1);
        let _ = &mut *model;
        assert!(model.cache().is_empty());
    }

    #[test]
    fn failed_lookup_is_not_cached() {
        let model = model();
        assert_eq!(model.used_end(SheetId(9)), CellPos::default());
        assert!(model.cache().is_empty());
    }
}
