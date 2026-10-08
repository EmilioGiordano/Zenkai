use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::{
    calc_result::CalcResult,
    expressions::types::CellReferenceIndex,
    model::{CellState, Model},
    types::Cell,
};

// Smaller areas are cheaper to read again than to keep.
const MIN_KEPT_CELLS: usize = 1024;
// Bounds the memory one recalculation can spend on kept values.
const MAX_KEPT_CELLS: usize = 4_000_000;

// (sheet, first row, first column, height, width)
type Area = (u32, i32, i32, i32, i32);

// Values of the large areas that SUMIF, COUNTIF and the rest of the family read, kept
// while one evaluation runs, so calls that differ only in their criterion read each cell
// once. A cell is kept when first read, so a sum range read only where the criteria
// match costs no more than before. Outside an evaluation cells may change, so nothing is
// kept.
pub(crate) enum CriteriaRanges {
    Off,
    On {
        kept: HashMap<Area, Arc<[OnceLock<CalcResult>]>>,
        kept_cells: usize,
    },
}

impl CriteriaRanges {
    pub(crate) fn on() -> CriteriaRanges {
        CriteriaRanges::On {
            kept: HashMap::new(),
            kept_cells: 0,
        }
    }

    // A spill writes cells that kept values may hold as empty.
    pub(crate) fn forget_values(&mut self) {
        if let CriteriaRanges::On { kept, kept_cells } = self {
            kept.clear();
            *kept_cells = 0;
        }
    }
}

pub(crate) struct AreaValues {
    sheet: u32,
    row: i32,
    column: i32,
    width: i32,
    kept: Option<Arc<[OnceLock<CalcResult>]>>,
}

impl Model<'_> {
    pub(crate) fn area_values(
        &mut self,
        sheet: u32,
        row: i32,
        column: i32,
        height: i32,
        width: i32,
    ) -> AreaValues {
        let kept = self.kept_values((sheet, row, column, height, width));
        AreaValues {
            sheet,
            row,
            column,
            width,
            kept,
        }
    }

    pub(crate) fn area_value<'v>(
        &mut self,
        values: &'v AreaValues,
        row_offset: i32,
        column_offset: i32,
    ) -> Cow<'v, CalcResult> {
        let reference = CellReferenceIndex {
            sheet: values.sheet,
            row: values.row + row_offset,
            column: values.column + column_offset,
        };
        let slot = values.kept.as_deref().and_then(|kept| {
            let index = i64::from(row_offset) * i64::from(values.width) + i64::from(column_offset);
            kept.get(usize::try_from(index).ok()?)
        });
        let Some(slot) = slot else {
            return Cow::Owned(self.read_cell(reference));
        };
        if let Some(value) = slot.get() {
            return Cow::Borrowed(value);
        }
        let circular_hits = self.circular_hits;
        let value = self.read_cell(reference);
        // A cell reached while it is being evaluated reads as #CIRC! only at that moment.
        if self.circular_hits == circular_hits {
            Cow::Borrowed(slot.get_or_init(|| value))
        } else {
            Cow::Owned(value)
        }
    }

    fn kept_values(&mut self, area: Area) -> Option<Arc<[OnceLock<CalcResult>]>> {
        let (_, _, _, height, width) = area;
        let cells = usize::try_from(height)
            .ok()?
            .checked_mul(usize::try_from(width).ok()?)?;
        let CriteriaRanges::On { kept, kept_cells } = &mut self.criteria_ranges else {
            return None;
        };
        if let Some(values) = kept.get(&area) {
            return Some(Arc::clone(values));
        }
        if cells < MIN_KEPT_CELLS || kept_cells.saturating_add(cells) > MAX_KEPT_CELLS {
            return None;
        }
        let values: Arc<[OnceLock<CalcResult>]> = (0..cells).map(|_| OnceLock::new()).collect();
        kept.insert(area, Arc::clone(&values));
        *kept_cells += cells;
        Some(values)
    }

    fn read_cell(&mut self, reference: CellReferenceIndex) -> CalcResult {
        match self.stored_value(reference) {
            Some(value) => value,
            None => self.evaluate_cell(reference),
        }
    }

    // What `evaluate_cell` returns for a cell that needs no evaluation, without cloning
    // the cell.
    fn stored_value(&self, reference: CellReferenceIndex) -> Option<CalcResult> {
        let CellReferenceIndex { sheet, row, column } = reference;
        let cell = self
            .workbook
            .worksheets
            .get(sheet as usize)?
            .sheet_data
            .get(&row)?
            .get(&column)?;
        let evaluated = match cell {
            Cell::SpillCell { .. } | Cell::ArrayFormula { .. } => false,
            Cell::CellFormula { .. } => matches!(
                self.cells.get(&(sheet, row, column)),
                Some(CellState::Evaluated)
            ),
            _ => true,
        };
        evaluated.then(|| self.get_cell_value(cell, reference))
    }
}
