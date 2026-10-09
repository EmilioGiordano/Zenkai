use zenkai_datagen::GenerationSpec;
use zenkai_engine::{Engine, EngineError, Workbook};
use zenkai_i18n::t;
use zenkai_types::{
    BorderPreset, CellPos, ColIdx, HAlign, MAX_COLS, MAX_ROWS, NumberFormat, Range, Rgb, RowIdx,
    SheetId, SheetInfo, StyleChange,
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
// The review records every generated cell, so the new text must fit in half of the app's
// 64 MiB review budget; with the snapshot of old inputs the peak stays near 100 MiB.
pub const MAX_GENERATE_BYTES: u64 = 32 * 1024 * 1024;
// Keeps the review's per-cell bookkeeping (about 60 bytes a cell beyond the text) near 12 MiB.
pub const MAX_GENERATE_CELLS: u64 = 200_000;

#[derive(Clone, Debug, PartialEq)]
enum Change {
    Inputs(Vec<Vec<String>>),
    Style(StyleChange),
    Generated(GenerationSpec),
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
    fitted(origin, rows.len() as u64, width as u64)
}

fn check_generated_size(spec: &GenerationSpec, cells: u64) -> Result<(), ToolError> {
    if cells > MAX_GENERATE_CELLS {
        return Err(ToolError::TooManyGeneratedCells {
            cells,
            limit: MAX_GENERATE_CELLS,
        });
    }
    let bytes = zenkai_datagen::estimated_output_bytes(spec);
    if bytes > MAX_GENERATE_BYTES {
        return Err(ToolError::GeneratedTextTooLarge {
            bytes,
            limit: MAX_GENERATE_BYTES,
        });
    }
    Ok(())
}

fn fitted(origin: CellPos, height: u64, width: u64) -> Result<Range, ToolError> {
    let last_row = u64::from(origin.row.get()) + height - 1;
    let last_col = u64::from(origin.col.get()) + width - 1;
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
        WriteRequest::GenerateData(request) => {
            zenkai_datagen::validate(&request.spec).map_err(ToolError::Generation)?;
            let header_and_rows = u64::from(request.spec.rows) + 1;
            let columns = request.spec.columns.len() as u64;
            check_generated_size(&request.spec, header_and_rows * columns)?;
            let target = fitted(parse_cell(&request.start)?, header_and_rows, columns)?;
            (
                &request.sheet,
                target,
                Change::Generated(request.spec.clone()),
            )
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

fn color_label(color: Option<Rgb>) -> String {
    color.map_or_else(
        || t!("plan.none").to_string(),
        |Rgb(rgb)| format!("#{rgb:06X}"),
    )
}

fn style_label(change: StyleChange) -> String {
    match change {
        StyleChange::Bold(true) => t!("plan.bold_on").to_string(),
        StyleChange::Bold(false) => t!("plan.bold_off").to_string(),
        StyleChange::Italic(true) => t!("plan.italic_on").to_string(),
        StyleChange::Italic(false) => t!("plan.italic_off").to_string(),
        StyleChange::Underline(true) => t!("plan.underline_on").to_string(),
        StyleChange::Underline(false) => t!("plan.underline_off").to_string(),
        StyleChange::Strike(true) => t!("plan.strike_on").to_string(),
        StyleChange::Strike(false) => t!("plan.strike_off").to_string(),
        StyleChange::Wrap(true) => t!("plan.wrap_on").to_string(),
        StyleChange::Wrap(false) => t!("plan.wrap_off").to_string(),
        StyleChange::FontSize(points) => t!("plan.font_size", points = points),
        StyleChange::Align(HAlign::General) => t!("plan.align_general").to_string(),
        StyleChange::Align(HAlign::Left) => t!("plan.align_left").to_string(),
        StyleChange::Align(HAlign::Center) => t!("plan.align_center").to_string(),
        StyleChange::Align(HAlign::Right) => t!("plan.align_right").to_string(),
        StyleChange::NumberFormat(NumberFormat::General) => t!("plan.format_general").to_string(),
        StyleChange::NumberFormat(NumberFormat::Number) => t!("plan.format_number").to_string(),
        StyleChange::NumberFormat(NumberFormat::Currency) => t!("plan.format_currency").to_string(),
        StyleChange::NumberFormat(NumberFormat::Percent) => t!("plan.format_percent").to_string(),
        StyleChange::NumberFormat(NumberFormat::Date) => t!("plan.format_date").to_string(),
        StyleChange::NumberFormat(NumberFormat::Time) => t!("plan.format_time").to_string(),
        StyleChange::Borders(BorderPreset::All) => t!("plan.borders_all").to_string(),
        StyleChange::Borders(BorderPreset::Outside) => t!("plan.borders_outside").to_string(),
        StyleChange::Borders(BorderPreset::Bottom) => t!("plan.borders_bottom").to_string(),
        StyleChange::Borders(BorderPreset::None) => t!("plan.borders_none").to_string(),
        StyleChange::Fill(color) => t!("plan.fill", color = color_label(color)),
        StyleChange::FontColor(color) => t!("plan.font_color", color = color_label(color)),
    }
}

fn generated_size(spec: &GenerationSpec) -> String {
    t!(
        "plan.generated_size",
        rows = t!("plan.rows", count = spec.rows),
        columns = t!("plan.columns", count = spec.columns.len())
    )
}

impl PlannedWrite {
    pub fn sheet(&self) -> SheetId {
        self.sheet
    }

    pub fn target(&self) -> Range {
        self.target
    }

    pub fn sheet_name(&self) -> &str {
        &self.sheet_name
    }

    // Only writes of cell inputs can be rejected later; a format change has no input to restore.
    pub fn input_block(&self) -> Option<Range> {
        matches!(self.change, Change::Inputs(_) | Change::Generated(_)).then_some(self.target)
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
                let formulas = match formulas {
                    0 => String::new(),
                    count => format!(" ({})", t!("plan.formulas", count = count)),
                };
                t!(
                    "plan.headline_write",
                    count = cells,
                    place = self.place(),
                    formulas = formulas
                )
            }
            Change::Style(change) => t!(
                "plan.format",
                place = self.place(),
                change = style_label(*change)
            ),
            Change::Generated(spec) => t!(
                "plan.headline_generate",
                size = generated_size(spec),
                place = self.place()
            ),
        }
    }

    // The first entries as the user will see them, for the approval bar's sample line.
    pub fn sample(&self) -> Option<String> {
        let entries = self.first_entries()?;
        let more = if self.target.cell_count() > SAMPLE_ENTRIES as u64 {
            ", …"
        } else {
            ""
        };
        Some(format!("{}{more}", entries.join(", ")))
    }

    // A generated table is only known once applied; its headers stand for it until then.
    fn first_entries(&self) -> Option<Vec<String>> {
        let entries: Vec<String> = match &self.change {
            Change::Inputs(rows) => rows
                .iter()
                .flatten()
                .take(SAMPLE_ENTRIES)
                .cloned()
                .collect(),
            Change::Generated(spec) => spec
                .columns
                .iter()
                .take(SAMPLE_ENTRIES)
                .map(|column| column.header_input())
                .collect(),
            Change::Style(_) => return None,
        };
        Some(entries.iter().map(|entry| shown_entry(entry)).collect())
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
                t!(
                    "plan.describe_write",
                    entry = shown_entry(&rows[0][0]),
                    place = place
                )
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
                t!(
                    "plan.describe_block",
                    cells = self.target.cell_count(),
                    formulas = formulas,
                    place = place,
                    sample = sample.join(", "),
                    more = more
                )
            }
            Change::Style(change) => {
                t!("plan.format", place = place, change = style_label(*change))
            }
            Change::Generated(spec) => t!(
                "plan.describe_generate",
                size = generated_size(spec),
                place = place,
                headers = self.first_entries().unwrap_or_default().join(", ")
            ),
        }
    }

    pub fn apply(self, workbook: &mut Workbook) -> Result<(), EngineError> {
        match self.change {
            Change::Inputs(rows) => workbook.set_inputs(self.sheet, self.target.start, &rows),
            Change::Style(change) => workbook.apply_style(self.sheet, self.target, change),
            Change::Generated(spec) => {
                let rows = zenkai_datagen::generate(&spec)
                    .map_err(|error| EngineError::Rejected(error.to_string()))?;
                let headers = spec.columns.iter().map(|column| column.header_input());
                let block: Vec<Vec<String>> =
                    std::iter::once(headers.collect()).chain(rows).collect();
                workbook.set_inputs(self.sheet, self.target.start, &block)
            }
        }
    }
}
