use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ironcalc::base::cell_store::CellStore;
use ironcalc::base::types::Cell;
use ironcalc::base::{Recalculation, UserModel};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

use crate::workbook::{col_i32, row_i32};

// IronCalc computes a sheet's dimension by walking every cell, which on a large sheet
// is too slow to repeat on every scroll or keystroke. Any mutable access to the model
// goes through `deref_mut`, which drops the cached ends, so they can never go stale;
// only the methods below that know an edit's reach keep them.
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
    cell.dynamic_range().is_some()
}

fn position(row: i32, col: i32) -> CellPos {
    CellPos::new(
        RowIdx::clamped(i64::from(row) - 1),
        ColIdx::clamped(i64::from(col) - 1),
    )
}

// Matches IronCalc's `Worksheet::dimension`: rows without cells still count, but a sheet
// without any cell ends at A1.
fn extent_of(sheet_data: &CellStore) -> Extent {
    let mut last_row = None;
    let mut last_col = None;
    let mut spills = false;
    for row in sheet_data.rows() {
        last_row = last_row.max(Some(row));
    }
    for (_, col, cell) in sheet_data.iter() {
        last_col = last_col.max(Some(col));
        spills |= is_dynamic(cell);
    }
    let last = last_row.zip(last_col).map(|(row, col)| position(row, col));
    Extent { last, spills }
}

