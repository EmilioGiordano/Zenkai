use std::collections::HashSet;

use crate::{
    cell::CellValue,
    criteria_ranges::CriteriaRanges,
    dependency_index::{CellKey, DependencyIndex},
    expressions::{parser::Node, types::CellReferenceIndex},
    model::Model,
    types::Cell,
};

/// How the last recalculation of a [`Model`] was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recalculation {
    /// Every formula of the workbook was evaluated.
    Full,
    /// Only the formulas the edits could change, and the volatile ones, were evaluated.
    Incremental,
}

// The (width, height) a dynamic formula's result had when the index was built.
pub(crate) struct Spill {
    anchor: CellKey,
    size: (i32, i32),
}

impl Spill {
    fn covers(&self, (sheet, row, column): CellKey) -> bool {
        let (anchor_sheet, anchor_row, anchor_column) = self.anchor;
        let (width, height) = self.size;
        sheet == anchor_sheet
            && (anchor_row..anchor_row.saturating_add(height)).contains(&row)
            && (anchor_column..anchor_column.saturating_add(width)).contains(&column)
    }
}

// The most cells kept as reached again while being evaluated. A cell takes about 12 bytes
// in the set and the set at most doubles that, so this is about 3 MB; past it the
// workbook is evaluated in full after every edit, as when there was no index with cycles.
pub(crate) const MAX_CIRCULAR_CELLS: usize = 100_000;

pub(crate) fn circular_cells_fit(count: usize) -> bool {
    count <= MAX_CIRCULAR_CELLS
}

// The most cells kept as tainted by a cycle (12 bytes each, about 6 MB with the set's
// headroom). Past it, or past the walk limit, the edit is evaluated in full and the rebuild
// starts the set again, so a workbook whose cycle readers shrink gets incremental edits back.
pub(crate) const MAX_TAINTED_CELLS: usize = 250_000;

enum Taint {
    Unknown,
    TooMany,
    Cells(HashSet<CellKey>),
}

pub(crate) struct Dependencies {
    index: DependencyIndex,
    spills: Vec<Spill>,
    // The cells a circular reference was found at when the index was built.
    circular: Vec<CellKey>,
    // The cells and everything that reads them, worked out the first time an edit needs it.
    // An edit that gives a cell a formula may add readers, so it drops a known set; one that
    // only writes a value or clears a cell cannot, and the stale entries left by cleared
    // formulas only make the check more cautious. `TooMany` lasts until the index is rebuilt,
    // which the full evaluation it forces does at once.
    tainted: Taint,
}

fn reference((sheet, row, column): CellKey) -> CellReferenceIndex {
    CellReferenceIndex { sheet, row, column }
}

