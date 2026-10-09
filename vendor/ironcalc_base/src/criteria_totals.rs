use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use crate::{
    calc_result::CalcResult,
    criteria_ranges::{Area, AreaValues, Slots},
    dependency_index::CellKey,
    exact_sum::{ExactSum, MAX_PARTIALS},
    expressions::types::CellReferenceIndex,
    functions::util::{build_criteria, Criterion},
    locale::Locale,
    model::Model,
};

// A delta reads each changed row twice, against once per row for reading the whole area
// again; past this share of changed rows, it saves too little to be worth it.
const DELTA_SHARE: usize = 8;

pub(crate) const USER_BYTES: usize = size_of::<CellKey>();

// What `build_criteria` makes of a criterion depends only on this and the locale, and a
// change of locale evaluates the whole workbook. So a criterion that comes from another
// cell, a wildcard or a volatile function keys a different total whenever its value
// changes, and a delta never mixes two criteria.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CriterionKey {
    Number(u64),
    Text(String),
    Boolean(bool),
    Empty,
}

impl CriterionKey {
    fn new(value: &CalcResult) -> Option<CriterionKey> {
        match value {
            CalcResult::Number(number) => Some(CriterionKey::Number(number.to_bits())),
            CalcResult::String(text) => Some(CriterionKey::Text(text.clone())),
            CalcResult::Boolean(boolean) => Some(CriterionKey::Boolean(*boolean)),
            CalcResult::EmptyCell | CalcResult::EmptyArg => Some(CriterionKey::Empty),
            _ => None,
        }
    }

    fn value(&self) -> CalcResult {
        match self {
            CriterionKey::Number(bits) => CalcResult::Number(f64::from_bits(*bits)),
            CriterionKey::Text(text) => CalcResult::String(text.clone()),
            CriterionKey::Boolean(boolean) => CalcResult::Boolean(*boolean),
            CriterionKey::Empty => CalcResult::EmptyCell,
        }
    }
}

// What COUNTIFS counts or SUMIFS sums. Every area has the same height and width.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TotalKey {
    cases: Vec<(Area, CriterionKey)>,
    // COUNTIFS has none.
    sum: Option<Area>,
}

impl TotalKey {
    pub(crate) fn new(
        cases: &[AreaValues],
        criteria: &[CalcResult],
        sums: Option<&AreaValues>,
    ) -> Option<TotalKey> {
        if cases.is_empty() || cases.len() != criteria.len() {
            return None;
        }
        let cases = cases
            .iter()
            .zip(criteria)
            .map(|(values, criterion)| Some((values.area, CriterionKey::new(criterion)?)))
            .collect::<Option<Vec<_>>>()?;
        Some(TotalKey {
            cases,
            sum: sums.map(|values| values.area),
        })
    }

    pub(crate) fn areas(&self) -> impl Iterator<Item = Area> + '_ {
        self.cases.iter().map(|(area, _)| *area).chain(self.sum)
    }

    fn criteria(&self) -> Vec<CalcResult> {
        self.cases
            .iter()
            .map(|(_, criterion)| criterion.value())
            .collect()
    }

    // Pending offsets are left out: they live within one evaluation and are capped by
    // `DELTA_SHARE`.
    pub(crate) fn bytes(&self) -> usize {
        let text: usize = self
            .cases
            .iter()
            .map(|(_, criterion)| match criterion {
                CriterionKey::Text(text) => text.capacity(),
                _ => 0,
            })
            .sum();
        size_of::<TotalKey>()
            + size_of::<Total>()
            + self.cases.capacity() * size_of::<(Area, CriterionKey)>()
            + text
            + MAX_PARTIALS * size_of::<f64>()
    }
}

enum Contribution {
    Nothing,
    Number(f64),
    // An error in the sum area, which the full read returns in place of a total, or a
    // value that was not kept.
    Unknown,
}

