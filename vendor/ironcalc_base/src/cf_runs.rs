use std::collections::{HashMap, HashSet};

use crate::{
    cf_types::{CfCellResult, CfRule, Cfvo},
    conditional_formatting::parse_sqref,
    dependency_index::{
        precedent_rects, precedents_by_offset, CellKey, OffsetPrecedent, PrecedentRect,
    },
    expressions::{parser::Node, types::CellReferenceRC},
    Model,
};

// (first row, first column, last row, last column)
pub(crate) type Rect = (i32, i32, i32, i32);

// A rule updated for a list of cells costs a rectangle of 16 bytes per range of the rule
// holding the cell and one evaluation of the rule per cell, as a whole pass does; past
// this the rule is evaluated over its whole range instead of listing the cells.
const MAX_PARTIAL_CELLS: usize = 4096;

// What decides the result of a rule at a cell.
enum Scope {
    // The cell alone, and the formulas the rule is given.
    Cell,
    // Every cell of the range, as a minimum, a mean, a rank or a count of duplicates does.
    Range,
    // The cells the formula reads, wherever they are.
    Formula,
}

fn scope(rule: &CfRule) -> Scope {
    match rule {
        CfRule::CellIs { .. }
        | CfRule::Text { .. }
        | CfRule::TimePeriod { .. }
        | CfRule::Blanks { .. }
        | CfRule::NotBlanks { .. }
        | CfRule::Errors { .. }
        | CfRule::NoErrors { .. } => Scope::Cell,
        CfRule::Formula { .. } => Scope::Formula,
        CfRule::ColorScale { .. }
        | CfRule::DataBar { .. }
        | CfRule::IconSet { .. }
        | CfRule::IconRating { .. }
        | CfRule::AboveAverage { .. }
        | CfRule::BelowAverage { .. }
        | CfRule::Top10 { .. }
        | CfRule::Bottom10 { .. }
        | CfRule::DuplicateValues { .. }
        | CfRule::UniqueValues { .. } => Scope::Range,
    }
}

// The cells outside its range a rule reads.
enum Reads {
    Nothing,
    // A volatile formula reads what it cannot tell beforehand.
    Always,
    Cells(Vec<PrecedentRect>),
}

// How the references of a formula rule shift from one cell to the next, to tell which cells
// read an edited one.
struct Readers {
    anchor: (i32, i32),
    precedents: Vec<OffsetPrecedent>,
}

enum Refresh {
    Nothing,
    Cells(Vec<(i32, i32)>),
    Whole,
}

// What one rule of a sheet gave to its cells, kept apart so a rule can be evaluated again
// without the others. A cell holds its results in the order the rule produced them, each
// with whether the rule stops the lower priority ones.
pub(crate) struct CfRun {
    rule: CfRule,
    ranges: Vec<Rect>,
    reads: Reads,
    readers: Option<Readers>,
    // The day of the last evaluation, for the rules that depend on today.
    day: Option<i64>,
    results: HashMap<(i32, i32), Vec<(CfCellResult, bool)>>,
}

fn inside((row1, column1, row2, column2): Rect, row: i32, column: i32) -> bool {
    (row1..=row2).contains(&row) && (column1..=column2).contains(&column)
}

fn intersection(
    (row1, column1, row2, column2): Rect,
    (other_row1, other_column1, other_row2, other_column2): Rect,
) -> Option<Rect> {
    let rect = (
        row1.max(other_row1),
        column1.max(other_column1),
        row2.min(other_row2),
        column2.min(other_column2),
    );
    (rect.0 <= rect.2 && rect.1 <= rect.3).then_some(rect)
}

fn formula_body(formula: &str) -> Option<&str> {
    let body = formula.trim().strip_prefix('=').unwrap_or(formula.trim());
    (!body.is_empty()).then_some(body)
}

fn formula_of(cfvo: &Cfvo) -> Option<&str> {
    match cfvo {
        Cfvo::Formula(formula) => Some(formula.as_str()),
        _ => None,
    }
}

