use std::borrow::Cow;

use crate::constants::{LAST_COLUMN, LAST_ROW};
use crate::criteria_ranges::AreaValues;
use crate::criteria_totals::TotalKey;
use crate::exact_sum::ExactSum;
use crate::expressions::types::CellReferenceIndex;
use crate::functions::util::{build_criteria, Criterion};
use crate::{
    calc_result::{CalcResult, Range},
    expressions::parser::{ArrayNode, Node},
    expressions::token::Error,
    model::Model,
};

// Totals are kept only for ranges of one shape: the full read takes the others in the
// shape of the first, which Excel does not do.
fn same_shape<'r>(mut ranges: impl Iterator<Item = &'r Range>) -> bool {
    let shape = |range: &Range| {
        (
            range.right.row - range.left.row,
            range.right.column - range.left.column,
        )
    };
    match ranges.next() {
        Some(first) => ranges.all(|range| shape(range) == shape(first)),
        None => false,
    }
}

/// Converts a single array element into the equivalent scalar `CalcResult`,
/// used to feed `build_criteria` from an inline-array criteria argument.
fn array_node_to_calc_result(node: &ArrayNode, cell: CellReferenceIndex) -> CalcResult {
    match node {
        ArrayNode::Number(n) => CalcResult::Number(*n),
        ArrayNode::Boolean(b) => CalcResult::Boolean(*b),
        ArrayNode::String(s) => CalcResult::String(s.clone()),
        ArrayNode::Error(e) => CalcResult::Error {
            error: e.clone(),
            origin: cell,
            message: "".to_string(),
        },
        ArrayNode::Empty => CalcResult::EmptyCell,
    }
}

impl<'a> Model<'a> {
    pub(crate) fn fn_countif(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() == 2 {
            let arguments = vec![args[0].clone(), args[1].clone()];
            self.fn_countifs(&arguments, cell)
        } else {
            CalcResult::new_args_number_error(cell)
        }
    }

    /// AVERAGEIF(criteria_range, criteria, [average_range])
    /// if average_rage is missing then criteria_range will be used
    pub(crate) fn fn_averageif(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() == 2 {
            let arguments = vec![args[0].clone(), args[0].clone(), args[1].clone()];
            self.fn_averageifs(&arguments, cell)
        } else if args.len() == 3 {
            let arguments = vec![args[2].clone(), args[0].clone(), args[1].clone()];
            self.fn_averageifs(&arguments, cell)
        } else {
            CalcResult::new_args_number_error(cell)
        }
    }