// Visits the cells of `sheet_data` inside `area`, walking whichever side is smaller.
fn for_each_within(sheet_data: &CellStore, area: Range, mut visit: impl FnMut(i32, i32, &Cell)) {
    let rows = row_i32(area.start.row)..=row_i32(area.end.row);
    let cols = col_i32(area.start.col)..=col_i32(area.end.col);
    let mut in_row = |row: i32| {
        if usize::from(area.cols()) < sheet_data.row_len(row) {
            for col in cols.clone() {
                if let Some(cell) = sheet_data.get(row, col) {
                    visit(row, col, cell);
                }
            }
        } else {
            for (col, cell) in sheet_data
                .row_cells(row)
                .filter(|(col, _)| cols.contains(col))
            {
                visit(row, col, cell);
            }
        }
    };
    if usize::try_from(area.rows()).is_ok_and(|rows| rows < sheet_data.row_count()) {
        for row in rows {
            in_row(row);
        }
    } else {
        for row in sheet_data.rows().filter(|row| rows.contains(row)) {
            in_row(row);
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

    // For edits that write only inside `area` of `sheet` and remove no cell; `edit` must
    // leave the workbook evaluated. The cached end grows to the cells now there instead of
    // walking the sheet again.
    pub(crate) fn write_within<T>(
        &mut self,
        sheet: SheetId,
        area: Range,
        edit: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        let result = edit(&mut self.model);
        self.keep_extents(&[(sheet, area)]);
        result
    }

    // For edits that change the values of existing cells only, adding or removing none.
    pub(crate) fn rewrite_values<T>(
        &mut self,
        edit: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        let result = edit(&mut self.model);
        self.keep_extents(&[]);
        result
    }

    // Evaluation adds or removes cells only on sheets holding a dynamic array, and only in
    // a full recalculation: an incremental one falls back to a full one whenever a spill
    // could grow, shrink or move. Every other sheet keeps its end however large it is.
    fn keep_extents(&mut self, written: &[(SheetId, Range)]) {
        let spills_unchanged =
            self.model.get_model().last_recalculation() == Recalculation::Incremental;
        let cached = std::mem::take(self.cache_mut());
        let kept: HashMap<SheetId, Extent> = cached
            .into_iter()
            .filter(|(_, extent)| spills_unchanged || !extent.spills)
            .filter_map(|(sheet, extent)| {
                let areas: Vec<Range> = written
                    .iter()
                    .filter(|(written_sheet, _)| *written_sheet == sheet)
                    .map(|(_, area)| *area)
                    .collect();
                if areas.is_empty() {
                    return Some((sheet, extent));
                }
                self.grown(sheet, &areas, extent)
                    .map(|grown| (sheet, grown))
            })
            .collect();
        *self.cache_mut() = kept;
    }

    // `None` when the sheet must be walked again: a sheet that had no cells may still hold
    // empty rows, which the walk counts, and a new dynamic array spills past its area.
    fn grown(&self, sheet: SheetId, areas: &[Range], extent: Extent) -> Option<Extent> {
        let last = extent.last?;
        let ws = self.model.get_model().workbook.worksheet(sheet.0).ok()?;
        let mut end = last;
        let mut dynamic = false;
        for area in areas {
            for_each_within(&ws.sheet_data, *area, |row, col, cell| {
                let at = position(row, col);
                end = CellPos::new(end.row.max(at.row), end.col.max(at.col));
                dynamic |= is_dynamic(cell);
            });
        }
        if dynamic && !extent.spills {
            return None;
        }
        Some(Extent {
            last: Some(end),
            spills: extent.spills,
        })
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

    fn type_at(model: &mut CachedModel, sheet: u32, at: &str, text: &str) {
        let pos = CellPos::parse_a1(at).unwrap();
        model.write_within(SheetId(sheet), Range::single(pos), |model| {
            model
                .set_user_input(sheet, row_i32(pos.row), col_i32(pos.col), text)
                .unwrap();
        });
    }

    fn last_recalculation(model: &CachedModel) -> Recalculation {
        model.get_model().last_recalculation()
    }

    fn two_filled_sheets() -> CachedModel {
        let mut model = model();
        model.new_sheet().unwrap();
        model.set_user_input(0, 1, 1, "1").unwrap();
        model.set_user_input(1, 1, 1, "1").unwrap();
        model.used_end(SheetId(0));
        model.used_end(SheetId(1));
        model
    }

    #[test]
    fn an_edit_keeps_the_ends_of_the_other_sheets() {
        let mut model = two_filled_sheets();
        type_at(&mut model, 0, "A1", "2");
        assert_eq!(model.cache().len(), 2);
    }

    fn spill_on_the_second_sheet() -> CachedModel {
        let mut model = two_filled_sheets();
        type_at(&mut model, 1, "A1", "=SEQUENCE(3)");
        assert_eq!(last_recalculation(&model), Recalculation::Full);
        assert!(!model.cache().contains_key(&SheetId(1)));
        assert!(model.cache().contains_key(&SheetId(0)));
        assert_eq!(model.used_end(SheetId(1)), CellPos::parse_a1("A3").unwrap());
        model
    }

    #[test]
    fn an_incremental_edit_keeps_the_end_of_a_sheet_with_a_spill() {
        let mut model = spill_on_the_second_sheet();
        type_at(&mut model, 0, "A1", "2");
        assert_eq!(last_recalculation(&model), Recalculation::Incremental);
        assert!(model.cache().contains_key(&SheetId(1)));
        assert_eq!(
            Some(model.used_end(SheetId(1))),
            model.walked_end(SheetId(1))
        );
    }

    #[test]
    fn a_sheet_with_a_spill_grows_by_an_incremental_write_and_drops_on_a_full_one() {
        let mut model = spill_on_the_second_sheet();
        type_at(&mut model, 1, "D7", "5");
        assert_eq!(last_recalculation(&model), Recalculation::Incremental);
        let grown = model.cache().get(&SheetId(1)).copied().unwrap();
        assert_eq!(grown.last, CellPos::parse_a1("D7"));
        assert!(grown.spills);
        type_at(&mut model, 0, "B1", "=SEQUENCE(2)");
        assert_eq!(last_recalculation(&model), Recalculation::Full);
        assert!(!model.cache().contains_key(&SheetId(0)));
        assert!(!model.cache().contains_key(&SheetId(1)));
        assert_eq!(
            Some(model.used_end(SheetId(0))),
            model.walked_end(SheetId(0))
        );
        assert_eq!(
            Some(model.used_end(SheetId(1))),
            model.walked_end(SheetId(1))
        );
    }

    #[test]
    fn failed_lookup_is_not_cached() {
        let model = model();
        assert_eq!(model.used_end(SheetId(9)), CellPos::default());
        assert!(model.cache().is_empty());
    }
}
