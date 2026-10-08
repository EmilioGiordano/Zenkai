use crate::{
    cell::CellValue,
    dependency_index::{CellKey, DependencyIndex},
    expressions::{parser::Node, types::CellReferenceIndex},
    model::Model,
    types::{ArrayKind, Cell},
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
            && (anchor_row..anchor_row + height).contains(&row)
            && (anchor_column..anchor_column + width).contains(&column)
    }
}

pub(crate) struct Dependencies {
    index: DependencyIndex,
    spills: Vec<Spill>,
}

fn reference((sheet, row, column): CellKey) -> CellReferenceIndex {
    CellReferenceIndex { sheet, row, column }
}

impl Model<'_> {
    pub(crate) fn evaluate_indexed(&mut self) {
        self.evaluate();
        // The result of a circular reference depends on the order formulas are visited
        // in, which only a full evaluation reproduces.
        if self.circular_hits == 0 {
            self.dependencies = Some(Box::new(self.build_dependencies()));
        }
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
            }
        }
        dependencies.index.sort();

        let mut dirty: Vec<CellKey> = dependencies
            .index
            .affected_by(edited)
            .into_iter()
            .filter(|cell| self.formula_node(*cell).is_some())
            .collect();
        // The order of a full evaluation, so recursion stays as shallow as there.
        dirty.sort_unstable();
        if dirty.iter().any(|cell| self.is_array_formula(*cell)) {
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
        for cell in &dirty {
            self.evaluate_cell(reference(*cell));
        }
        // A formula that now spills writes into cells nothing was watching.
        if self.circular_hits != circular_hits
            || dirty.iter().any(|cell| self.is_array_formula(*cell))
        {
            self.evaluate_indexed();
            return;
        }
        self.evaluate_conditional_formatting();
        self.dependencies = Some(dependencies);
        self.last_recalculation = Recalculation::Incremental;
    }

    /// How the last evaluation was done.
    pub fn last_recalculation(&self) -> Recalculation {
        self.last_recalculation
    }

    fn build_dependencies(&self) -> Dependencies {
        let mut index = DependencyIndex::new();
        let mut spills = Vec::new();
        for (sheet, worksheet) in (0u32..).zip(&self.workbook.worksheets) {
            for (row, cells) in &worksheet.sheet_data {
                for (column, cell) in cells {
                    let key = (sheet, *row, *column);
                    if let Some(node) = self.formula_node(key) {
                        index.add(key, node);
                    }
                    if let Cell::ArrayFormula {
                        kind: ArrayKind::Dynamic,
                        r,
                        ..
                    } = cell
                    {
                        spills.push(Spill {
                            anchor: key,
                            size: *r,
                        });
                    }
                }
            }
        }
        index.sort();
        Dependencies { index, spills }
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
            Some(Cell::ArrayFormula {
                kind: ArrayKind::Dynamic,
                r,
                ..
            }) => Some(*r),
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