    // FIXME: This function shares a lot of code with apply_ifs. Can we merge them?
    pub(crate) fn fn_countifs(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        let args_count = args.len();
        if args_count < 2 || !args_count.is_multiple_of(2) {
            return CalcResult::new_args_number_error(cell);
        }

        let case_count = args_count / 2;
        // NB: this is a beautiful example of the borrow checker
        // The order of these two definitions cannot be swapped.
        let mut criteria = Vec::new();
        let mut fn_criteria = Vec::new();
        let ranges = &mut Vec::new();
        for case_index in 0..case_count {
            let criterion = self.evaluate_node_in_context(&args[case_index * 2 + 1], cell);
            criteria.push(criterion);
            // NB: We cannot do:
            // fn_criteria.push(build_criteria(&criterion));
            // because criterion doesn't live long enough
            let result = self.evaluate_node_in_context(&args[case_index * 2], cell);
            if result.is_error() {
                return result;
            }
            if let CalcResult::Range { left, right } = result {
                if left.sheet != right.sheet {
                    return CalcResult::new_error(
                        Error::VALUE,
                        cell,
                        "Ranges are in different sheets".to_string(),
                    );
                }
                // TODO test ranges are of the same size as sum_range
                ranges.push(Range { left, right });
            } else {
                return CalcResult::new_error(Error::VALUE, cell, "Expected a range".to_string());
            }
        }
        for criterion in criteria.iter() {
            fn_criteria.push(build_criteria(criterion, self.locale));
        }

        let first_range = &ranges[0];
        let left_row = first_range.left.row;
        let left_column = first_range.left.column;
        let right_row = first_range.right.row;
        let right_column = first_range.right.column;

        let worksheet = match self.workbook.worksheet(first_range.left.sheet) {
            Ok(s) => s,
            Err(_) => {
                return CalcResult::new_error(
                    Error::ERROR,
                    cell,
                    format!("Invalid worksheet index: '{}'", first_range.left.sheet),
                )
            }
        };
        let open_row = left_row == 1 && right_row == LAST_ROW;
        let open_column = left_column == 1 && right_column == LAST_COLUMN;
        // The used area takes a pass over every cell of the sheet, and only open ranges
        // need it.
        let (max_row, max_column) = if open_row || open_column {
            let dimension = worksheet.dimension();
            (dimension.max_row, dimension.max_column)
        } else {
            (right_row, right_column)
        };
        // Past the used area of an open range every cell is empty: counted once below.
        let last_row = if open_row { max_row } else { right_row };
        let last_column = if open_column {
            max_column
        } else {
            right_column
        };
        let height = (last_row - left_row + 1).max(0);
        let width = (last_column - left_column + 1).max(0);
        let empty_matches = fn_criteria
            .iter()
            .all(|fn_criterion| fn_criterion(&CalcResult::EmptyCell));

        let values: Vec<AreaValues> = ranges
            .iter()
            .map(|range| {
                self.area_values(
                    range.left.sheet,
                    range.left.row,
                    range.left.column,
                    height,
                    width,
                )
            })
            .collect();
        let key = if same_shape(ranges.iter()) {
            TotalKey::new(&values, &criteria, None)
        } else {
            None
        };
        let kept = key
            .as_ref()
            .and_then(|key| self.kept_total(key, &values, None, cell));
        let mut total = match kept {
            Some(count) => count,
            None => {
                let misses = self.criteria_ranges.misses;
                let mut count = 0.0;
                for row_offset in 0..height {
                    for column_offset in 0..width {
                        let mut is_true = true;
                        for (values, fn_criterion) in values.iter().zip(fn_criteria.iter()) {
                            // We check if value in range n meets criterion n
                            if !fn_criterion(&self.area_value(values, row_offset, column_offset)) {
                                is_true = false;
                                break;
                            }
                        }
                        if is_true {
                            count += 1.0;
                        }
                    }
                }
                if let Some(key) = key {
                    let mut sum = ExactSum::default();
                    sum.add(count);
                    self.keep_total(key, sum, misses, cell);
                }
                count
            }
        };
        if open_column && right_column > max_column && empty_matches {
            total += f64::from(LAST_COLUMN - max_column) * f64::from(height);
        }
        if open_row && right_row > max_row && empty_matches {
            // In f64: a whole sheet holds more cells than an i32 counts.
            total += f64::from(LAST_ROW - max_row) * f64::from(right_column - left_column + 1);
        }
        CalcResult::Number(total)
    }

    // The sum range, the criteria ranges and the criteria of SUMIFS and the functions
    // like it.
    fn ifs_arguments(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> Result<(Range, Vec<Range>, Vec<CalcResult>), CalcResult> {
        let args_count = args.len();
        if args_count < 3 || args_count.is_multiple_of(2) {
            return Err(CalcResult::new_args_number_error(cell));
        }
        let arg_0 = self.evaluate_node_in_context(&args[0], cell);
        if arg_0.is_error() {
            return Err(arg_0);
        }
        let sum_range = if let CalcResult::Range { left, right } = arg_0 {
            if left.sheet != right.sheet {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Ranges are in different sheets".to_string(),
                ));
            }
            Range { left, right }
        } else {
            return Err(CalcResult::new_error(
                Error::VALUE,
                cell,
                "Expected a range".to_string(),
            ));
        };

