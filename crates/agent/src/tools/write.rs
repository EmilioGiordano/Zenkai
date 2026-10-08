use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{
    CellPos, ColIdx, MAX_COLS, MAX_ROWS, Range, RowIdx, SheetId, SheetInfo, StyleChange,
};

use crate::tools::error::ToolError;
use crate::tools::read::{find_sheet, parse_cell, parse_range};
use crate::tools::reply::WriteSummary;
use crate::tools::request::WriteRequest;

// Each written cell costs undo history in the engine; agents write in pieces this size.
pub const MAX_WRITE_CELLS: u64 = 5_000;
pub const MAX_FORMAT_CELLS: u64 = 100_000;
pub const MAX_CELL_CHARS: usize = 32_767;

#[derive(Clone, Debug, PartialEq)]
enum Change {
    Inputs(Vec<Vec<String>>),
    Style(StyleChange),
}

// A write that passed every check and only waits for the user's approval. Inputs go in
// as one rectangular block, which the engine undoes in a single step.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedWrite {
    sheet: SheetId,
    sheet_name: String,
    target: Range,
    change: Change,
}

fn checked_count(cells: u64, limit: u64) -> Result<(), ToolError> {
    if cells > limit {
        return Err(ToolError::TooManyCells { cells, limit });
    }
    Ok(())
}

fn block_at(origin: CellPos, rows: &[Vec<String>]) -> Result<Range, ToolError> {
    let width = rows.first().map_or(0, Vec::len);
    if rows.is_empty() || width == 0 {
        return Err(ToolError::NothingToWrite);
    }
    if rows.iter().any(|row| row.len() != width) {
        return Err(ToolError::NotRectangular);
    }
    checked_count(rows.len() as u64 * width as u64, MAX_WRITE_CELLS)?;
    if let Some(entry) = rows
        .iter()
        .flatten()
        .find(|e| e.chars().count() > MAX_CELL_CHARS)
    {
        return Err(ToolError::TextTooLong {
            chars: entry.chars().count(),
            limit: MAX_CELL_CHARS,
        });
    }
    let last_row = u64::from(origin.row.get()) + rows.len() as u64 - 1;
    let last_col = u64::from(origin.col.get()) + width as u64 - 1;
    if last_row >= u64::from(MAX_ROWS) || last_col >= u64::from(MAX_COLS) {
        return Err(ToolError::OutsideSheet);
    }
    Ok(Range::new(
        origin,
        CellPos::new(
            RowIdx::clamped(last_row as i64),
            ColIdx::clamped(last_col as i64),
        ),
    ))
}

pub fn plan_write(request: &WriteRequest, sheets: &[SheetInfo]) -> Result<PlannedWrite, ToolError> {
    let (sheet_name, target, change) = match request {
        WriteRequest::WriteCells(request) => {
            let origin = parse_cell(&request.start)?;
            let target = block_at(origin, &request.rows)?;
            (&request.sheet, target, Change::Inputs(request.rows.clone()))
        }
        WriteRequest::SetFormula(request) => {
            if !request.formula.starts_with('=') {
                return Err(ToolError::NotAFormula);
            }
            let rows = vec![vec![request.formula.clone()]];
            let target = block_at(parse_cell(&request.cell)?, &rows)?;
            (&request.sheet, target, Change::Inputs(rows))
        }
        WriteRequest::FormatRange(request) => {
            let target = parse_range(&request.range)?;
            checked_count(target.cell_count(), MAX_FORMAT_CELLS)?;
            (
                &request.sheet,
                target,
                Change::Style(request.format.style_change()),
            )
        }
    };
    let sheet = find_sheet(sheets, sheet_name)?;
    Ok(PlannedWrite {
        sheet: sheet.id,
        sheet_name: sheet.name.clone(),
        target,
        change,
    })
}

impl PlannedWrite {
    pub fn sheet(&self) -> SheetId {
        self.sheet
    }

    pub fn summary(&self) -> WriteSummary {
        WriteSummary {
            range: self.target,
            cells: self.target.cell_count(),
        }
    }

    // Shown to the user before they allow the change.
    pub fn describe(&self) -> String {
        let place = format!("'{}'!{}", self.sheet_name, self.target);
        match &self.change {
            Change::Inputs(rows) if rows.len() == 1 && rows[0].len() == 1 => {
                let entry: String = rows[0][0].chars().take(80).collect();
                format!("Write \"{entry}\" in {place}")
            }
            Change::Inputs(_) => format!("Write {} cells in {place}", self.target.cell_count()),
            Change::Style(change) => format!("Format {place}: {change:?}"),
        }
    }

    pub fn apply(self, workbook: &mut Workbook) -> Result<(), EngineError> {
        match self.change {
            Change::Inputs(rows) => workbook.set_inputs(self.sheet, self.target.start, &rows),
            Change::Style(change) => workbook.apply_style(self.sheet, self.target, change),
        }
    }
}
