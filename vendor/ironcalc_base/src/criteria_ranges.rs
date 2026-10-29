use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::{Arc, OnceLock};

use crate::{
    calc_result::CalcResult,
    criteria_totals::{Total, TotalKey},
    dependency_index::CellKey,
    expressions::types::CellReferenceIndex,
    functions::util::{criterion_key, value_keys, MatchKey},
    locale::Locale,
    model::{CellState, Model},
    types::Cell,
};

// A cell read costs about 500 ns (measured on a 200k-row workbook), so a smaller area is
// read again in under 0.5 ms, less than what keeping it saves.
const MIN_KEPT_CELLS: usize = 1024;
// Bounds the memory spent on kept values (slots and text) and totals, during an
// evaluation and between evaluations; past it, cells are read one by one as without
// keeping. A 200k-row column costs about 10 MB, so this fits about twelve, against
// 640 MB for the whole 200k-row workbook.
const KEPT_BYTES: usize = 128 << 20;
// Bounds what one incremental evaluation spends updating kept totals: changed cells checked
// against kept areas, plus changed offsets taken out of totals. Past it everything is read
// again instead. The 4M of the dependents walk cap; at about 25 ns each, about 100 ms.
const DELTA_WORK: usize = 4_000_000;

// (sheet, first row, first column, height, width)
pub(crate) type Area = (u32, i32, i32, i32, i32);

// Arc and OnceLock rather than Rc and OnceCell: Zenkai moves the model to background
// threads, so it must stay Send.
pub(crate) type Slots = Arc<[OnceLock<CalcResult>]>;

const SLOT_BYTES: usize = size_of::<OnceLock<CalcResult>>();
// A key of an index costs its hash map slot, at worst 8/7 of its size, plus its text.
const INDEX_KEY_BYTES: usize = (size_of::<MatchKey>() + size_of::<Vec<(i32, i32)>>() + 1) * 8 / 7;
const INDEX_CELL_BYTES: usize = size_of::<(i32, i32)>();

// Where each key is found in a kept area, in the order of a full read, so a criterion that
// matches by equality alone reads only the cells it can match.
pub(crate) struct CriteriaIndex {
    cells: HashMap<MatchKey, Vec<(i32, i32)>>,
}

impl CriteriaIndex {
    // None while a value of the area is not kept yet: reading it here could evaluate a cell
    // the full read would not have reached yet. None as well as soon as the index would
    // cost more than `limit`, so a refused index never takes more than that.
    fn build(
        slots: &[OnceLock<CalcResult>],
        width: i32,
        limit: usize,
    ) -> Option<(CriteriaIndex, usize)> {
        let mut cells: HashMap<MatchKey, Vec<(i32, i32)>> = HashMap::new();
        let mut spent = 0;
        let mut row_offset = 0;
        let mut column_offset = 0;
        for slot in slots {
            for key in value_keys(slot.get()?).into_iter().flatten() {
                spent += INDEX_CELL_BYTES;
                let at = cells.entry(key).or_insert_with_key(|key| {
                    spent += INDEX_KEY_BYTES;
                    if let MatchKey::Text(text) = key {
                        spent += text.len();
                    }
                    Vec::new()
                });
                at.push((row_offset, column_offset));
                if spent > limit {
                    return None;
                }
            }
            column_offset += 1;
            if column_offset == width {
                column_offset = 0;
                row_offset += 1;
            }
        }
        let bytes = cells
            .iter()
            .map(|(key, at)| {
                let text = match key {
                    MatchKey::Text(text) => text.len(),
                    MatchKey::Number(_) => 0,
                };
                INDEX_KEY_BYTES + text + at.capacity() * INDEX_CELL_BYTES
            })
            .sum();
        Some((CriteriaIndex { cells }, bytes))
    }

    /// The (row, column) offsets of the cells holding `key`, in the order of a full read.
    pub(crate) fn cells(&self, key: &MatchKey) -> &[(i32, i32)] {
        self.cells.get(key).map_or(&[], Vec::as_slice)
    }
}