        let case_count = (args_count - 1) / 2;
        let mut criteria = Vec::new();
        let mut ranges = Vec::new();
        for case_index in 1..=case_count {
            let criterion = self.evaluate_node_in_context(&args[case_index * 2], cell);
            // NB: criterion might be an error. That's ok
            criteria.push(criterion);
            let result = self.evaluate_node_in_context(&args[case_index * 2 - 1], cell);
            if result.is_error() {
                return Err(result);
            }
            if let CalcResult::Range { left, right } = result {
                if left.sheet != right.sheet {
                    return Err(CalcResult::new_error(
                        Error::VALUE,
                        cell,
                        "Ranges are in different sheets".to_string(),
                    ));
                }
                // TODO test ranges are of the same size as sum_range
                ranges.push(Range { left, right });
            } else {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Expected a range".to_string(),
                ));
            }
        }
        Ok((sum_range, ranges, criteria))
    }

    pub(crate) fn apply_ifs<F>(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
        apply: F,
    ) -> Result<(), CalcResult>
    where
        F: FnMut(f64),
    {
        let (sum_range, ranges, criteria) = self.ifs_arguments(args, cell)?;
        let fn_criteria: Vec<Criterion<'_>> = criteria
            .iter()
            .map(|criterion| build_criteria(criterion, self.locale))
            .collect();
        self.run_ifs(&sum_range, &ranges, &fn_criteria, cell, apply)
    }

    /// SUMIFS, from the kept total when there is one.
    pub(crate) fn sum_ifs(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        let (sum_range, ranges, criteria) = match self.ifs_arguments(args, cell) {
            Ok(arguments) => arguments,
            Err(error) => return error,
        };
        let fn_criteria: Vec<Criterion<'_>> = criteria
            .iter()
            .map(|criterion| build_criteria(criterion, self.locale))
            .collect();
        let (sums, values) = match self.ifs_areas(&sum_range, &ranges, cell) {
            Ok(areas) => areas,
            Err(error) => return error,
        };
        let key = if same_shape(std::iter::once(&sum_range).chain(&ranges)) {
            TotalKey::new(&values, &criteria, Some(&sums))
        } else {
            None
        };
        if let Some(total) = key
            .as_ref()
            .and_then(|key| self.kept_total(key, &values, Some(&sums), cell))
        {
            return CalcResult::Number(total);
        }
        let misses = self.criteria_ranges.misses;
        let mut total = ExactSum::default();
        if let Err(error) = self.scan_ifs(&sums, &values, &fn_criteria, |value| total.add(value)) {
            return error;
        }
        let value = total.value();
        if let Some(key) = key {
            self.keep_total(key, total, misses, cell);
        }
        CalcResult::Number(value)
    }

    /// Walks `sum_range` and applies `apply` to every numeric cell whose parallel
    /// cell in each criteria range satisfies the matching criterion. `ranges` and
    /// `fn_criteria` are parallel (one criteria range and one predicate per case).
    ///
    /// Shared by [`Model::apply_ifs`] and the array-criteria path of SUMIF.
    pub(crate) fn run_ifs<F>(
        &mut self,
        sum_range: &Range,
        ranges: &[Range],
        fn_criteria: &[Criterion<'_>],
        cell: CellReferenceIndex,
        apply: F,
    ) -> Result<(), CalcResult>
    where
        F: FnMut(f64),
    {
        let (sums, values) = self.ifs_areas(sum_range, ranges, cell)?;
        self.scan_ifs(&sums, &values, fn_criteria, apply)
    }

    // The sum area and the criteria areas, all in the shape of the sum range.
    fn ifs_areas(
        &mut self,
        sum_range: &Range,
        ranges: &[Range],
        cell: CellReferenceIndex,
    ) -> Result<(AreaValues, Vec<AreaValues>), CalcResult> {
        let left_row = sum_range.left.row;
        let left_column = sum_range.left.column;
        let mut right_row = sum_range.right.row;
        let mut right_column = sum_range.right.column;

        if left_row == 1 && right_row == LAST_ROW {
            right_row = match self.workbook.worksheet(sum_range.left.sheet) {
                Ok(s) => s.dimension().max_row,
                Err(_) => {
                    return Err(CalcResult::new_error(
                        Error::ERROR,
                        cell,
                        format!("Invalid worksheet index: '{}'", sum_range.left.sheet),
                    ));
                }
            };
        }
        if left_column == 1 && right_column == LAST_COLUMN {
            right_column = match self.workbook.worksheet(sum_range.left.sheet) {
                Ok(s) => s.dimension().max_column,
                Err(_) => {
                    return Err(CalcResult::new_error(
                        Error::ERROR,
                        cell,
                        format!("Invalid worksheet index: '{}'", sum_range.left.sheet),
                    ));
                }
            };
        }

        let height = right_row - left_row + 1;
        let width = right_column - left_column + 1;
        let sums = self.area_values(sum_range.left.sheet, left_row, left_column, height, width);
        let values: Vec<AreaValues> = ranges
            .iter()
            .map(|range| {
                self.area_values(
                    range.left.sheet,
                    range.left.row,
                    range.left.column,
                    height,
                    width,
                )
            })
            .collect();
        Ok((sums, values))
    }

    fn scan_ifs<F>(
        &mut self,
        sums: &AreaValues,
        values: &[AreaValues],
        fn_criteria: &[Criterion<'_>],
        mut apply: F,
    ) -> Result<(), CalcResult>
    where
        F: FnMut(f64),
    {
        let (_, _, _, height, width) = sums.area;
        for row_offset in 0..height {
            for column_offset in 0..width {
                let mut is_true = true;
                for (values, fn_criterion) in values.iter().zip(fn_criteria.iter()) {
                    // We check if value in range n meets criterion n
                    if !fn_criterion(&self.area_value(values, row_offset, column_offset)) {
                        is_true = false;
                        break;
                    }
                }
                if is_true {
                    match self.area_value(sums, row_offset, column_offset) {
                        Cow::Borrowed(CalcResult::Number(n)) => apply(*n),
                        Cow::Owned(CalcResult::Number(n)) => apply(n),
                        v if v.is_error() => return Err(v.into_owned()),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Evaluates `node` and requires it to be a single-sheet range.
    fn node_to_range(
        &mut self,
        node: &Node,
        cell: CellReferenceIndex,
    ) -> Result<Range, CalcResult> {
        let value = self.evaluate_node_in_context(node, cell);
        if value.is_error() {
            return Err(value);
        }
        if let CalcResult::Range { left, right } = value {
            if left.sheet != right.sheet {
                return Err(CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Ranges are in different sheets".to_string(),
                ));
            }
            Ok(Range { left, right })
        } else {
            Err(CalcResult::new_error(
                Error::VALUE,
                cell,
                "Expected a range".to_string(),
            ))
        }
    }

    /// SUMIF where the `criteria` argument may be a single value, a range or an
    /// array. A scalar criterion yields a single sum; a range or array criterion
    /// spills one sum per criterion element, preserving the criteria's shape.
    pub(crate) fn sumif(
        &mut self,
        criteria_range: &Node,
        criteria: &Node,
        sum_range: &Node,
        cell: CellReferenceIndex,
    ) -> CalcResult {
        // Collect the criteria into a 2-D grid of scalar values. A scalar
        // criterion takes the ordinary (non-spilling) SUMIFS path.
        let criteria_grid: Vec<Vec<CalcResult>> = match self
            .evaluate_node_in_context(criteria, cell)
        {
            CalcResult::Range { left, right } => {
                if left.sheet != right.sheet {
                    return CalcResult::new_error(
                        Error::VALUE,
                        cell,
                        "Ranges are in different sheets".to_string(),
                    );
                }
                let mut grid = Vec::new();
                for r in left.row..=right.row {
                    let mut row = Vec::new();
                    for c in left.column..=right.column {
                        row.push(self.evaluate_cell(CellReferenceIndex {
                            sheet: left.sheet,
                            row: r,
                            column: c,
                        }));
                    }
                    grid.push(row);
                }
                grid
            }
            CalcResult::Array(array) => array
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|node| array_node_to_calc_result(node, cell))
                        .collect()
                })
                .collect(),
            _ => {
                // Single criterion: delegate to SUMIFS, which returns a scalar.
                let arguments = vec![sum_range.clone(), criteria_range.clone(), criteria.clone()];
                return self.fn_sumifs(&arguments, cell);
            }
        };

        // Range/array criteria: resolve the ranges once, then compute one sum
        // per criterion and spill the results.
        let sum_range = match self.node_to_range(sum_range, cell) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let criteria_range = match self.node_to_range(criteria_range, cell) {
            Ok(r) => r,
            Err(e) => return e,
        };

        let mut output: Vec<Vec<ArrayNode>> = Vec::with_capacity(criteria_grid.len());
        for criteria_row in &criteria_grid {
            let mut out_row: Vec<ArrayNode> = Vec::with_capacity(criteria_row.len());
            for criterion in criteria_row {
                let fn_criteria = [build_criteria(criterion, self.locale)];
                let mut total = ExactSum::default();
                let node = match self.run_ifs(
                    &sum_range,
                    std::slice::from_ref(&criteria_range),
                    &fn_criteria,
                    cell,
                    |v| total.add(v),
                ) {
                    Ok(()) => ArrayNode::Number(total.value()),
                    Err(CalcResult::Error { error, .. }) => ArrayNode::Error(error),
                    Err(_) => ArrayNode::Error(Error::ERROR),
                };
                out_row.push(node);
            }
            output.push(out_row);
        }
        CalcResult::Array(output)
    }

    pub(crate) fn fn_averageifs(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        let mut total = 0.0;
        let mut count = 0.0;

        let average = |value: f64| {
            total += value;
            count += 1.0;
        };
        if let Err(e) = self.apply_ifs(args, cell, average) {
            return e;
        }

        if count == 0.0 {
            return CalcResult::Error {
                error: Error::DIV,
                origin: cell,
                message: "division by 0".to_string(),
            };
        }
        CalcResult::Number(total / count)
    }

    pub(crate) fn fn_minifs(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        let mut min = f64::INFINITY;
        let apply_min = |value: f64| min = value.min(min);
        if let Err(e) = self.apply_ifs(args, cell, apply_min) {
            return e;
        }

        if min.is_infinite() {
            min = 0.0;
        }
        CalcResult::Number(min)
    }

    pub(crate) fn fn_maxifs(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        let mut max = -f64::INFINITY;
        let apply_max = |value: f64| max = value.max(max);
        if let Err(e) = self.apply_ifs(args, cell, apply_max) {
            return e;
        }
        if max.is_infinite() {
            max = 0.0;
        }
        CalcResult::Number(max)
    }
}
