use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, SheetInfo, SheetVisibility};

use crate::tools::error::ToolError;
use crate::tools::reply::{
    CellPage, FindResult, FoundCell, HiddenContent, SheetSummary, ToolReply,
};
use crate::tools::request::{Find, ReadRange, ReadRequest};

pub const MAX_READ_CELLS: u64 = 2_000;
pub const MAX_FIND_RESULTS: usize = 200;
// Longer than a cell shows on screen even in a wide column: the user sees only a part.
pub const LONG_TEXT_CHARS: usize = 1_000;

pub fn find_sheet<'a>(sheets: &'a [SheetInfo], name: &str) -> Result<&'a SheetInfo, ToolError> {
    // Excel compares sheet names without regard to case.
    sheets
        .iter()
        .find(|sheet| sheet.name.to_lowercase() == name.to_lowercase())
        .ok_or_else(|| ToolError::UnknownSheet(name.to_string()))
}

pub fn parse_range(text: &str) -> Result<Range, ToolError> {
    Range::parse_a1(text).ok_or_else(|| ToolError::BadAddress(text.to_string()))
}

pub fn parse_cell(text: &str) -> Result<CellPos, ToolError> {
    CellPos::parse_a1(text).ok_or_else(|| ToolError::BadAddress(text.to_string()))
}

pub fn read(request: &ReadRequest, workbook: &Workbook) -> Result<ToolReply, ToolError> {
    match request {
        ReadRequest::ListSheets => Ok(ToolReply::Sheets(list_sheets(workbook))),
        ReadRequest::ReadRange(request) => read_range(request, workbook).map(ToolReply::Cells),
        ReadRequest::Find(request) => find(request, workbook).map(ToolReply::Found),
    }
}

fn used_range(workbook: &Workbook, sheet: SheetId) -> Option<Range> {
    let end = workbook.used_end(sheet);
    let empty = end == CellPos::default() && workbook.input(sheet, end).is_empty();
    (!empty).then(|| Range::new(CellPos::default(), end))
}

fn list_sheets(workbook: &Workbook) -> Vec<SheetSummary> {
    workbook
        .sheets()
        .into_iter()
        .map(|sheet| SheetSummary {
            used: used_range(workbook, sheet.id),
            id: sheet.id,
            name: sheet.name,
            visibility: sheet.visibility,
        })
        .collect()
}

fn hidden_lines(workbook: &Workbook, sheet: &SheetInfo, range: Range) -> Vec<HiddenContent> {
    let sizes = workbook.sizes(sheet.id);
    let rows: Vec<RowIdx> = sizes
        .rows
        .iter()
        .filter(|(row, height)| *height == 0.0 && (range.start.row..=range.end.row).contains(row))
        .map(|(row, _)| *row)
        .collect();
    let cols: Vec<ColIdx> = sizes
        .columns
        .iter()
        .filter(|span| span.width == 0.0)
        .flat_map(|span| span.first.get()..=span.last.get())
        .filter_map(ColIdx::new)
        .filter(|col| (range.start.col..=range.end.col).contains(col))
        .collect();
    let mut hidden = Vec::new();
    if sheet.visibility != SheetVisibility::Visible {
        hidden.push(HiddenContent::Sheet(sheet.id));
    }
    if !rows.is_empty() {
        hidden.push(HiddenContent::Rows(rows));
    }
    if !cols.is_empty() {
        hidden.push(HiddenContent::Columns(cols));
    }
    hidden
}

