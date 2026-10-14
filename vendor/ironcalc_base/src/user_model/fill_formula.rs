use crate::{
    constants::{LAST_COLUMN, LAST_ROW},
    expressions::{parser::static_analysis::StaticResult, parser::Node, types::Area},
    model::Model,
    types::Cell,
    user_model::history::Diff,
    UserModel,
};

fn resolve(absolute: bool, value: i32, origin: i32) -> i32 {
    if absolute {
        value
    } else {
        value.saturating_add(origin)
    }
}

// Whether `node`, written at (row, column), reads only cells of the sheet and keeps every
// range in order. Anything else is stored differently once it is written as text and
// parsed again, so it is not filled by sharing the formula.
fn is_stored_unchanged_at(node: &Node, row: i32, column: i32) -> bool {
    let in_sheet = |row: i32, column: i32| {
        (1..=LAST_ROW).contains(&row) && (1..=LAST_COLUMN).contains(&column)
    };
    match node {
        Node::BooleanKind(_)
        | Node::NumberKind(_)
        | Node::StringKind(_)
        | Node::ErrorKind(_)
        | Node::EmptyArgKind => true,
        Node::ReferenceKind {
            absolute_row,
            absolute_column,
            row: target_row,
            column: target_column,
            ..
        } => in_sheet(
            resolve(*absolute_row, *target_row, row),
            resolve(*absolute_column, *target_column, column),
        ),
        Node::RangeKind {
            absolute_row1,
            absolute_column1,
            row1,
            column1,
            absolute_row2,
            absolute_column2,
            row2,
            column2,
            ..
        } => {
            let (first_row, first_column) = (
                resolve(*absolute_row1, *row1, row),
                resolve(*absolute_column1, *column1, column),
            );
            let (last_row, last_column) = (
                resolve(*absolute_row2, *row2, row),
                resolve(*absolute_column2, *column2, column),
            );
            in_sheet(first_row, first_column)
                && in_sheet(last_row, last_column)
                && first_row <= last_row
                && first_column <= last_column
        }
        Node::OpConcatenateKind { left, right }
        | Node::OpSumKind { left, right, .. }
        | Node::OpProductKind { left, right, .. }
        | Node::OpPowerKind { left, right }
        | Node::CompareKind { left, right, .. } => {
            is_stored_unchanged_at(left, row, column) && is_stored_unchanged_at(right, row, column)
        }
        Node::UnaryKind { right, .. } => is_stored_unchanged_at(right, row, column),
        Node::FunctionKind { args, .. } => args
            .iter()
            .all(|arg| is_stored_unchanged_at(arg, row, column)),
        _ => false,
    }
}

impl Model<'_> {
    // The formula index of the cell at `source` when every cell of `target` can hold it as is.
    fn shareable_formula(&self, sheet: u32, source: (i32, i32), target: &Area) -> Option<i32> {
        let Some(Cell::CellFormula { f, .. }) = self
            .workbook
            .worksheet(sheet)
            .ok()?
            .cell(source.0, source.1)
        else {
            return None;
        };
        let (node, static_result) = self.parsed_formulas.get(sheet as usize)?.get(*f as usize)?;
        if *static_result != StaticResult::Scalar {
            return None;
        }
        let last_row = target.row + target.height - 1;
        let last_column = target.column + target.width - 1;
        let corners = [
            (target.row, target.column),
            (target.row, last_column),
            (last_row, target.column),
            (last_row, last_column),
        ];
        corners
            .iter()
            .all(|(row, column)| is_stored_unchanged_at(node, *row, *column))
            .then_some(*f)
    }
}

impl UserModel<'_> {
    /// Fills `target` with the formula of the cell at `source`, references shifted as when
    /// the formula is copied, as one undo step. Returns false, changing nothing, when the
    /// cells have to be filled as text: the source is not a plain formula, a reference would
    /// leave the sheet or invert a range, or the target holds an array or a spill.
    pub fn fill_formula(
        &mut self,
        source: (i32, i32),
        target: &Area,
    ) -> Result<bool, String> {
        let Some(old_values) = self.write_filled_formula(source, target)? else {
            return Ok(false);
        };
        self.push_diff_list(vec![Diff::FillFormula {
            sheet: target.sheet,
            source_row: source.0,
            source_column: source.1,
            row: target.row,
            column: target.column,
            width: target.width,
            height: target.height,
            old_values,
        }]);
        self.set_selected_range(
            target.row,
            target.column,
            target.row + target.height - 1,
            target.column + target.width - 1,
        )?;
        self.evaluate_if_not_paused();
        Ok(true)
    }

    // The cells of `target` as they were, or None when they cannot be filled by sharing.
    pub(super) fn write_filled_formula(
        &mut self,
        source: (i32, i32),
        target: &Area,
    ) -> Result<Option<Vec<Option<Cell>>>, String> {
        let sheet = target.sheet;
        let fits = target.width > 0
            && target.height > 0
            && target.row >= 1
            && target.column >= 1
            && target.row.saturating_add(target.height - 1) <= LAST_ROW
            && target.column.saturating_add(target.width - 1) <= LAST_COLUMN;
        let source_inside = (target.row..target.row + target.height).contains(&source.0)
            && (target.column..target.column + target.width).contains(&source.1);
        if !fits || source_inside {
            return Ok(None);
        }
        let Some(formula_index) = self.model.shareable_formula(sheet, source, target) else {
            return Ok(None);
        };
        let worksheet = self.model.workbook.worksheet(sheet)?;
        let mut old_values = Vec::with_capacity(target.width as usize * target.height as usize);
        for row in target.row..target.row + target.height {
            for column in target.column..target.column + target.width {
                let old = worksheet.cell(row, column);
                if matches!(
                    old,
                    Some(Cell::ArrayFormula(_)) | Some(Cell::SpillCell(_))
                ) {
                    return Ok(None);
                }
                old_values.push(old.cloned());
            }
        }
        let cells = (target.row..target.row + target.height).flat_map(|row| {
            (target.column..target.column + target.width).map(move |column| (row, column))
        });
        for (row, column) in cells {
            if let Err(error) = self
                .model
                .set_user_formula_index(sheet, row, column, formula_index)
            {
                self.restore_cells(target, &old_values)?;
                return Err(error);
            }
        }
        Ok(Some(old_values))
    }

    // Puts back the cells of `area`, given row by row.
    pub(super) fn restore_cells(
        &mut self,
        area: &Area,
        old_values: &[Option<Cell>],
    ) -> Result<(), String> {
        let worksheet = self.model.workbook.worksheet_mut(area.sheet)?;
        let cells = (area.row..area.row + area.height).flat_map(|row| {
            (area.column..area.column + area.width).map(move |column| (row, column))
        });
        for ((row, column), old) in cells.zip(old_values) {
            match old {
                Some(cell) => worksheet.update_cell(row, column, cell.clone())?,
                None => worksheet.cell_clear_contents(row, column)?,
            }
        }
        Ok(())
    }
}
