use crate::{expressions::types::CellReferenceIndex, model::Model};

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
    /// The cells from `left` to `right`, both on the same sheet.
    pub(crate) fn new(left: CellReferenceIndex, right: CellReferenceIndex) -> Self {
        RangeCells {
            sheet: left.sheet,
            first_column: left.column,
            last_column: right.column,
            last_row: right.row,
            row: left.row,
            column: left.column,
            worksheet_last_row: None,
        }
    }

    pub(crate) fn next(&mut self, model: &Model, walk: RangeWalk) -> Option<CellReferenceIndex> {
        while self.row <= self.last_row {
            let column = match walk {
                RangeWalk::Every => Some(self.column),
                RangeWalk::Occupied => {
                    let sheet_data = &model
                        .workbook
                        .worksheets
                        .get(self.sheet as usize)?
                        .sheet_data;
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
