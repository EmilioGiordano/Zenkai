use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_types::{
    CellPos, ColIdx, MAX_COLS, MAX_ROWS, Range, Rgb, RowIdx, SheetId, SheetInfo, StyleChange,
};

use crate::tools::approval_text::{shown_entry, shown_text};
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

const SAMPLE_ENTRIES: usize = 4;

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn color_label(color: Option<Rgb>) -> String {
    color.map_or_else(|| "none".to_string(), |Rgb(rgb)| format!("#{rgb:06X}"))
}

fn style_label(change: StyleChange) -> String {
    match change {
        StyleChange::Bold(on) => format!("bold {}", on_off(on)),
        StyleChange::Italic(on) => format!("italic {}", on_off(on)),
        StyleChange::Underline(on) => format!("underline {}", on_off(on)),
        StyleChange::Strike(on) => format!("strikethrough {}", on_off(on)),
        StyleChange::Wrap(on) => format!("wrap text {}", on_off(on)),
        StyleChange::FontSize(points) => format!("font size {points}"),
        StyleChange::Align(align) => format!("align {align:?}").to_lowercase(),
        StyleChange::NumberFormat(format) => format!("number format {format:?}").to_lowercase(),
        StyleChange::Borders(preset) => format!("borders {preset:?}").to_lowercase(),
        StyleChange::Fill(color) => format!("fill {}", color_label(color)),
        StyleChange::FontColor(color) => format!("font colour {}", color_label(color)),
    }
}

impl PlannedWrite {
    pub fn sheet(&self) -> SheetId {
        self.sheet
    }

    pub fn target(&self) -> Range {
        self.target
    }

    fn place(&self) -> String {
        let name = shown_text(&self.sheet_name);
        if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            format!("{name}!{}", self.target)
        } else {
            format!("'{name}'!{}", self.target)
        }
    }

    // The action in words, as the approval bar's second line.
    pub fn headline(&self) -> String {
        match &self.change {
            Change::Inputs(rows) => {
                let cells = self.target.cell_count();
                let formulas = rows
                    .iter()
                    .flatten()
                    .filter(|entry| entry.starts_with('='))
                    .count();
                let noun = if cells == 1 { "cell" } else { "cells" };
                let formulas = match formulas {
                    0 => String::new(),
                    1 => " (1 formula)".to_string(),
                    count => format!(" ({count} formulas)"),
                };
                format!("Write {cells} {noun} in {}{formulas}", self.place())
            }
            Change::Style(change) => format!("Format {}: {}", self.place(), style_label(*change)),
        }
    }

    // The first entries as the user will see them, for the approval bar's sample line.
    pub fn sample(&self) -> Option<String> {
        let Change::Inputs(rows) = &self.change else {
            return None;
        };
        let shown: Vec<String> = rows
            .iter()
            .flatten()
            .take(SAMPLE_ENTRIES)
            .map(|entry| shown_entry(entry))
            .collect();
        let more = if self.target.cell_count() > SAMPLE_ENTRIES as u64 {
            ", …"
        } else {
            ""
        };
        Some(format!("{}{more}", shown.join(", ")))
    }

    pub fn summary(&self) -> WriteSummary {
        WriteSummary {
            range: self.target,
            cells: self.target.cell_count(),
        }
    }

    pub fn describe(&self) -> String {
        let place = format!("'{}'!{}", shown_text(&self.sheet_name), self.target);
        match &self.change {
            Change::Inputs(rows) if rows.len() == 1 && rows[0].len() == 1 => {
                format!("Write {} in {place}", shown_entry(&rows[0][0]))
            }
            Change::Inputs(rows) => {
                let sample: Vec<String> = rows
                    .iter()
                    .flatten()
                    .take(SAMPLE_ENTRIES)
                    .map(|entry| shown_entry(entry))
                    .collect();
                let more = if self.target.cell_count() > SAMPLE_ENTRIES as u64 {
                    ", …"
                } else {
                    ""
                };
                let formulas = rows
                    .iter()
                    .flatten()
                    .filter(|entry| entry.starts_with('='))
                    .count();
                format!(
                    "Write {} cells ({formulas} formulas in total) in {place}: {}{more}",
                    self.target.cell_count(),
                    sample.join(", ")
                )
            }
            Change::Style(change) => format!("Format {place}: {}", style_label(*change)),
        }
    }

    pub fn apply(self, workbook: &mut Workbook) -> Result<(), EngineError> {
        match self.change {
            Change::Inputs(rows) => workbook.set_inputs(self.sheet, self.target.start, &rows),
            Change::Style(change) => workbook.apply_style(self.sheet, self.target, change),
        }
    }
}
