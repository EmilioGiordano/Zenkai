use std::borrow::Cow;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::{Arc, OnceLock};

use crate::{
    calc_result::CalcResult,
    expressions::types::CellReferenceIndex,
    model::{CellState, Model},
    types::Cell,
};

// A cell read costs about 500 ns (measured on a 200k-row workbook), so a smaller area is
// read again in under 0.5 ms, less than what keeping it saves.
const MIN_KEPT_CELLS: usize = 1024;
// Bounds the memory one evaluation spends on kept values, slots and text included; past
// it, cells are read one by one as without keeping. A 200k-row column costs about 11 MB,
// so this fits about ten, against 640 MB for the whole 200k-row workbook.
const KEPT_BYTES: usize = 128 << 20;

// (sheet, first row, first column, height, width)
type Area = (u32, i32, i32, i32, i32);

// Arc and OnceLock rather than Rc and OnceCell: Zenkai moves the model to background
// threads, so it must stay Send.
type Slots = Arc<[OnceLock<CalcResult>]>;

// Values of the large areas that SUMIF, COUNTIF and the rest of the family read, kept
// while one evaluation runs, so calls that differ only in their criterion read each cell
// once. A cell is kept when first read, so a sum range read only where the criteria
// match costs no more than before. Outside an evaluation cells may change, so nothing is
// kept.
pub(crate) enum CriteriaRanges {
    Off,
    On {
        kept: HashMap<Area, Slots>,
        // Never refunded during an evaluation: running calls may still hold forgotten
        // values.
        bytes_left: usize,
    },
}

impl CriteriaRanges {
    pub(crate) fn on() -> CriteriaRanges {
        CriteriaRanges::with_budget(KEPT_BYTES)
    }

    // A separate constructor so tests can exhaust a small budget.
    fn with_budget(bytes: usize) -> CriteriaRanges {
        CriteriaRanges::On {
            kept: HashMap::new(),
            bytes_left: bytes,
        }
    }

    // A spill writes or clears cells that kept values may hold.
    pub(crate) fn forget_values(&mut self) {
        if let CriteriaRanges::On { kept, .. } = self {
            kept.clear();
        }
    }

    fn spend(&mut self, bytes: usize) -> bool {
        match self {
            CriteriaRanges::On { bytes_left, .. } if *bytes_left >= bytes => {
                *bytes_left -= bytes;
                true
            }
            _ => false,
        }
    }
}

fn heap_bytes(value: &CalcResult) -> usize {
    match value {
        CalcResult::String(text) => text.capacity(),
        CalcResult::Error { message, .. } => message.capacity(),
        _ => 0,
    }
}

pub(crate) struct AreaValues {
    sheet: u32,
    row: i32,
    column: i32,
    width: i32,
    kept: Option<Slots>,
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
        if self.circular_hits == circular_hits && self.criteria_ranges.spend(heap_bytes(&value)) {
            Cow::Borrowed(slot.get_or_init(|| value))
        } else {
            Cow::Owned(value)
        }
    }

    fn kept_values(&mut self, area: Area) -> Option<Slots> {
        let (_, _, _, height, width) = area;
        let cells = usize::try_from(height)
            .ok()?
            .checked_mul(usize::try_from(width).ok()?)?;
        let CriteriaRanges::On { kept, .. } = &self.criteria_ranges else {
            return None;
        };
        if let Some(values) = kept.get(&area) {
            return Some(Arc::clone(values));
        }
        if cells < MIN_KEPT_CELLS
            || !self
                .criteria_ranges
                .spend(cells.checked_mul(size_of::<OnceLock<CalcResult>>())?)
        {
            return None;
        }
        let values: Slots = (0..cells).map(|_| OnceLock::new()).collect();
        if let CriteriaRanges::On { kept, .. } = &mut self.criteria_ranges {
            kept.insert(area, Arc::clone(&values));
        }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::mem::size_of;
    use std::sync::OnceLock;

    use super::CriteriaRanges;
    use crate::calc_result::CalcResult;
    use crate::test::util::new_empty_model;

    const SLOT: usize = size_of::<OnceLock<CalcResult>>();

    #[test]
    fn areas_from_1024_cells_are_kept() {
        let mut model = new_empty_model();
        model.criteria_ranges = CriteriaRanges::on();
        assert!(model.area_values(0, 1, 1, 1023, 1).kept.is_none());
        assert!(model.area_values(0, 1, 1, 1024, 1).kept.is_some());
        assert!(model.area_values(0, 1, 1, 512, 2).kept.is_some());
    }

    #[test]
    fn values_forgotten_by_spills_stay_counted_until_the_budget_runs_out() {
        let mut model = new_empty_model();
        for row in 1..=1024 {
            model._set(&format!("A{row}"), &row.to_string());
        }
        model.evaluate();
        model.criteria_ranges = CriteriaRanges::with_budget(2 * 1024 * SLOT);
        for spill in 0..3 {
            let values = model.area_values(0, 1, 1, 1024, 1);
            assert_eq!(values.kept.is_some(), spill < 2, "after {spill} spills");
            for row_offset in 0..1024 {
                let value = model.area_value(&values, row_offset, 0);
                assert!(
                    matches!(value.as_ref(), CalcResult::Number(n) if *n == f64::from(row_offset + 1))
                );
            }
            model.criteria_ranges.forget_values();
        }
    }

    #[test]
    fn an_area_whose_slots_exceed_the_budget_is_not_kept() {
        let mut model = new_empty_model();
        model.criteria_ranges = CriteriaRanges::on();
        let values = model.area_values(0, 1, 1, 4_000_000, 1);
        assert!(values.kept.is_none());
    }

    #[test]
    fn long_text_stops_being_kept_at_the_budget() {
        let mut model = new_empty_model();
        let text = "x".repeat(32_000);
        for row in 1..=1100 {
            model._set(&format!("A{row}"), &text);
        }
        model.evaluate();
        let slots = 1100 * SLOT;
        model.criteria_ranges = CriteriaRanges::with_budget(slots + 10 * 32_000);
        let values = model.area_values(0, 1, 1, 1100, 1);
        for row_offset in 0..1100 {
            let value = model.area_value(&values, row_offset, 0);
            assert!(matches!(value.as_ref(), CalcResult::String(s) if *s == text));
        }
        let kept = values.kept.as_deref().unwrap();
        let filled = kept.iter().filter(|slot| slot.get().is_some()).count();
        assert_eq!(filled, 10);
    }
}