// The formulas a rule evaluates once, apart from the one of a formula rule.
fn threshold_formulas(rule: &CfRule) -> Vec<&str> {
    match rule {
        CfRule::CellIs {
            formula, formula2, ..
        } => std::iter::once(formula.as_str())
            .chain(formula2.as_deref())
            .collect(),
        CfRule::ColorScale { thresholds } => thresholds
            .iter()
            .filter_map(|threshold| formula_of(&threshold.cfvo))
            .collect(),
        CfRule::DataBar { min, max, .. } => min.iter().chain(max).filter_map(formula_of).collect(),
        CfRule::IconSet { thresholds, .. } => thresholds
            .iter()
            .filter_map(|threshold| formula_of(&threshold.cfvo))
            .collect(),
        CfRule::IconRating { thresholds, .. } => thresholds
            .iter()
            .filter_map(|(cfvo, _)| formula_of(cfvo))
            .collect(),
        _ => Vec::new(),
    }
}

fn reads_of(rects: Vec<PrecedentRect>) -> Reads {
    if rects.is_empty() {
        Reads::Nothing
    } else {
        Reads::Cells(rects)
    }
}

impl CfRun {
    fn reads_changed(&self, changed: &HashSet<CellKey>) -> bool {
        match &self.reads {
            Reads::Nothing => false,
            Reads::Always => true,
            Reads::Cells(rects) => changed.iter().any(|&(sheet, row, column)| {
                rects
                    .iter()
                    .any(|&(rect_sheet, row1, column1, row2, column2)| {
                        rect_sheet == sheet && inside((row1, column1, row2, column2), row, column)
                    })
            }),
        }
    }

    fn cells_changed_inside(&self, changed: &[(i32, i32)]) -> Vec<(i32, i32)> {
        changed
            .iter()
            .copied()
            .filter(|&(row, column)| self.ranges.iter().any(|&rect| inside(rect, row, column)))
            .collect()
    }

    // The cells of the range whose formula reads one of the `changed` cells.
    fn cells_reading(&self, readers: &Readers, changed: &HashSet<CellKey>) -> Refresh {
        let mut cells: HashSet<(i32, i32)> = HashSet::new();
        for &cell in changed {
            for precedent in &readers.precedents {
                let Some(area) = precedent.readers_of(cell, readers.anchor) else {
                    continue;
                };
                for &rect in &self.ranges {
                    let Some((row1, column1, row2, column2)) = intersection(area, rect) else {
                        continue;
                    };
                    let size = (i64::from(row2) - i64::from(row1) + 1)
                        * (i64::from(column2) - i64::from(column1) + 1);
                    let room = MAX_PARTIAL_CELLS.saturating_sub(cells.len());
                    if !usize::try_from(size).is_ok_and(|size| size <= room) {
                        return Refresh::Whole;
                    }
                    for row in row1..=row2 {
                        for column in column1..=column2 {
                            cells.insert((row, column));
                        }
                    }
                }
            }
        }
        Refresh::Cells(cells.into_iter().collect())
    }
}