enum Indexing {
    Asked,
    Built(Arc<CriteriaIndex>, usize),
    Refused,
}

// Values of the large areas that SUMIF, COUNTIF and the rest of the family read, so calls
// that differ only in their criterion read each cell once, and the totals of SUMIF(S) and
// COUNTIF(S), so an edit updates them by what changed instead of reading every cell
// again. A cell is kept when first read, so a sum range read only where the criteria
// match costs no more than before.
//
// Between evaluations only the totals stay, with the areas they read, holding the values
// of the last evaluation. An incremental evaluation takes out what its changed cells held
// before anything is read (`start_incremental`), and every total still kept when it ends
// is exact for the new values (`finish`).
pub(crate) struct CriteriaRanges {
    // Outside an evaluation cells may change, so nothing is read or kept.
    reading: bool,
    kept: HashMap<Area, Slots>,
    // Indexes of kept areas, for the current evaluation only.
    indexes: HashMap<Area, Indexing>,
    pub(crate) totals: HashMap<TotalKey, Total>,
    bytes_left: usize,
    delta_work: usize,
    // Reads that returned a value without keeping it: a total that needs one cannot be
    // kept.
    pub(crate) misses: u64,
    // A spill wrote cells that kept values or totals may hold; nothing is kept for the
    // rest of the evaluation.
    forgotten: bool,
}

impl CriteriaRanges {
    pub(crate) fn new() -> CriteriaRanges {
        CriteriaRanges::with_budget(KEPT_BYTES)
    }

    // A separate constructor so tests can exhaust a small budget.
    fn with_budget(bytes: usize) -> CriteriaRanges {
        CriteriaRanges {
            reading: false,
            kept: HashMap::new(),
            indexes: HashMap::new(),
            totals: HashMap::new(),
            bytes_left: bytes,
            delta_work: DELTA_WORK,
            misses: 0,
            forgotten: false,
        }
    }

    pub(crate) fn start_full(&mut self) {
        *self = CriteriaRanges::new();
        self.reading = true;
    }

    // Only valid when nothing but the `changed` cells changed since the last evaluation.
    pub(crate) fn start_incremental(&mut self, changed: &HashSet<CellKey>, locale: &Locale) {
        self.forget_indexes();
        self.reading = true;
        if self.kept.len().saturating_mul(changed.len()) > self.delta_work {
            self.start_full();
            return;
        }
        let mut changed_offsets: HashMap<Area, Vec<(i32, i32)>> = HashMap::new();
        for area in self.kept.keys() {
            let (sheet, row, column, height, width) = *area;
            let offsets: Vec<(i32, i32)> = changed
                .iter()
                .filter(|(cell_sheet, _, _)| *cell_sheet == sheet)
                .map(|&(_, cell_row, cell_column)| (cell_row - row, cell_column - column))
                .filter(|(row_offset, column_offset)| {
                    (0..height).contains(row_offset) && (0..width).contains(column_offset)
                })
                .collect();
            if !offsets.is_empty() {
                changed_offsets.insert(*area, offsets);
            }
        }
        let work: usize = self
            .totals
            .keys()
            .flat_map(TotalKey::areas)
            .filter_map(|area| changed_offsets.get(&area))
            .map(Vec::len)
            .sum();
        if work > self.delta_work {
            self.start_full();
            return;
        }
        let mut refund = 0;
        let kept = &self.kept;
        self.totals.retain(|key, total| {
            refund += total.forget_users(changed);
            let exact = total.take_out(key, kept, &changed_offsets, locale);
            if !exact {
                refund += total.bytes();
            }
            exact
        });
        for (area, offsets) in &changed_offsets {
            let width = area.4;
            // Nothing holds the slots between evaluations; if something did, starting over
            // is always correct.
            let Some(slots) = self.kept.get_mut(area).and_then(Arc::get_mut) else {
                self.start_full();
                return;
            };
            for &(row_offset, column_offset) in offsets {
                let index = row_offset as usize * width as usize + column_offset as usize;
                if let Some(value) = slots.get_mut(index).and_then(OnceLock::take) {
                    refund += heap_bytes(&value);
                }
            }
        }
        self.bytes_left += refund;
    }

