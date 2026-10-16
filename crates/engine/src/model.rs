use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ironcalc::base::cell_store::CellStore;
use ironcalc::base::expressions::types::Area;
use ironcalc::base::types::Cell;
use ironcalc::base::{StepCells, UserModel};
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
    // result reaches, even after an edit on another sheet. IronCalc stores a formula that
    // may return an array as a dynamic anchor as soon as it is entered, so a sheet only
    // gains one where a cell is written.
    spills: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum HistoryStep {
    Undo,
    Redo,
}

pub(crate) type HistoryTake = fn(&mut UserModel<'static>) -> Result<(), String>;

// What an edit may do to the cells inside the areas it reports.
#[derive(Clone, Copy)]
enum CellChange {
    AddOnly,
    AddOrRemove,
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

fn step_area(area: &Area) -> Option<(SheetId, Range)> {
    if area.width < 1 || area.height < 1 {
        return None;
    }
    let end = position(
        area.row.saturating_add(area.height - 1),
        area.column.saturating_add(area.width - 1),
    );
    Some((
        SheetId(area.sheet),
        Range::new(position(area.row, area.column), end),
    ))
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

    // For edits that write only inside `area` of `sheet` and remove no cell, such as typing
    // or styling a range. The cached end grows to the cells now there instead of walking
    // the sheet again.
    pub(crate) fn write_within<T>(
        &mut self,
        sheet: SheetId,
        area: Range,
        edit: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        let evaluations = self.full_evaluations();
        let result = edit(&mut self.model);
        self.keep_extents(&[(sheet, area)], CellChange::AddOnly, &[], evaluations);
        result
    }

    // For edits that add or remove no cell: clearing values, row and column sizes, hidden
    // lines, frozen panes.
    pub(crate) fn edit_keeping_cells<T>(
        &mut self,
        edit: impl FnOnce(&mut UserModel<'static>) -> T,
    ) -> T {
        let evaluations = self.full_evaluations();
        let result = edit(&mut self.model);
        self.keep_extents(&[], CellChange::AddOnly, &[], evaluations);
        result
    }

    // `run` gets the model and the undo or redo to take on it, once, so a caller can wrap
    // the step, for instance to time it. The cells it changes are read before it is taken;
    // a step that failed may have applied in part, so it drops every end.
    pub(crate) fn step<T>(
        &mut self,
        step: HistoryStep,
        run: impl FnOnce(&mut UserModel<'static>, HistoryTake) -> Result<T, String>,
    ) -> Result<T, String> {
        let (reach, take) = match step {
            HistoryStep::Undo => (self.model.undo_cells(), UserModel::undo as HistoryTake),
            HistoryStep::Redo => (self.model.redo_cells(), UserModel::redo as HistoryTake),
        };
        let evaluations = self.full_evaluations();
        let result = run(&mut self.model, take);
        match (reach, &result) {
            (Some(StepCells { areas, sheets }), Ok(_)) => {
                let changed: Vec<(SheetId, Range)> = areas.iter().filter_map(step_area).collect();
                let rebuilt: Vec<SheetId> = sheets.into_iter().map(SheetId).collect();
                self.keep_extents(&changed, CellChange::AddOrRemove, &rebuilt, evaluations);
            }
            _ => self.cache_mut().clear(),
        }
        result
    }

    fn full_evaluations(&self) -> u64 {
        self.model.get_model().full_evaluations()
    }

    // Evaluation adds or removes cells only on sheets holding a dynamic array, and only in
    // a full evaluation: an incremental one falls back to a full one whenever a spill
    // could grow, shrink or move. An edit that evaluated nothing in full, `evaluations`
    // being the count before it, left spills alone; no edit may leave evaluation paused.
    // Every other sheet keeps its end unless the edit changed cells of it or, for
    // `rebuilt`, moved its rows or columns.
    fn keep_extents(
        &mut self,
        changed: &[(SheetId, Range)],
        change: CellChange,
        rebuilt: &[SheetId],
        evaluations: u64,
    ) {
        let spills_unchanged = self.full_evaluations() == evaluations;
        let cached = std::mem::take(self.cache_mut());
        let kept: HashMap<SheetId, Extent> = cached
            .into_iter()
            .filter(|(sheet, extent)| {
                (spills_unchanged || !extent.spills) && !rebuilt.contains(sheet)
            })
            .filter_map(|(sheet, extent)| {
                let areas: Vec<Range> = changed
                    .iter()
                    .filter(|(changed_sheet, _)| *changed_sheet == sheet)
                    .map(|(_, area)| *area)
                    .collect();
                if areas.is_empty() {
                    return Some((sheet, extent));
                }
                let kept = match change {
                    CellChange::AddOnly => self.grown(sheet, &areas, extent),
                    CellChange::AddOrRemove => self.kept_inside(sheet, &areas, extent),
                };
                kept.map(|kept| (sheet, kept))
            })
            .collect();
        *self.cache_mut() = kept;
    }

    // Removing a cell shrinks the end only when it sat on the last row or column, so the
    // end holds when every area lies before both. Undoing a dynamic array's anchor also
    // clears the cells it spilled into, outside its area.
    fn kept_inside(&self, sheet: SheetId, areas: &[Range], extent: Extent) -> Option<Extent> {
        let last = extent.last?;
        let inside = areas
            .iter()
            .all(|area| area.end.row < last.row && area.end.col < last.col);
        if extent.spills || !inside {
            return None;
        }
        self.grown(sheet, areas, extent)
    }

    // `None` when the sheet must be walked again: a sheet that had no cells may still hold
    // empty rows, which the walk counts. A dynamic array written in an area has already
    // spilled, so its result is read too, and the sheet is marked as holding spills.
    fn grown(&self, sheet: SheetId, areas: &[Range], extent: Extent) -> Option<Extent> {
        let last = extent.last?;
        let ws = self.model.get_model().workbook.worksheet(sheet.0).ok()?;
        let mut end = last;
        let mut spilling = Vec::new();
        let mut grow = |row: i32, col: i32| {
            let at = position(row, col);
            end = CellPos::new(end.row.max(at.row), end.col.max(at.col));
        };
        for area in areas {
            for_each_within(&ws.sheet_data, *area, |row, col, cell| {
                grow(row, col);
                if let Some((width, height)) = cell.dynamic_range() {
                    let last_row = row.saturating_add(height.max(1) - 1);
                    let last_col = col.saturating_add(width.max(1) - 1);
                    spilling.push(Range::new(position(row, col), position(last_row, last_col)));
                }
            });
        }
        let dynamic = !spilling.is_empty();
        for spill in spilling {
            for_each_within(&ws.sheet_data, spill, |row, col, _| grow(row, col));
        }
        Some(Extent {
            last: Some(end),
            spills: extent.spills || dynamic,
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
    use ironcalc::base::Recalculation;

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
        assert!(model.cache().contains_key(&SheetId(0)));
        let extent = model.cache().get(&SheetId(1)).copied().unwrap();
        assert_eq!(extent.last, CellPos::parse_a1("A3"));
        assert!(extent.spills);
        assert_eq!(model.used_end(SheetId(1)), CellPos::parse_a1("A3").unwrap());
        model
    }

    #[test]
    fn edits_that_do_not_evaluate_keep_the_end_of_a_sheet_with_a_spill() {
        let mut model = spill_on_the_second_sheet();
        assert_eq!(last_recalculation(&model), Recalculation::Full);
        bold(&mut model, 0, "C5");
        model.edit_keeping_cells(|model| model.set_rows_height(1, 2, 2, 40.0).unwrap());
        assert_eq!(model.cache().len(), 2);
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn a_dynamic_formula_written_below_the_used_area_grows_the_end_to_its_spill() {
        let mut model = two_filled_sheets();
        type_at(&mut model, 0, "B6", "=SEQUENCE(2,3)");
        let extent = model.cache().get(&SheetId(0)).copied().unwrap();
        assert_eq!(extent.last, CellPos::parse_a1("D7"));
        assert!(extent.spills);
        assert_cached_ends_match_a_walk(&model);
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

    fn undo(model: &mut CachedModel) {
        model
            .step(HistoryStep::Undo, |model, take| take(model))
            .unwrap();
    }

    fn redo(model: &mut CachedModel) {
        model
            .step(HistoryStep::Redo, |model, take| take(model))
            .unwrap();
    }

    fn assert_cached_ends_match_a_walk(model: &CachedModel) {
        let cached: Vec<(SheetId, CellPos)> = model
            .cache()
            .iter()
            .map(|(sheet, extent)| (*sheet, extent.last.unwrap_or_default()))
            .collect();
        for (sheet, last) in cached {
            assert_eq!(Some(last), model.walked_end(sheet), "sheet {}", sheet.0);
        }
    }

    #[test]
    fn an_undo_keeps_the_ends_of_the_sheets_it_does_not_touch() {
        let mut model = two_filled_sheets();
        type_at(&mut model, 0, "C3", "x");
        undo(&mut model);
        assert!(model.cache().contains_key(&SheetId(1)));
        assert!(!model.cache().contains_key(&SheetId(0)));
        model.used_end(SheetId(0));
        redo(&mut model);
        assert!(model.cache().contains_key(&SheetId(1)));
        model.used_end(SheetId(0));
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn an_undo_inside_the_used_area_keeps_its_end() {
        let mut model = two_filled_sheets();
        type_at(&mut model, 0, "D9", "end");
        type_at(&mut model, 0, "B2", "x");
        undo(&mut model);
        assert!(model.cache().contains_key(&SheetId(0)));
        assert_cached_ends_match_a_walk(&model);
        redo(&mut model);
        assert!(model.cache().contains_key(&SheetId(0)));
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn an_undo_on_the_last_row_drops_its_end() {
        let mut model = two_filled_sheets();
        type_at(&mut model, 0, "D9", "end");
        undo(&mut model);
        assert!(!model.cache().contains_key(&SheetId(0)));
    }

    #[test]
    fn an_undo_of_inserted_rows_drops_only_that_sheets_end() {
        let mut model = two_filled_sheets();
        model.insert_rows(0, 1, 1).unwrap();
        model.used_end(SheetId(0));
        model.used_end(SheetId(1));
        undo(&mut model);
        assert!(!model.cache().contains_key(&SheetId(0)));
        assert!(model.cache().contains_key(&SheetId(1)));
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn an_undo_of_a_new_sheet_drops_every_end() {
        let mut model = model();
        model.new_sheet().unwrap();
        model.used_end(SheetId(0));
        model.used_end(SheetId(1));
        undo(&mut model);
        assert!(model.cache().is_empty());
    }

    #[test]
    fn an_incremental_undo_keeps_the_end_of_an_untouched_sheet_with_a_spill() {
        let mut model = spill_on_the_second_sheet();
        type_at(&mut model, 0, "B1", "7");
        undo(&mut model);
        assert_eq!(last_recalculation(&model), Recalculation::Incremental);
        assert!(model.cache().contains_key(&SheetId(1)));
        assert_cached_ends_match_a_walk(&model);
    }

    fn bold(model: &mut CachedModel, sheet: u32, range: &str) {
        let range = Range::parse_a1(range).unwrap();
        let area = Area {
            sheet,
            row: row_i32(range.start.row),
            column: col_i32(range.start.col),
            width: i32::from(range.cols()),
            height: i32::try_from(range.rows()).unwrap(),
        };
        model.write_within(SheetId(sheet), range, |model| {
            model.update_range_style(&area, "font.b", "true").unwrap();
        });
    }

    #[test]
    fn a_style_grows_its_sheets_end_and_keeps_the_others() {
        let mut model = two_filled_sheets();
        bold(&mut model, 0, "C5");
        assert_eq!(
            model.cache().get(&SheetId(0)).map(|extent| extent.last),
            Some(CellPos::parse_a1("C5"))
        );
        assert!(model.cache().contains_key(&SheetId(1)));
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn styling_a_whole_column_reaches_a_row_with_its_own_height() {
        let mut model = two_filled_sheets();
        model.edit_keeping_cells(|model| model.set_rows_height(0, 7, 7, 40.0).unwrap());
        assert_eq!(model.cache().len(), 2);
        bold(&mut model, 0, "A1:A1048576");
        assert!(model.cache().contains_key(&SheetId(1)));
        model.used_end(SheetId(0));
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn row_heights_hidden_lines_and_panes_keep_every_end() {
        let mut model = two_filled_sheets();
        model.edit_keeping_cells(|model| {
            model.set_rows_height(0, 3, 3, 40.0).unwrap();
            model.set_columns_width(1, 2, 2, 90.0).unwrap();
            model.set_rows_hidden(0, 2, 2, true).unwrap();
            model.set_frozen_rows_count(1, 1).unwrap();
        });
        assert_eq!(model.cache().len(), 2);
        assert_cached_ends_match_a_walk(&model);
        undo(&mut model);
        undo(&mut model);
        assert_eq!(model.cache().len(), 2);
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn a_formula_that_may_spill_marks_its_sheet_before_it_spills() {
        let mut model = two_filled_sheets();
        type_at(&mut model, 0, "C1", "=IF(B1>0,SEQUENCE(9),0)");
        let anchor = model
            .get_model()
            .workbook
            .worksheet(0)
            .unwrap()
            .cell(1, 3)
            .and_then(Cell::dynamic_range);
        assert_eq!(anchor, Some((1, 1)));
        model.used_end(SheetId(0));
        assert!(model.cache().get(&SheetId(0)).unwrap().spills);
        type_at(&mut model, 0, "B1", "1");
        assert_eq!(last_recalculation(&model), Recalculation::Full);
        assert!(!model.cache().contains_key(&SheetId(0)));
        assert_eq!(model.used_end(SheetId(0)), CellPos::parse_a1("C9").unwrap());
        assert_cached_ends_match_a_walk(&model);
    }

    #[test]
    fn failed_lookup_is_not_cached() {
        let model = model();
        assert_eq!(model.used_end(SheetId(9)), CellPos::default());
        assert!(model.cache().is_empty());
    }
}
