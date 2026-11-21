use crate::{
    calc_result::CalcResult,
    expressions::{token::Error, types::CellReferenceIndex},
    model::Model,
};

/// Which cells of a range a [`RangeCells`] walk gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RangeWalk {
    /// Every cell of the range, empty or not.
    Every,
    /// Only the cells the worksheet holds. A position without a cell evaluates as an empty
    /// cell and changes nothing, so a function an empty cell adds nothing to can skip it.
    Occupied,
}

/// The cells of a range in the order a loop over its rows and then its columns visits them.
/// The worksheet is read as the walk goes, so a cell that evaluating an earlier one writes,
/// such as a spill, is given as that loop would give it. A walk over `Occupied` cells costs
/// the cells it gives plus one look at each row of the range, and stops at the worksheet's
/// last row once the range has more rows left than the worksheet holds.
pub(crate) struct RangeCells {
    sheet: u32,
    first_column: i32,
    last_column: i32,
    last_row: i32,
    row: i32,
    column: i32,
    // The last row the worksheet held when last read. Reading it reads every row, so it is
    // read only once the range has more rows left than the worksheet holds, and again when
    // the walk passes it, as a spill may have written rows below it.
    worksheet_last_row: Option<i32>,
}

impl RangeCells {
    /// The cells from `left` to `right`, both on the same sheet, or the error of `cell` when
    /// that sheet does not exist.
    pub(crate) fn new(
        model: &Model,
        left: CellReferenceIndex,
        right: CellReferenceIndex,
        cell: CellReferenceIndex,
    ) -> Result<Self, CalcResult> {
        if model.workbook.worksheet(left.sheet).is_err() {
            return Err(CalcResult::new_error(
                Error::ERROR,
                cell,
                format!("Invalid worksheet index: '{}'", left.sheet),
            ));
        }
        Ok(RangeCells {
            sheet: left.sheet,
            first_column: left.column,
            last_column: right.column,
            last_row: right.row,
            row: left.row,
            column: left.column,
            worksheet_last_row: None,
        })
    }

    pub(crate) fn next(&mut self, model: &Model, walk: RangeWalk) -> Option<CellReferenceIndex> {
        while self.row <= self.last_row {
            let column = match walk {
                RangeWalk::Every => Some(self.column),
                RangeWalk::Occupied => {
                    let sheet_data = &model.workbook.worksheets[self.sheet as usize].sheet_data;
                    if self.worksheet_last_row.is_none()
                        && (self.last_row - self.row) as usize > sheet_data.row_count()
                    {
                        self.worksheet_last_row = Some(sheet_data.last_row()?);
                    }
                    if let Some(last_row) = self.worksheet_last_row {
                        if self.row > last_row {
                            let now = sheet_data.last_row()?;
                            if self.row > now {
                                return None;
                            }
                            self.worksheet_last_row = Some(now);
                        }
                    }
                    sheet_data.next_column(self.row, self.column)
                }
            };
            match column.filter(|column| *column <= self.last_column) {
                Some(column) => {
                    self.column = column + 1;
                    return Some(CellReferenceIndex {
                        sheet: self.sheet,
                        row: self.row,
                        column,
                    });
                }
                None => {
                    self.row += 1;
                    self.column = self.first_column;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::util::new_empty_model;

    fn reference(sheet: u32, row: i32, column: i32) -> CellReferenceIndex {
        CellReferenceIndex { sheet, row, column }
    }

    #[test]
    fn a_walk_over_a_missing_sheet_is_an_error() {
        let model = new_empty_model();
        let walk = RangeCells::new(
            &model,
            reference(7, 1, 1),
            reference(7, 2, 2),
            reference(0, 1, 1),
        );
        assert!(matches!(
            walk,
            Err(CalcResult::Error {
                error: Error::ERROR,
                ..
            })
        ));
    }

    #[test]
    fn a_walk_over_occupied_cells_gives_them_in_row_order() {
        let mut model = new_empty_model();
        for cell in ["C5", "A2", "B2", "D9"] {
            model._set(cell, "x");
        }
        let mut walk = RangeCells::new(
            &model,
            reference(0, 1, 1),
            reference(0, 1_048_576, 3),
            reference(0, 1, 5),
        )
        .map_err(|_| ())
        .expect("sheet 0 exists");
        let mut cells = Vec::new();
        while let Some(cell) = walk.next(&model, RangeWalk::Occupied) {
            cells.push((cell.row, cell.column));
        }
        assert_eq!(cells, [(2, 1), (2, 2), (5, 3)]);
    }
}