fn read_range(request: &ReadRange, workbook: &Workbook) -> Result<CellPage, ToolError> {
    let sheets = workbook.sheets();
    let sheet = find_sheet(&sheets, &request.sheet)?;
    let range = parse_range(&request.range)?;
    let cols = u32::from(range.cols());
    if u64::from(cols) > MAX_READ_CELLS {
        return Err(ToolError::TooManyCells {
            cells: u64::from(cols),
            limit: MAX_READ_CELLS,
        });
    }
    // Rows past the used area hold nothing; they are not worth a page.
    let end = workbook.used_end(sheet.id);
    let last_row = range.end.row.min(end.row).max(range.start.row);
    let rows_per_page = (MAX_READ_CELLS as u32 / cols).max(1);
    let total_rows = last_row.get() - range.start.row.get() + 1;
    let pages = total_rows.div_ceil(rows_per_page);
    if request.page >= pages {
        return Err(ToolError::NoSuchPage {
            page: request.page,
            pages,
        });
    }
    let first = range
        .start
        .row
        .offset(i64::from(request.page) * i64::from(rows_per_page));
    let last = first.offset(i64::from(rows_per_page) - 1).min(last_row);
    let page_range = Range::new(
        CellPos::new(first, range.start.col),
        CellPos::new(last, range.end.col),
    );
    let mut long = Vec::new();
    let mut formulas = Vec::new();
    let values = (first.get()..=last.get())
        .filter_map(RowIdx::new)
        .map(|row| {
            (range.start.col.get()..=range.end.col.get())
                .filter_map(ColIdx::new)
                .map(|col| {
                    let pos = CellPos::new(row, col);
                    let input = workbook.input(sheet.id, pos);
                    if input.starts_with('=') {
                        formulas.push((pos, input));
                    }
                    let text = workbook.cell(sheet.id, pos).text;
                    if text.chars().count() > LONG_TEXT_CHARS {
                        long.push(pos);
                    }
                    text
                })
                .collect()
        })
        .collect();
    let mut hidden = hidden_lines(workbook, sheet, page_range);
    if !long.is_empty() {
        hidden.push(HiddenContent::LongText(long));
    }
    Ok(CellPage {
        sheet: sheet.name.clone(),
        range: page_range,
        page: request.page,
        pages,
        values,
        formulas,
        hidden,
    })
}

fn find(request: &Find, workbook: &Workbook) -> Result<FindResult, ToolError> {
    let sheets = workbook.sheets();
    let searched: Vec<&SheetInfo> = match &request.sheet {
        Some(name) => vec![find_sheet(&sheets, name)?],
        None => sheets.iter().collect(),
    };
    if request.text.is_empty() {
        return Err(ToolError::EmptySearch);
    }
    let needle = request.text.to_lowercase();
    let mut found = Vec::new();
    let mut hidden = Vec::new();
    let mut truncated = false;
    for sheet in searched {
        let mut matches: Vec<CellPos> = workbook
            .filled_cells(sheet.id)
            .into_iter()
            .filter(|pos| {
                workbook
                    .cell(sheet.id, *pos)
                    .text
                    .to_lowercase()
                    .contains(&needle)
            })
            .collect();
        matches.sort_by_key(|pos| (pos.row, pos.col));
        let room = MAX_FIND_RESULTS - found.len();
        truncated |= matches.len() > room;
        matches.truncate(room);
        if let (Some(first), Some(last)) = (matches.first(), matches.last()) {
            let rows = Range::new(*first, *last);
            let span = Range::new(
                CellPos::new(rows.start.row, ColIdx::default()),
                CellPos::new(rows.end.row, ColIdx::LAST),
            );
            hidden.extend(hidden_lines(workbook, sheet, span).into_iter().filter(
                |item| match item {
                    HiddenContent::Rows(rows) => matches.iter().any(|pos| rows.contains(&pos.row)),
                    HiddenContent::Columns(cols) => {
                        matches.iter().any(|pos| cols.contains(&pos.col))
                    }
                    _ => true,
                },
            ));
        }
        found.extend(matches.into_iter().map(|pos| FoundCell {
            sheet: sheet.name.clone(),
            pos,
            text: workbook.cell(sheet.id, pos).text,
        }));
        if found.len() >= MAX_FIND_RESULTS {
            break;
        }
    }
    Ok(FindResult {
        found,
        truncated,
        hidden,
    })
}