    // Keeps the totals that are exact and still read by some formula, and the areas they
    // read.
    pub(crate) fn finish(&mut self) {
        if self.forgotten {
            *self = CriteriaRanges::new();
            return;
        }
        self.reading = false;
        self.forget_indexes();
        let mut refund = 0;
        self.totals.retain(|_, total| {
            let keep = total.is_current();
            if !keep {
                refund += total.bytes();
            }
            keep
        });
        let read: HashSet<Area> = self.totals.keys().flat_map(TotalKey::areas).collect();
        self.kept.retain(|area, slots| {
            let keep = read.contains(area);
            if !keep {
                refund += slots.len() * SLOT_BYTES
                    + slots
                        .iter()
                        .filter_map(OnceLock::get)
                        .map(heap_bytes)
                        .sum::<usize>();
            }
            keep
        });
        self.bytes_left += refund;
    }

    // A spill writes or clears cells that kept values may hold. Running calls may still
    // hold forgotten values, so their bytes come back only when the evaluation ends.
    pub(crate) fn forget_values(&mut self) {
        self.kept.clear();
        self.indexes.clear();
        self.totals.clear();
        self.forgotten = true;
    }

    fn forget_indexes(&mut self) {
        for (_, indexing) in self.indexes.drain() {
            if let Indexing::Built(_, bytes) = indexing {
                self.bytes_left += bytes;
            }
        }
    }

    // An area's index is built the second time it is asked for, so an area one criterion
    // reads once is not indexed, and only once every value of the area is kept.
    fn index(&mut self, area: Area, slots: &Slots) -> Option<Arc<CriteriaIndex>> {
        if !self.is_keeping() {
            return None;
        }
        match self.indexes.get(&area) {
            None => {
                self.indexes.insert(area, Indexing::Asked);
                return None;
            }
            Some(Indexing::Built(index, _)) => return Some(Arc::clone(index)),
            Some(Indexing::Refused) => return None,
            Some(Indexing::Asked) => {}
        }
        // A failed build is not tried again in this evaluation, as each try reads the area.
        let Some((index, bytes)) = CriteriaIndex::build(slots, area.4, self.bytes_left)
            .filter(|&(_, bytes)| self.spend(bytes))
        else {
            self.indexes.insert(area, Indexing::Refused);
            return None;
        };
        let index = Arc::new(index);
        self.indexes
            .insert(area, Indexing::Built(Arc::clone(&index), bytes));
        Some(index)
    }

    pub(crate) fn is_keeping(&self) -> bool {
        self.reading && !self.forgotten
    }

    pub(crate) fn is_kept(&self, area: &Area) -> bool {
        self.kept.contains_key(area)
    }

    pub(crate) fn spend(&mut self, bytes: usize) -> bool {
        if self.is_keeping() && self.bytes_left >= bytes {
            self.bytes_left -= bytes;
            true
        } else {
            false
        }
    }

    pub(crate) fn refund(&mut self, bytes: usize) {
        self.bytes_left += bytes;
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
    pub(crate) area: Area,
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
        let area = (sheet, row, column, height, width);
        let kept = self.kept_values(area);
        AreaValues { area, kept }
    }

    /// The cells of `values` that `criterion` can match, when an index of the area can tell
    /// them without reading every cell.
    pub(crate) fn criterion_cells(
        &mut self,
        values: &AreaValues,
        criterion: &CalcResult,
    ) -> Option<(Arc<CriteriaIndex>, MatchKey)> {
        let key = criterion_key(criterion, self.locale)?;
        let slots = values.kept.as_ref()?;
        let index = self.criteria_ranges.index(values.area, slots)?;
        Some((index, key))
    }