impl Model<'_> {
    pub(crate) fn evaluate_indexed(&mut self) {
        self.evaluate();
        if self.circular_hits != 0 {
            // Totals kept while a cell could read as #CIRC! are not worth trusting.
            self.criteria_ranges = CriteriaRanges::new();
        }
        if circular_cells_fit(self.circular_cells.len()) {
            self.dependencies = Some(Box::new(self.build_dependencies()));
        }
    }

    // The result of a circular reference depends on the order formulas are visited in, so
    // an edit may only leave it alone if no cell it changes is, or reads, a cell the
    // cycle was found at, and if it cannot read one through a reference found at run time.
    fn edits_reach_cycles(
        dependencies: &mut Dependencies,
        edited: &[CellKey],
        dirty: &[CellKey],
        max_tainted: usize,
    ) -> bool {
        if dependencies.circular.is_empty() {
            return false;
        }
        if dependencies.index.has_reads_at_run_time() {
            return true;
        }
        if matches!(dependencies.tainted, Taint::Unknown) {
            dependencies.tainted = match dependencies
                .index
                .dependents_of(&dependencies.circular, max_tainted)
            {
                Some(cells) => Taint::Cells(cells),
                None => Taint::TooMany,
            };
        }
        let Taint::Cells(tainted) = &dependencies.tainted else {
            return true;
        };
        edited
            .iter()
            .chain(dirty)
            .any(|cell| tainted.contains(cell))
    }

    // Only valid when nothing but the content of the `edited` cells changed since the last
    // evaluation; `UserModel` tracks that. Falls back to `evaluate_indexed` when there is
    // no index, or an array, a spill or a circular reference is involved.
    pub(crate) fn evaluate_incremental(&mut self, edited: &[CellKey]) {
        let Some(mut dependencies) = self.dependencies.take() else {
            self.evaluate_indexed();
            return;
        };
        if self.edits_touch_spills(&dependencies.spills, edited) {
            self.evaluate_indexed();
            return;
        }
        for &cell in edited {
            dependencies.index.forget_volatile(cell);
            if let Some(node) = self.formula_node(cell) {
                dependencies.index.add(cell, node);
                if matches!(dependencies.tainted, Taint::Cells(_)) {
                    dependencies.tainted = Taint::Unknown;
                }
            }
        }
        dependencies.index.sort();

        let Some(affected) = dependencies.index.affected_by(edited) else {
            self.evaluate_indexed();
            return;
        };
        let mut dirty: Vec<CellKey> = affected
            .into_iter()
            .filter(|cell| self.formula_node(*cell).is_some())
            .collect();
        // The order of a full evaluation, so recursion stays as shallow as there.
        dirty.sort_unstable();
        if dirty.iter().any(|cell| self.is_array_formula(*cell))
            || Self::edits_reach_cycles(&mut dependencies, edited, &dirty, MAX_TAINTED_CELLS)
        {
            self.evaluate_indexed();
            return;
        }

        for cell in &dirty {
            self.cells.remove(cell);
            self.support.remove(&reference(*cell));
        }
        self.clear_variable_stack();
        self.clear_lambdas();
        let circular_hits = self.circular_hits;
        let changed: HashSet<CellKey> = edited.iter().chain(&dirty).copied().collect();
        self.criteria_ranges
            .start_incremental(&changed, self.locale);
        for cell in &dirty {
            self.evaluate_cell(reference(*cell));
        }
        self.criteria_ranges.finish();
        // A formula that now spills writes into cells nothing was watching.
        if self.circular_hits != circular_hits
            || dirty.iter().any(|cell| self.is_array_formula(*cell))
        {
            self.evaluate_indexed();
            return;
        }
        self.update_conditional_formatting(&changed);
        self.dependencies = Some(dependencies);
        self.last_recalculation = Recalculation::Incremental;
    }

    /// How the last evaluation was done.
    pub fn last_recalculation(&self) -> Recalculation {
        self.last_recalculation
    }

    /// How many times every formula was evaluated, since the model was created. An edit that
    /// leaves it alone did not evaluate in full, so it did not add or remove a spilled cell.
    pub fn full_evaluations(&self) -> u64 {
        self.full_evaluations
    }

    /// How many SUMIF, SUMIFS, COUNTIF and COUNTIFS results were updated by what changed
    /// instead of reading their whole ranges again, since the model was created.
    pub fn criteria_deltas(&self) -> u64 {
        self.criteria_deltas
    }

    fn build_dependencies(&self) -> Dependencies {
        let mut index = DependencyIndex::new();
        let mut spills = Vec::new();
        for (sheet, worksheet) in (0u32..).zip(&self.workbook.worksheets) {
            if worksheet.shared_formulas.is_empty() {
                continue;
            }
            for (row, column, cell) in worksheet.sheet_data.iter() {
                let key = (sheet, row, column);
                if let Some(node) = self.formula_node(key) {
                    index.add(key, node);
                }
                if let Some(size) = cell.dynamic_range() {
                    spills.push(Spill { anchor: key, size });
                }
            }
        }
        index.sort();
        index.shrink_to_fit();
        let mut circular: Vec<CellKey> = self.circular_cells.iter().copied().collect();
        circular.sort_unstable();
        Dependencies {
            index,
            spills,
            circular,
            tainted: Taint::Unknown,
        }
    }

    fn formula_node(&self, (sheet, row, column): CellKey) -> Option<&Node> {
        let formula = self
            .workbook
            .worksheets
            .get(sheet as usize)?
            .cell(row, column)?
            .get_formula()?;
        self.parsed_formulas
            .get(sheet as usize)?
            .get(usize::try_from(formula).ok()?)
            .map(|(node, _)| node)
    }

    // Includes a dynamic formula whose result fits in its own cell: a full evaluation
    // visits dynamic formulas first, so cells reading one that just became blocked get
    // `#SPILL!` there and its first value here.
    fn is_array_formula(&self, (sheet, row, column): CellKey) -> bool {
        matches!(
            self.workbook
                .worksheets
                .get(sheet as usize)
                .and_then(|worksheet| worksheet.cell(row, column)),
            Some(Cell::ArrayFormula { .. }) | Some(Cell::SpillCell { .. })
        )
    }

    fn spill_size(&self, (sheet, row, column): CellKey) -> Option<(i32, i32)> {
        match self
            .workbook
            .worksheets
            .get(sheet as usize)
            .and_then(|worksheet| worksheet.cell(row, column))
        {
            Some(cell) => cell.dynamic_range(),
            _ => None,
        }
    }

    // Whether the edits may grow, shrink, block or unblock a spill.
    fn edits_touch_spills(&self, spills: &[Spill], edited: &[CellKey]) -> bool {
        let blocked = |anchor: CellKey| {
            matches!(
                self.get_cell_value_by_index(anchor.0, anchor.1, anchor.2),
                Ok(CellValue::String(value)) if value == "#SPILL!"
            )
        };
        spills.iter().any(|spill| {
            blocked(spill.anchor)
                || self.spill_size(spill.anchor) != Some(spill.size)
                || edited.iter().any(|cell| spill.covers(*cell))
        }) || edited.iter().any(|cell| self.is_array_formula(*cell))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Model, Taint};
    use crate::UserModel;

    #[test]
    fn too_many_tainted_cells_stay_remembered_until_the_index_is_rebuilt() {
        let mut user_model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
        user_model.set_user_input(0, 1, 1, "1").unwrap();
        user_model.set_user_input(0, 1, 4, "=C1*2").unwrap();
        user_model.set_user_input(0, 1, 3, "=C2+1").unwrap();
        user_model.set_user_input(0, 2, 3, "=C1+1").unwrap();
        let model = user_model.get_model();
        let mut dependencies = model.build_dependencies();
        assert!(!dependencies.circular.is_empty());
        let away_from_the_cycle = [(0, 1, 1)];

        // The cycle and D1 are 3 cells or more, so a cap of 1 overflows.
        assert!(Model::edits_reach_cycles(
            &mut dependencies,
            &away_from_the_cycle,
            &[],
            1
        ));
        assert!(matches!(dependencies.tainted, Taint::TooMany));

        // Remembered: an edit away from the cycle is still evaluated in full, even with room.
        assert!(Model::edits_reach_cycles(
            &mut dependencies,
            &away_from_the_cycle,
            &[],
            usize::MAX
        ));
        assert!(matches!(dependencies.tainted, Taint::TooMany));

        let mut rebuilt = model.build_dependencies();
        assert!(!Model::edits_reach_cycles(
            &mut rebuilt,
            &away_from_the_cycle,
            &[],
            usize::MAX
        ));
        assert!(matches!(rebuilt.tainted, Taint::Cells(_)));
    }
}