// What one cell offset adds to a total, as the loops of `fn_countifs` and `scan_ifs`
// decide it. `value_at` reads the offset in the n-th criteria area, then in the sum area.
fn contribution(
    criteria: &[Criterion<'_>],
    counts: bool,
    mut value_at: impl FnMut(usize) -> Option<CalcResult>,
) -> Contribution {
    for (index, criterion) in criteria.iter().enumerate() {
        match value_at(index) {
            Some(value) if criterion(&value) => {}
            Some(_) => return Contribution::Nothing,
            None => return Contribution::Unknown,
        }
    }
    if counts {
        return Contribution::Number(1.0);
    }
    match value_at(criteria.len()) {
        Some(CalcResult::Number(number)) => Contribution::Number(number),
        Some(value) if value.is_error() => Contribution::Unknown,
        Some(_) => Contribution::Nothing,
        None => Contribution::Unknown,
    }
}

pub(crate) struct Total {
    sum: ExactSum,
    // The formulas that read this total in their last evaluation.
    users: Vec<CellKey>,
    // Changed offsets whose new values are still to be added.
    pending: Vec<(i32, i32)>,
    key_bytes: usize,
}

impl Total {
    pub(crate) fn bytes(&self) -> usize {
        self.key_bytes + self.users.len() * USER_BYTES
    }

    // A formula that changed is evaluated again and reads the total again if it still
    // needs it. Returns the bytes freed.
    pub(crate) fn forget_users(&mut self, changed: &HashSet<CellKey>) -> usize {
        let before = self.users.len();
        self.users.retain(|user| !changed.contains(user));
        (before - self.users.len()) * USER_BYTES
    }

    pub(crate) fn is_current(&self) -> bool {
        self.pending.is_empty() && !self.users.is_empty()
    }

    // Subtracts what the changed cells added with their old values, still in `kept`, and
    // leaves their offsets pending. False when the total cannot be kept.
    pub(crate) fn take_out(
        &mut self,
        key: &TotalKey,
        kept: &HashMap<Area, Slots>,
        changed_offsets: &HashMap<Area, Vec<(i32, i32)>>,
        locale: &Locale,
    ) -> bool {
        let Some(&((_, _, _, height, width), _)) = key.cases.first() else {
            return false;
        };
        let mut offsets: Vec<(i32, i32)> = key
            .areas()
            .filter_map(|area| changed_offsets.get(&area))
            .flatten()
            .copied()
            .collect();
        if offsets.is_empty() {
            return true;
        }
        offsets.sort_unstable();
        offsets.dedup();
        let cells = height as usize * width as usize;
        if offsets.len() > cells / DELTA_SHARE {
            return false;
        }
        let Some(slots) = key
            .areas()
            .map(|area| kept.get(&area))
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let criteria_values = key.criteria();
        let criteria: Vec<Criterion<'_>> = criteria_values
            .iter()
            .map(|criterion| build_criteria(criterion, locale))
            .collect();
        for &(row_offset, column_offset) in &offsets {
            let index = row_offset as usize * width as usize + column_offset as usize;
            let old = contribution(&criteria, key.sum.is_none(), |area| {
                slots[area].get(index)?.get().cloned()
            });
            match old {
                Contribution::Number(number) => self.sum.add(-number),
                Contribution::Nothing => {}
                Contribution::Unknown => return false,
            }
        }
        self.pending = offsets;
        self.sum.is_exact()
    }
}

impl Model<'_> {
    // The total of `key`, brought up to date with the values of this evaluation, or None
    // when the caller must read the whole area. `cases` and `sums` are the areas `key` was
    // made from.
    pub(crate) fn kept_total(
        &mut self,
        key: &TotalKey,
        cases: &[AreaValues],
        sums: Option<&AreaValues>,
        cell: CellReferenceIndex,
    ) -> Option<f64> {
        if !self.criteria_ranges.is_keeping() {
            return None;
        }
        let (key, mut total) = self.criteria_ranges.totals.remove_entry(key)?;
        if !total.pending.is_empty() {
            let criteria_values = key.criteria();
            let locale = self.locale;
            let criteria: Vec<Criterion<'_>> = criteria_values
                .iter()
                .map(|criterion| build_criteria(criterion, locale))
                .collect();
            let misses = self.criteria_ranges.misses;
            for (row_offset, column_offset) in std::mem::take(&mut total.pending) {
                let new = contribution(&criteria, key.sum.is_none(), |area| {
                    let values = cases.get(area).or(sums)?;
                    Some(
                        self.area_value(values, row_offset, column_offset)
                            .into_owned(),
                    )
                });
                match new {
                    Contribution::Number(number) => total.sum.add(number),
                    Contribution::Nothing => {}
                    Contribution::Unknown => {
                        self.criteria_ranges.refund(total.bytes());
                        return None;
                    }
                }
            }
            // A read that was not kept leaves a slot a later delta would need.
            if self.criteria_ranges.misses != misses
                || !self.criteria_ranges.is_keeping()
                || !total.sum.is_exact()
            {
                self.criteria_ranges.refund(total.bytes());
                return None;
            }
            self.criteria_deltas += 1;
        }
        let user = (cell.sheet, cell.row, cell.column);
        if !total.users.contains(&user) {
            if !self.criteria_ranges.spend(USER_BYTES) {
                self.criteria_ranges.refund(total.bytes());
                return None;
            }
            total.users.push(user);
        }
        let value = total.sum.value();
        if let Some(replaced) = self.criteria_ranges.totals.insert(key, total) {
            self.criteria_ranges.refund(replaced.bytes());
        }
        Some(value)
    }

    // Keeps a total just read from every cell, when every value it read was kept: a later
    // delta reads them as the old values.
    pub(crate) fn keep_total(
        &mut self,
        key: TotalKey,
        sum: ExactSum,
        misses: u64,
        cell: CellReferenceIndex,
    ) {
        let store = &mut self.criteria_ranges;
        if store.misses != misses
            || !sum.is_exact()
            || !key.areas().all(|area| store.is_kept(&area))
            || store.totals.contains_key(&key)
        {
            return;
        }
        let key_bytes = key.bytes();
        if !store.spend(key_bytes + USER_BYTES) {
            return;
        }
        store.totals.insert(
            key,
            Total {
                sum,
                users: vec![(cell.sheet, cell.row, cell.column)],
                pending: Vec::new(),
                key_bytes,
            },
        );
    }
}