    pub(crate) fn area_value<'v>(
        &mut self,
        values: &'v AreaValues,
        row_offset: i32,
        column_offset: i32,
    ) -> Cow<'v, CalcResult> {
        let (sheet, row, column, _, width) = values.area;
        let reference = CellReferenceIndex {
            sheet,
            row: row + row_offset,
            column: column + column_offset,
        };
        let slot = values.kept.as_deref().and_then(|kept| {
            let index = i64::from(row_offset) * i64::from(width) + i64::from(column_offset);
            kept.get(usize::try_from(index).ok()?)
        });
        let Some(slot) = slot else {
            self.criteria_ranges.misses += 1;
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
            self.criteria_ranges.misses += 1;
            Cow::Owned(value)
        }
    }

    fn kept_values(&mut self, area: Area) -> Option<Slots> {
        let (_, _, _, height, width) = area;
        let cells = usize::try_from(height)
            .ok()?
            .checked_mul(usize::try_from(width).ok()?)?;
        if !self.criteria_ranges.is_keeping() {
            return None;
        }
        if let Some(values) = self.criteria_ranges.kept.get(&area) {
            return Some(Arc::clone(values));
        }
        if cells < MIN_KEPT_CELLS || !self.criteria_ranges.spend(cells.checked_mul(SLOT_BYTES)?) {
            return None;
        }
        let values: Slots = (0..cells).map(|_| OnceLock::new()).collect();
        self.criteria_ranges.kept.insert(area, Arc::clone(&values));
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
            .get(row, column)?;
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

    use std::collections::HashSet;

    use std::sync::OnceLock;

    use super::{
        CriteriaIndex, CriteriaRanges, INDEX_CELL_BYTES, INDEX_KEY_BYTES, KEPT_BYTES, SLOT_BYTES,
    };
    use crate::calc_result::CalcResult;
    use crate::criteria_totals::{TotalKey, USER_BYTES};
    use crate::exact_sum::ExactSum;
    use crate::expressions::types::CellReferenceIndex;
    use crate::functions::util::MatchKey;
    use crate::test::util::new_empty_model;

    fn reading(bytes: usize) -> CriteriaRanges {
        let mut store = CriteriaRanges::with_budget(bytes);
        store.reading = true;
        store
    }

    #[test]
    fn areas_from_1024_cells_are_kept() {
        let mut model = new_empty_model();
        model.criteria_ranges = reading(KEPT_BYTES);
        assert!(model.area_values(0, 1, 1, 1023, 1).kept.is_none());
        assert!(model.area_values(0, 1, 1, 1024, 1).kept.is_some());
        assert!(model.area_values(0, 1, 1, 512, 2).kept.is_some());
    }

    #[test]
    fn nothing_is_kept_after_a_spill_until_the_evaluation_ends() {
        let mut model = new_empty_model();
        for row in 1..=1024 {
            model._set(&format!("A{row}"), &row.to_string());
        }
        model.evaluate();
        model.criteria_ranges = reading(1024 * SLOT_BYTES);
        let values = model.area_values(0, 1, 1, 1024, 1);
        assert!(values.kept.is_some());
        model.criteria_ranges.forget_values();
        let values = model.area_values(0, 1, 1, 1024, 1);
        assert!(values.kept.is_none());
        for row_offset in 0..1024 {
            let value = model.area_value(&values, row_offset, 0);
            assert!(
                matches!(value.as_ref(), CalcResult::Number(n) if *n == f64::from(row_offset + 1))
            );
        }
        model.criteria_ranges.finish();
        model.criteria_ranges.start_full();
        assert!(model.area_values(0, 1, 1, 1024, 1).kept.is_some());
    }

    // A1:A1024 holding "a" and "b" in turn, read whole; returns the cells "a" is found at
    // the first three times an index is asked for, within `bytes` beyond the slots.
    fn indexed_cells(bytes: usize, read_whole: bool) -> Vec<Option<usize>> {
        let mut model = new_empty_model();
        for row in 1..=1024 {
            model._set(&format!("A{row}"), if row % 2 == 0 { "b" } else { "a" });
        }
        model.evaluate();
        model.criteria_ranges = reading(1024 * SLOT_BYTES + 1024 + bytes);
        let values = model.area_values(0, 1, 1, 1024, 1);
        let rows = if read_whole { 1024 } else { 1023 };
        for row_offset in 0..rows {
            model.area_value(&values, row_offset, 0);
        }
        let criterion = CalcResult::String("A".to_string());
        (0..3)
            .map(|_| {
                model
                    .criterion_cells(&values, &criterion)
                    .map(|(index, key)| {
                        let cells = index.cells(&key);
                        assert!(cells
                            .iter()
                            .all(|&(row, column)| row % 2 == 0 && column == 0));
                        cells.len()
                    })
            })
            .collect()
    }

    // Two keys of one letter, 512 cells each.
    const INDEX_BYTES: usize = 2 * (INDEX_KEY_BYTES + 1 + 512 * INDEX_CELL_BYTES);

    #[test]
    fn an_area_is_indexed_the_second_time_once_read_whole() {
        assert_eq!(
            indexed_cells(INDEX_BYTES, true),
            [None, Some(512), Some(512)]
        );
        assert_eq!(indexed_cells(INDEX_BYTES, false), [None, None, None]);
    }

    #[test]
    fn an_index_that_failed_is_not_built_again_in_the_evaluation() {
        let mut model = new_empty_model();
        for row in 1..=8 {
            model._set(&format!("A{row}"), "a");
        }
        model.evaluate();
        model.criteria_ranges = reading(KEPT_BYTES);
        let values = model.area_values(0, 1, 1, 8, 1);
        for row_offset in 0..7 {
            model.area_value(&values, row_offset, 0);
        }
        let criterion = CalcResult::String("a".to_string());
        assert!(model.criterion_cells(&values, &criterion).is_none());
        assert!(model.criterion_cells(&values, &criterion).is_none());
        model.area_value(&values, 7, 0);
        assert!(model.criterion_cells(&values, &criterion).is_none());
    }

    #[test]
    fn an_index_build_stops_at_its_limit() {
        let slots: Vec<OnceLock<CalcResult>> = (0..1024)
            .map(|row| OnceLock::from(CalcResult::String(["a", "b"][row % 2].to_string())))
            .collect();
        let (_, bytes) = CriteriaIndex::build(&slots, 1, INDEX_BYTES).unwrap();
        assert_eq!(bytes, INDEX_BYTES);
        assert!(CriteriaIndex::build(&slots, 1, INDEX_BYTES - 1).is_none());
    }

    #[test]
    fn an_index_past_the_budget_is_not_built() {
        assert_eq!(indexed_cells(INDEX_BYTES - 1, true), [None, None, None]);
    }

    #[test]
    fn an_index_gives_its_bytes_back_when_the_evaluation_ends() {
        let mut model = new_empty_model();
        for row in 1..=1024 {
            model._set(&format!("A{row}"), &(row % 7 + 2).to_string());
        }
        model.evaluate();
        model.criteria_ranges = reading(KEPT_BYTES);
        let values = model.area_values(0, 1, 1, 1024, 1);
        for row_offset in 0..1024 {
            model.area_value(&values, row_offset, 0);
        }
        let before = model.criteria_ranges.bytes_left;
        let criterion = CalcResult::Number(3.0);
        assert!(model.criterion_cells(&values, &criterion).is_none());
        let (index, key) = model.criterion_cells(&values, &criterion).unwrap();
        assert_eq!(key, MatchKey::Number(3f64.to_bits()));
        let threes = (1..=1024).filter(|row| row % 7 + 2 == 3).count();
        assert_eq!(index.cells(&key).len(), threes);
        assert!(model.criteria_ranges.bytes_left < before);
        drop(values);
        model.criteria_ranges.finish();
        assert!(model.criteria_ranges.indexes.is_empty());
        assert_eq!(model.criteria_ranges.bytes_left, KEPT_BYTES);
    }

    #[test]
    fn an_area_whose_slots_exceed_the_budget_is_not_kept() {
        let mut model = new_empty_model();
        model.criteria_ranges = reading(KEPT_BYTES);
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
        let slots = 1100 * SLOT_BYTES;
        model.criteria_ranges = reading(slots + 10 * 32_000);
        let values = model.area_values(0, 1, 1, 1100, 1);
        for row_offset in 0..1100 {
            let value = model.area_value(&values, row_offset, 0);
            assert!(matches!(value.as_ref(), CalcResult::String(s) if *s == text));
        }
        let kept = values.kept.as_deref().unwrap();
        let filled = kept.iter().filter(|slot| slot.get().is_some()).count();
        assert_eq!(filled, 10);
    }

    // Keeps the count of A1:A1024 within `bytes`; returns the budget left over after the
    // store dropped everything again, and whether the total was kept.
    fn keep_a_total(bytes: usize) -> (bool, usize) {
        let mut model = new_empty_model();
        for row in 1..=1024 {
            model._set(&format!("A{row}"), &row.to_string());
        }
        model.evaluate();
        model.criteria_ranges = reading(bytes);
        let misses = model.criteria_ranges.misses;
        let values = model.area_values(0, 1, 1, 1024, 1);
        let mut count = ExactSum::default();
        for row_offset in 0..1024 {
            let value = model.area_value(&values, row_offset, 0);
            assert!(matches!(value.as_ref(), CalcResult::Number(_)));
            count.add(1.0);
        }
        let criteria = [CalcResult::String(">0".to_string())];
        let key = TotalKey::new(std::slice::from_ref(&values), &criteria, None).unwrap();
        let cell = CellReferenceIndex {
            sheet: 0,
            row: 1,
            column: 2,
        };
        model.keep_total(key, count, misses, cell);
        drop(values);
        let kept = model.criteria_ranges.totals.len() == 1;
        model.criteria_ranges.finish();
        let changed = HashSet::from([(0, 1, 2)]);
        let locale = model.locale;
        model.criteria_ranges.start_incremental(&changed, locale);
        model.criteria_ranges.finish();
        assert!(model.criteria_ranges.totals.is_empty());
        assert!(model.criteria_ranges.kept.is_empty());
        (kept, model.criteria_ranges.bytes_left)
    }

    #[test]
    fn a_total_is_kept_only_within_the_budget_and_gives_its_bytes_back() {
        let criteria = [CalcResult::String(">0".to_string())];
        let mut model = new_empty_model();
        model.criteria_ranges = reading(KEPT_BYTES);
        let values = model.area_values(0, 1, 1, 1024, 1);
        let key_bytes = TotalKey::new(std::slice::from_ref(&values), &criteria, None)
            .unwrap()
            .bytes();
        let needed = 1024 * SLOT_BYTES + key_bytes + USER_BYTES;
        assert_eq!(keep_a_total(needed), (true, needed));
        assert_eq!(keep_a_total(needed - 1), (false, needed - 1));
    }

    // A count of A1:A1024 kept, then `changed` rows of A edited with `delta_work` allowed.
    fn update_a_total(changed_rows: i32, delta_work: usize) -> bool {
        let mut model = new_empty_model();
        for row in 1..=1024 {
            model._set(&format!("A{row}"), &row.to_string());
        }
        model.evaluate();
        model.criteria_ranges = reading(KEPT_BYTES);
        let misses = model.criteria_ranges.misses;
        let values = model.area_values(0, 1, 1, 1024, 1);
        let mut count = ExactSum::default();
        for row_offset in 0..1024 {
            model.area_value(&values, row_offset, 0);
            count.add(1.0);
        }
        let criteria = [CalcResult::String(">0".to_string())];
        let key = TotalKey::new(std::slice::from_ref(&values), &criteria, None).unwrap();
        let cell = CellReferenceIndex {
            sheet: 0,
            row: 1,
            column: 2,
        };
        model.keep_total(key, count, misses, cell);
        drop(values);
        model.criteria_ranges.finish();
        model.criteria_ranges.delta_work = delta_work;
        let changed: HashSet<_> = (1..=changed_rows).map(|row| (0, row, 1)).collect();
        let locale = model.locale;
        model.criteria_ranges.start_incremental(&changed, locale);
        model.criteria_ranges.totals.len() == 1
    }

    #[test]
    fn deltas_past_the_work_cap_read_everything_again() {
        assert!(update_a_total(100, 100));
        assert!(!update_a_total(100, 99));
    }
}