impl Model<'_> {
    /// Evaluates all conditional formatting rules for the workbook.
    ///
    /// Iterates every worksheet's CF rules in priority order (lowest priority number,
    /// processed last). What each rule gives to its cells is kept apart, in
    /// `cf_runs`, and merged in priority order by `get_extended_style_for_cell`.
    pub fn evaluate_conditional_formatting(&mut self) {
        let today = self.cf_today();
        let sheet_count = self.workbook.worksheets.len();
        let mut all_runs = Vec::with_capacity(sheet_count);
        for sheet_idx in 0..sheet_count {
            let mut cfs = self.workbook.worksheets[sheet_idx]
                .conditional_formatting
                .clone();
            // Lower priority number = higher priority; process high-priority first so that
            // the first writer into the cell wins.
            cfs.sort_by_key(|cf| cf.priority);
            let sheet = sheet_idx as u32;
            let mut runs = Vec::new();
            for cf in cfs {
                let ranges = parse_sqref(&cf.range);
                if ranges.is_empty() {
                    continue;
                }
                let (reads, readers) = self.cf_reads(sheet, &cf.cf_rule, &ranges);
                let mut run = CfRun {
                    rule: cf.cf_rule,
                    ranges,
                    reads,
                    readers,
                    day: None,
                    results: HashMap::new(),
                };
                self.evaluate_cf_run(sheet, &mut run, today);
                runs.push(run);
            }
            all_runs.push(runs);
        }
        self.cf_runs = all_runs;
    }

    // Evaluates again only the rules the `changed` cells can change, and in them only the
    // cells they can change when the rule looks at one cell at a time. The result is the one
    // `evaluate_conditional_formatting` gives, provided nothing but the `changed` cells
    // changed since it ran.
    pub(crate) fn update_conditional_formatting(&mut self, changed: &HashSet<CellKey>) {
        let today = self.cf_today();
        let mut all_runs = std::mem::take(&mut self.cf_runs);
        for (sheet, runs) in all_runs.iter_mut().enumerate() {
            if runs.is_empty() {
                continue;
            }
            let sheet = sheet as u32;
            let on_sheet: Vec<(i32, i32)> = changed
                .iter()
                .filter(|(changed_sheet, _, _)| *changed_sheet == sheet)
                .map(|&(_, row, column)| (row, column))
                .collect();
            for run in runs.iter_mut() {
                self.refresh_cf_run(sheet, run, changed, &on_sheet, today);
            }
        }
        self.cf_runs = all_runs;
    }

    fn refresh_cf_run(
        &mut self,
        sheet: u32,
        run: &mut CfRun,
        changed: &HashSet<CellKey>,
        on_sheet: &[(i32, i32)],
        today: Option<i64>,
    ) {
        let stale_day = matches!(run.rule, CfRule::TimePeriod { .. }) && run.day != today;
        if stale_day || matches!(run.reads, Reads::Always) {
            self.evaluate_cf_run(sheet, run, today);
            return;
        }
        let refresh = match scope(&run.rule) {
            Scope::Range => {
                if run.reads_changed(changed) || !run.cells_changed_inside(on_sheet).is_empty() {
                    Refresh::Whole
                } else {
                    Refresh::Nothing
                }
            }
            Scope::Cell => {
                if run.reads_changed(changed) {
                    Refresh::Whole
                } else {
                    let own = run.cells_changed_inside(on_sheet);
                    if own.is_empty() {
                        Refresh::Nothing
                    } else {
                        Refresh::Cells(own)
                    }
                }
            }
            Scope::Formula => match &run.readers {
                Some(readers) => run.cells_reading(readers, changed),
                None if run.reads_changed(changed) => Refresh::Whole,
                None => Refresh::Nothing,
            },
        };
        match refresh {
            Refresh::Nothing => {}
            Refresh::Cells(cells) if cells.len() <= MAX_PARTIAL_CELLS => {
                self.evaluate_cf_cells(sheet, run, &cells);
            }
            Refresh::Cells(_) | Refresh::Whole => self.evaluate_cf_run(sheet, run, today),
        }
    }

    fn evaluate_cf_run(&mut self, sheet: u32, run: &mut CfRun, today: Option<i64>) {
        run.results = HashMap::new();
        run.day = today;
        self.cf_sink.clear();
        self.apply_cf_rule(sheet, &run.rule, &run.ranges, &run.ranges);
        self.keep_cf_results(run);
    }

    // A cell covered by several areas of the range is visited once per area, as a whole
    // pass does, so that it gets the same results.
    fn evaluate_cf_cells(&mut self, sheet: u32, run: &mut CfRun, cells: &[(i32, i32)]) {
        let mut visits = Vec::with_capacity(cells.len());
        for &(row, column) in cells {
            run.results.remove(&(row, column));
            for &rect in &run.ranges {
                if inside(rect, row, column) {
                    visits.push((row, column, row, column));
                }
            }
        }
        self.cf_sink.clear();
        self.apply_cf_rule(sheet, &run.rule, &visits, &run.ranges);
        self.keep_cf_results(run);
    }

    fn keep_cf_results(&mut self, run: &mut CfRun) {
        for (row, column, result, stop_if_true) in self.cf_sink.drain(..) {
            run.results
                .entry((row, column))
                .or_default()
                .push((result, stop_if_true));
        }
    }

    pub(crate) fn cf_today(&self) -> Option<i64> {
        crate::tz::excel_serial_for_now(&self.tz).map(|serial| serial.floor() as i64)
    }

    // The results of the rules of a sheet at a cell in priority order, where a rule that
    // stops the others leaves only its own and the ones after it.
    pub(crate) fn cf_results(&self, sheet: u32, row: i32, column: i32) -> Vec<&CfCellResult> {
        let mut merged = Vec::new();
        let Some(runs) = self.cf_runs.get(sheet as usize) else {
            return merged;
        };
        for run in runs {
            let Some(results) = run.results.get(&(row, column)) else {
                continue;
            };
            for (result, stop_if_true) in results {
                if *stop_if_true {
                    merged.clear();
                }
                merged.push(result);
            }
        }
        merged
    }

    fn parse_cf_formula(
        &mut self,
        sheet: u32,
        body: &str,
        (row, column): (i32, i32),
    ) -> Option<Node> {
        let sheet_name = self.workbook.worksheets.get(sheet as usize)?.get_name();
        let context = CellReferenceRC {
            sheet: sheet_name,
            row,
            column,
        };
        Some(self.parse_internal_formula(body, &context))
    }

    fn cf_reads(&mut self, sheet: u32, rule: &CfRule, ranges: &[Rect]) -> (Reads, Option<Readers>) {
        let CfRule::Formula { formula, .. } = rule else {
            return (self.threshold_reads(sheet, rule), None);
        };
        let Some(body) = formula_body(formula) else {
            return (Reads::Nothing, None);
        };
        // The same anchor `apply_cf_formula` parses at.
        let Some(anchor) = ranges
            .iter()
            .map(|&(row, column, _, _)| (row, column))
            .reduce(|(top, left), (row, column)| (top.min(row), left.min(column)))
        else {
            return (Reads::Nothing, None);
        };
        let far = ranges
            .iter()
            .map(|&(_, _, row, column)| (row, column))
            .fold(anchor, |(bottom, right), (row, column)| {
                (bottom.max(row), right.max(column))
            });
        let Some(node) = self.parse_cf_formula(sheet, body, anchor) else {
            return (Reads::Always, None);
        };
        let key = (sheet, anchor.0, anchor.1);
        if let Some(precedents) = precedents_by_offset(&node, key) {
            let shift = (far.0 - anchor.0, far.1 - anchor.1);
            let rects = precedents
                .iter()
                .map(|precedent| precedent.reach(shift))
                .collect();
            return (reads_of(rects), Some(Readers { anchor, precedents }));
        }
        match precedent_rects(&node, key, (sheet, far.0, far.1)) {
            Some(rects) => (reads_of(rects), None),
            None => (Reads::Always, None),
        }
    }

    // `evaluate_formula` reads these as if they were at A1.
    fn threshold_reads(&mut self, sheet: u32, rule: &CfRule) -> Reads {
        let mut rects = Vec::new();
        for formula in threshold_formulas(rule) {
            let Some(body) = formula_body(formula) else {
                continue;
            };
            let Some(node) = self.parse_cf_formula(sheet, body, (1, 1)) else {
                return Reads::Always;
            };
            match precedent_rects(&node, (sheet, 1, 1), (sheet, 1, 1)) {
                Some(found) => rects.extend(found),
                None => return Reads::Always,
            }
        }
        reads_of(rects)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashSet;

    use super::MAX_PARTIAL_CELLS;
    use crate::cf_types::{CfCellResult, CfRuleInput, PeriodType, TextOperator, ValueOperator};
    use crate::test::util::new_empty_model;
    use crate::types::Dxf;
    use crate::{Model, Recalculation};

    fn text_contains_a() -> CfRuleInput {
        CfRuleInput::Text {
            operator: TextOperator::Contains,
            value: "a".to_string(),
            format: Dxf::default(),
            stop_if_true: false,
        }
    }

    fn formula(formula: &str) -> CfRuleInput {
        CfRuleInput::Formula {
            formula: formula.to_string(),
            format: Dxf::default(),
            stop_if_true: false,
        }
    }

    // A model with the rule evaluated in full and a result planted at `row` of column A:
    // whether the plant survives an update tells whether the rule was evaluated there.
    fn planted(range: &str, rule: CfRuleInput, row: i32) -> Model<'static> {
        let mut model = new_empty_model();
        model.add_conditional_formatting(0, range, rule).unwrap();
        model.evaluate_indexed();
        model.cf_runs[0][0]
            .results
            .insert((row, 1), vec![(CfCellResult::Dxf(99), false)]);
        model
    }

    fn survives(model: &Model, row: i32) -> bool {
        model.cf_runs[0][0]
            .results
            .get(&(row, 1))
            .is_some_and(|results| matches!(results[0].0, CfCellResult::Dxf(99)))
    }

    fn is_matched(model: &Model, row: i32) -> bool {
        model.cf_runs[0][0]
            .results
            .get(&(row, 1))
            .is_some_and(|results| matches!(results[0].0, CfCellResult::Dxf(id) if id != 99))
    }

    fn edit(model: &mut Model, cell: &str, value: &str) {
        model._set(cell, value);
        let reference = model._parse_reference(cell);
        model.evaluate_incremental(&[(reference.sheet, reference.row, reference.column)]);
        assert_eq!(model.last_recalculation(), Recalculation::Incremental);
    }

    #[test]
    fn a_rule_that_looks_at_one_cell_is_evaluated_for_the_edited_cell_only() {
        let mut model = planted("A1:A10", text_contains_a(), 9);
        edit(&mut model, "A2", "a");
        assert!(survives(&model, 9));
        assert!(is_matched(&model, 2));
        edit(&mut model, "A9", "b");
        assert!(!survives(&model, 9));
        edit(&mut model, "A2", "b");
        assert!(!is_matched(&model, 2));
    }

    #[test]
    fn a_rule_over_the_whole_range_is_evaluated_when_a_cell_of_it_changes() {
        let rule = CfRuleInput::AboveAverage {
            format: Dxf::default(),
            stop_if_true: false,
        };
        let mut model = planted("A1:A10", rule, 9);
        edit(&mut model, "B1", "5");
        assert!(survives(&model, 9));
        edit(&mut model, "A1", "5");
        assert!(!survives(&model, 9));
    }

    #[test]
    fn a_formula_rule_is_evaluated_for_the_cells_that_read_the_edited_one() {
        let mut model = planted("A1:A10", formula("=$B1>3"), 9);
        edit(&mut model, "C1", "1");
        edit(&mut model, "A9", "1");
        assert!(survives(&model, 9));
        edit(&mut model, "B2", "5");
        assert!(survives(&model, 9));
        assert!(is_matched(&model, 2));
        edit(&mut model, "B9", "1");
        assert!(!survives(&model, 9));
    }

    #[test]
    fn a_volatile_formula_rule_is_evaluated_after_every_edit() {
        let mut model = planted("A1:A10", formula("=INDIRECT(\"C1\")>0"), 9);
        edit(&mut model, "D5", "1");
        assert!(!survives(&model, 9));
    }

    #[test]
    fn a_threshold_read_from_a_cell_is_evaluated_again_when_that_cell_changes() {
        let rule = CfRuleInput::CellIs {
            operator: ValueOperator::GreaterThan,
            formula: "=$C$1".to_string(),
            formula2: None,
            format: Dxf::default(),
            stop_if_true: false,
        };
        let mut model = planted("A1:A10", rule, 9);
        edit(&mut model, "A2", "5");
        assert!(survives(&model, 9));
        edit(&mut model, "C1", "3");
        assert!(!survives(&model, 9));
        assert!(is_matched(&model, 2));
    }

    #[test]
    fn a_rule_that_depends_on_today_is_evaluated_again_when_the_day_changes() {
        let rule = CfRuleInput::TimePeriod {
            time_period: PeriodType::Between,
            date1: Some("2020-01-01".to_string()),
            date2: Some("2020-12-31".to_string()),
            format: Dxf::default(),
            stop_if_true: false,
        };
        let mut model = planted("A1:A10", rule, 9);
        model.update_conditional_formatting(&HashSet::new());
        assert!(survives(&model, 9));
        model.cf_runs[0][0].day = Some(0);
        model.update_conditional_formatting(&HashSet::new());
        assert!(!survives(&model, 9));
    }

    #[test]
    fn a_list_of_cells_longer_than_the_limit_evaluates_the_whole_rule() {
        let last_row = i32::try_from(MAX_PARTIAL_CELLS).unwrap() + 904;
        for (edited, kept) in [(MAX_PARTIAL_CELLS, true), (MAX_PARTIAL_CELLS + 1, false)] {
            let mut model = planted(&format!("A1:A{last_row}"), text_contains_a(), last_row);
            let changed: HashSet<_> = (1..=i32::try_from(edited).unwrap())
                .map(|row| (0, row, 1))
                .collect();
            model.update_conditional_formatting(&changed);
            assert_eq!(survives(&model, last_row), kept, "{edited} cells");
        }
    }
}
