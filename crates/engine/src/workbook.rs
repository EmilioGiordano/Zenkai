use std::io::Cursor;

use ironcalc::base::expressions::types::Area;
use ironcalc::base::types::{
    Cell, Color, FormulaValue, HorizontalAlignment, SpillValue, Style, VerticalAlignment,
};
use ironcalc::base::{BorderArea, ClipboardData, UserModel};
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::load_from_xlsx_bytes;

use crate::error::EngineError;
use crate::model::CachedModel;
use zenkai_types::{
    BorderPreset, CellPos, CellStyle, CellView, ColIdx, ColumnSpan, Contents, HAlign, Range, Rgb,
    RowIdx, SheetId, SheetInfo, SheetSizes, SheetVisibility, StyleChange, VAlign, ValueKind,
};

const LOCALE: &str = "en";
const MAX_FILL_CELLS: u64 = 1_000_000;
// IronCalc's undo history keeps about 1.5 KB per styled cell (measured: 2.8M cells cost
// 4.3 GB, 6.4 GB peak on undo), so styling is capped by memory, not by what the sheet holds.
const STYLE_HISTORY_BYTES_PER_CELL: u64 = 1536;
const STYLE_HISTORY_BUDGET_BYTES: u64 = 1536 * 1024 * 1024;
const MAX_STYLED_CELLS: u64 = STYLE_HISTORY_BUDGET_BYTES / STYLE_HISTORY_BYTES_PER_CELL;
const LANGUAGE: &str = "en";
// Stored widths are in Excel characters and heights in points; these give Excel's pixels.
const PIXELS_PER_CHAR: f64 = 7.0;
const CHAR_WIDTH_PADDING: f64 = 5.0;
const PIXELS_PER_POINT: f64 = 4.0 / 3.0;
// IronCalc's setter takes its own pixels: characters times this factor.
const COLUMN_WIDTH_FACTOR: f64 = 9.0;
// And its row height setter takes points times this factor.
const ROW_HEIGHT_FACTOR: f64 = 1.5625;
// Excel's last date, 9999-12-31.
const MAX_DATE_SERIAL: f64 = 2_958_465.0;
// Excel's own limit for a number format code.
const MAX_FORMAT_CODE: usize = 255;

fn whole_lines(range: Range) -> bool {
    (range.start.row == RowIdx::default() && range.end.row == RowIdx::LAST)
        || (range.start.col == ColIdx::default() && range.end.col == ColIdx::LAST)
}

// IronCalc formats whole rows and columns as row and column styles, but any other range
// cell by cell; a huge partial range would run for minutes.
fn check_border_range(range: Range) -> Result<(), EngineError> {
    if !whole_lines(range) && range.cell_count() > MAX_FILL_CELLS {
        return Err(rejected(format!(
            "borders on {} cells at once are not supported; select whole rows or columns",
            range.cell_count()
        )));
    }
    Ok(())
}

fn check_format_code(code: &str) -> Result<(), EngineError> {
    if code.chars().count() > MAX_FORMAT_CODE {
        return Err(rejected(format!(
            "a number format is limited to {MAX_FORMAT_CODE} characters"
        )));
    }
    Ok(())
}

pub trait Engine: Send {
    fn sheets(&self) -> Vec<SheetInfo>;
    fn cell(&self, sheet: SheetId, pos: CellPos) -> CellView;
    fn input(&self, sheet: SheetId, pos: CellPos) -> String;
    fn contents(&self, sheet: SheetId) -> Result<impl Fn(CellPos) -> Contents + Sync, EngineError>;
    fn set_input(&mut self, sheet: SheetId, pos: CellPos, text: &str) -> Result<(), EngineError>;
    fn set_inputs(
        &mut self,
        sheet: SheetId,
        origin: CellPos,
        rows: &[Vec<String>],
    ) -> Result<(), EngineError>;
    fn clear(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError>;
    fn clear_formats(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError>;
    fn clear_all(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError>;
    fn fill(&mut self, sheet: SheetId, target: Range, down: bool) -> Result<(), EngineError>;
    fn extend(&mut self, sheet: SheetId, source: Range, target: Range) -> Result<(), EngineError>;
    fn fill_with(
        &mut self,
        sheet: SheetId,
        from: CellPos,
        text: &str,
        range: Range,
    ) -> Result<(), EngineError>;
    fn sort(
        &mut self,
        sheet: SheetId,
        range: Range,
        key: ColIdx,
        descending: bool,
    ) -> Result<(), EngineError>;
    fn copy(&mut self, sheet: SheetId, range: Range) -> Result<Copied, EngineError>;
    fn paste(
        &mut self,
        sheet: SheetId,
        at: CellPos,
        copied: &Copied,
        cut: bool,
    ) -> Result<(), EngineError>;
    fn apply_style(
        &mut self,
        sheet: SheetId,
        range: Range,
        change: StyleChange,
    ) -> Result<(), EngineError>;
    fn set_scattered_inputs(
        &mut self,
        sheet: SheetId,
        inputs: &[(CellPos, String)],
    ) -> Result<(), EngineError>;
    fn set_number_format(
        &mut self,
        sheet: SheetId,
        range: Range,
        code: &str,
    ) -> Result<(), EngineError>;
    fn set_rows_hidden(
        &mut self,
        sheet: SheetId,
        range: Range,
        hidden: bool,
    ) -> Result<(), EngineError>;
    fn set_columns_hidden(
        &mut self,
        sheet: SheetId,
        range: Range,
        hidden: bool,
    ) -> Result<(), EngineError>;
    fn undo(&mut self) -> Result<(), EngineError>;
    fn redo(&mut self) -> Result<(), EngineError>;
    fn sizes(&self, sheet: SheetId) -> SheetSizes;
    fn set_column_width(
        &mut self,
        sheet: SheetId,
        col: ColIdx,
        pixels: f32,
    ) -> Result<(), EngineError>;
    fn set_row_height(
        &mut self,
        sheet: SheetId,
        row: RowIdx,
        pixels: f32,
    ) -> Result<(), EngineError>;
    fn frozen(&self, sheet: SheetId) -> (u32, u16);
    fn set_frozen(&mut self, sheet: SheetId, rows: u32, cols: u16) -> Result<(), EngineError>;
    fn insert_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError>;
    fn delete_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError>;
    fn insert_columns(&mut self, sheet: SheetId, at: ColIdx, count: u16)
    -> Result<(), EngineError>;
    fn delete_columns(&mut self, sheet: SheetId, at: ColIdx, count: u16)
    -> Result<(), EngineError>;
    fn merged(&self, sheet: SheetId) -> Vec<Range>;
    fn used_end(&self, sheet: SheetId) -> CellPos;
    fn filled_cells(&self, sheet: SheetId) -> Vec<CellPos>;
    fn add_sheet(&mut self) -> Result<SheetId, EngineError>;
    fn duplicate_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError>;
    fn rename_sheet(&mut self, sheet: SheetId, name: &str) -> Result<(), EngineError>;
    fn delete_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError>;
    fn move_sheet(&mut self, sheet: SheetId, to: u32) -> Result<(), EngineError>;
    fn to_xlsx(&self) -> Result<Vec<u8>, EngineError>;
}

pub struct Workbook {
    model: CachedModel,
}

// What a copy produced: the text Excel and other apps understand, plus the engine's
// own payload so a paste inside Zenkai keeps formulas (shifted) and styles.
#[derive(Clone, Debug)]
pub struct Copied {
    pub text: String,
    payload: serde_json::Value,
}

fn select(model: &mut UserModel<'static>, sheet: SheetId, range: Range) -> Result<(), EngineError> {
    model.set_selected_sheet(sheet.0).map_err(rejected)?;
    model
        .set_selected_cell(row_i32(range.start.row), col_i32(range.start.col))
        .map_err(rejected)?;
    model
        .set_selected_range(
            row_i32(range.start.row),
            col_i32(range.start.col),
            row_i32(range.end.row),
            col_i32(range.end.col),
        )
        .map_err(rejected)
}

// Typed, pasted and imported text reaches the engine parser without going through
// the file preflight, so formulas get the same limits here.
fn check_input(text: &str) -> Result<(), EngineError> {
    // The engine, like Excel, turns "+1+2" and "-1+2" into formulas too.
    if text.starts_with(['=', '+', '-']) {
        zenkai_xlsx_reader::limits::check_formula_text(text).map_err(EngineError::Rejected)?;
    }
    Ok(())
}

pub(crate) fn read_book(
    bytes: &[u8],
    name: &str,
) -> Result<ironcalc::base::types::Workbook, EngineError> {
    load_from_xlsx_bytes(bytes, name, LOCALE, timezone())
        .map_err(|e| EngineError::InvalidFile(format!("{e:?}")))
}

pub(crate) fn read_book_fast(
    bytes: &[u8],
    name: &str,
    inspect: &zenkai_xlsx_reader::Inspect<'_>,
) -> Result<ironcalc::base::types::Workbook, zenkai_xlsx_reader::ReadError> {
    zenkai_xlsx_reader::read_xlsx(bytes, name, LOCALE, timezone(), inspect)
}

// TODAY() and NOW() must follow the user clock, as in Excel.
fn timezone() -> &'static str {
    static TIMEZONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TIMEZONE.get_or_init(|| {
        iana_time_zone::get_timezone()
            .ok()
            .filter(|tz| ironcalc::base::Model::new_empty("probe", LOCALE, tz, LANGUAGE).is_ok())
            .unwrap_or_else(|| "UTC".to_string())
    })
}

fn rejected(message: String) -> EngineError {
    EngineError::Rejected(message)
}

impl Workbook {
    // The sheet is looked up once so a scan over thousands of cells only pays the cell
    // lookups.
    fn cell_lookup<'a>(
        &'a self,
        sheet: SheetId,
    ) -> Result<impl Fn(CellPos) -> Option<&'a Cell> + Sync + 'a, EngineError> {
        let sheet_data = &self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map_err(|_| EngineError::UnknownSheet(sheet))?
            .sheet_data;
        Ok(move |pos: CellPos| sheet_data.get(&row_i32(pos.row))?.get(&col_i32(pos.col)))
    }
}

fn row_i32(row: RowIdx) -> i32 {
    row.get() as i32 + 1
}

fn col_i32(col: ColIdx) -> i32 {
    i32::from(col.get()) + 1
}

fn area(sheet: SheetId, range: Range) -> Area {
    Area {
        sheet: sheet.0,
        row: row_i32(range.start.row),
        column: col_i32(range.start.col),
        width: i32::from(range.cols()),
        height: range.rows() as i32,
    }
}

impl Workbook {
    // Up to MAX_STYLED_CELLS the range is styled as selected, so a cell typed into later
    // keeps the format as in Excel. A bigger one is cut to the used area, which holds
    // every cell that exists; `None` means the range lies wholly beyond it.
    fn style_target(&self, sheet: SheetId, range: Range) -> Result<Option<Range>, EngineError> {
        if whole_lines(range) || range.cell_count() <= MAX_STYLED_CELLS {
            return Ok(Some(range));
        }
        let Some(used) = range.clip_to(self.used_end(sheet)) else {
            return Ok(None);
        };
        if used.cell_count() > MAX_STYLED_CELLS {
            return Err(rejected(format!(
                "formatting {} cells at once is not supported; select whole rows or columns",
                used.cell_count()
            )));
        }
        Ok(Some(used))
    }

    fn formattable(&self, sheet: SheetId, range: Range) -> Result<Range, EngineError> {
        self.style_target(sheet, range)?.ok_or_else(|| {
            rejected(format!(
                "formatting {} empty cells at once is not supported; select whole rows or columns",
                range.cell_count()
            ))
        })
    }

    fn apply_borders(
        &mut self,
        sheet: SheetId,
        range: Range,
        preset: BorderPreset,
    ) -> Result<(), EngineError> {
        check_border_range(range)?;
        let kind = match preset {
            BorderPreset::All => "All",
            BorderPreset::Outside => "Outer",
            BorderPreset::Bottom => "Bottom",
            BorderPreset::None => "None",
        };
        // BorderArea's fields are private to ironcalc; its serde form is public.
        let border: BorderArea = serde_json::from_value(serde_json::json!({
            "item": { "style": "thin" },
            "type": kind,
        }))
        .map_err(|e| rejected(e.to_string()))?;
        self.model
            .set_area_with_border(&area(sheet, range), &border)
            .map_err(rejected)
    }

    // Cells holding contents (not just a style) inside `range`, read straight from the
    // sheet data: cheaper than `filled_cells`, which formats every input of the sheet.
    fn content_cells_in(&self, sheet: SheetId, range: Range) -> Result<Vec<CellPos>, EngineError> {
        let ws = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map_err(|_| EngineError::UnknownSheet(sheet))?;
        let rows = row_i32(range.start.row)..=row_i32(range.end.row);
        let cols = col_i32(range.start.col)..=col_i32(range.end.col);
        Ok(ws
            .sheet_data
            .iter()
            .filter(|(row, _)| rows.contains(*row))
            .flat_map(|(row, columns)| {
                columns
                    .iter()
                    .filter(|(col, cell)| {
                        cols.contains(*col) && !matches!(cell, Cell::EmptyCell { .. })
                    })
                    .map(move |(col, _)| {
                        CellPos::new(
                            RowIdx::clamped(i64::from(*row) - 1),
                            ColIdx::clamped(i64::from(*col) - 1),
                        )
                    })
            })
            .collect())
    }

    // What Excel's AutoFill continues along one source line: numbers (two or more),
    // dates (one or more, a day apart when alone) and text ending in a number ("Item 1").
    // Anything else repeats.
    fn series(&self, sheet: SheetId, line: impl Iterator<Item = CellPos>) -> Option<Series> {
        let cells: Vec<(String, CellView)> = line
            .map(|pos| (self.input(sheet, pos), self.cell(sheet, pos)))
            .collect();
        if cells.iter().any(|(input, _)| input.starts_with('=')) {
            return None;
        }
        // Rust also parses "inf", "nan" and "1e999", which Excel keeps as text.
        let plain = |input: &str| {
            input
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && value.abs() < 1e290)
        };
        // Dates first: a date loaded from a file may read back as its bare serial.
        let dates = cells
            .iter()
            .map(|(_, view)| {
                // Excel dates run from serial 0 to 2958465 (9999-12-31).
                view.number
                    .filter(|serial| (0.0..=MAX_DATE_SERIAL).contains(serial))
                    .filter(|_| is_date_format(&view.style.num_fmt))
            })
            .collect::<Option<Vec<f64>>>();
        if let Some(serials) = dates {
            return Some(Series::Dates(Trend::fit_or_step(&serials)));
        }
        if let Some(values) = cells
            .iter()
            .map(|(input, _)| plain(input))
            .collect::<Option<Vec<_>>>()
        {
            return (values.len() >= 2).then(|| Series::Numbers(Trend::fit(&values)));
        }
        let parts = cells
            .iter()
            .map(|(input, _)| split_trailing_number(input))
            .collect::<Option<Vec<_>>>()?;
        let (prefix, _, width) = parts[0].clone();
        if parts.iter().any(|(p, _, _)| *p != prefix) {
            return None;
        }
        let numbers: Vec<f64> = parts.iter().map(|(_, n, _)| *n as f64).collect();
        Some(Series::Text {
            prefix,
            width,
            trend: Trend::fit_or_step(&numbers),
        })
    }

    pub fn new_empty() -> Result<Workbook, EngineError> {
        let model =
            UserModel::new_empty("Book1", LOCALE, timezone(), LANGUAGE).map_err(rejected)?;
        Ok(Workbook {
            model: CachedModel::new(model),
        })
    }

    pub fn from_xlsx_bytes(bytes: &[u8], name: &str) -> Result<Workbook, EngineError> {
        Workbook::from_book(read_book(bytes, name)?)
    }

    pub(crate) fn from_book(
        book: ironcalc::base::types::Workbook,
    ) -> Result<Workbook, EngineError> {
        let model = ironcalc::base::Model::from_workbook(book, LANGUAGE)
            .map_err(EngineError::InvalidFile)?;
        let mut model = UserModel::from_model(model);
        model.evaluate();
        let workbook = Workbook {
            model: CachedModel::new(model),
        };
        workbook.warm_used_areas();
        Ok(workbook)
    }

    // Call on a background thread after building or editing, so the UI thread never walks a sheet.
    pub fn warm_used_areas(&self) {
        for sheet in self.sheets() {
            self.model.used_end(sheet.id);
        }
    }

    fn resolve(&self, color: &Color) -> Option<Rgb> {
        match color {
            Color::None => None,
            other => Rgb::parse_hex(&self.model.resolve_color(other)),
        }
    }

    fn style_view(&self, style: &Style) -> CellStyle {
        let align = match style.alignment.as_ref().map(|a| &a.horizontal) {
            Some(HorizontalAlignment::Left) => HAlign::Left,
            Some(HorizontalAlignment::Center | HorizontalAlignment::CenterContinuous) => {
                HAlign::Center
            }
            Some(HorizontalAlignment::Right) => HAlign::Right,
            _ => HAlign::General,
        };
        let valign = match style.alignment.as_ref().map(|a| &a.vertical) {
            Some(VerticalAlignment::Top) => VAlign::Top,
            Some(VerticalAlignment::Center) => VAlign::Center,
            _ => VAlign::Bottom,
        };
        CellStyle {
            valign,
            wrap: style.alignment.as_ref().is_some_and(|a| a.wrap_text),
            bold: style.font.b,
            italic: style.font.i,
            underline: style.font.u,
            strike: style.font.strike,
            font_size: (style.font.sz > 0).then_some(style.font.sz as f32),
            font_color: self.resolve(&style.font.color),
            fill: self.resolve(&style.fill.color),
            align,
            border_top: style.border.top.is_some(),
            border_left: style.border.left.is_some(),
            border_bottom: style.border.bottom.is_some(),
            border_right: style.border.right.is_some(),
            num_fmt: style.num_fmt.clone(),
        }
    }
}

impl Engine for Workbook {
    fn sheets(&self) -> Vec<SheetInfo> {
        (0u32..)
            .zip(self.model.get_worksheets_properties())
            .map(|(index, props)| SheetInfo {
                id: SheetId(index),
                visibility: match props.state.as_str() {
                    "hidden" => SheetVisibility::Hidden,
                    "veryHidden" => SheetVisibility::VeryHidden,
                    _ => SheetVisibility::Visible,
                },
                name: props.name,
            })
            .collect()
    }

    fn cell(&self, sheet: SheetId, pos: CellPos) -> CellView {
        let (row, col) = (row_i32(pos.row), col_i32(pos.col));
        let model = self.model.get_model();
        let text = match self.model.get_formatted_cell_value(sheet.0, row, col) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(%error, %pos, "could not read cell");
                return CellView {
                    text: "#ERROR".to_string(),
                    kind: ValueKind::Error,
                    ..CellView::default()
                };
            }
        };
        let (kind, number) = match model.get_cell_value_by_index(sheet.0, row, col) {
            Ok(ironcalc::base::cell::CellValue::Number(n)) => (ValueKind::Number, Some(n)),
            Ok(ironcalc::base::cell::CellValue::Boolean(_)) => (ValueKind::Bool, None),
            Ok(ironcalc::base::cell::CellValue::String(s)) if s.starts_with('#') => {
                (ValueKind::Error, None)
            }
            Ok(ironcalc::base::cell::CellValue::String(_)) => (ValueKind::Text, None),
            _ => (ValueKind::Empty, None),
        };
        let style = self
            .model
            .get_cell_style(sheet.0, row, col)
            .map(|s| self.style_view(&s))
            .unwrap_or_default();
        CellView {
            text,
            kind,
            number,
            style,
        }
    }

    // A formula returning "" counts as filled, as it does for Ctrl+Arrow in Excel. The
    // stored value is read without formatting, so a scan over a column of hundreds of
    // thousands of cells does not build a string per cell.
    fn contents(&self, sheet: SheetId) -> Result<impl Fn(CellPos) -> Contents + Sync, EngineError> {
        let cell_at = self.cell_lookup(sheet)?;
        Ok(move |pos| match cell_at(pos) {
            None | Some(Cell::EmptyCell { .. }) => Contents::Empty,
            Some(
                Cell::NumberCell { v, .. }
                | Cell::CellFormula {
                    v: FormulaValue::Number(v),
                    ..
                }
                | Cell::ArrayFormula {
                    v: FormulaValue::Number(v),
                    ..
                }
                | Cell::SpillCell {
                    v: SpillValue::Number(v),
                    ..
                },
            ) => Contents::Number(*v),
            Some(_) => Contents::NonNumeric,
        })
    }

    fn input(&self, sheet: SheetId, pos: CellPos) -> String {
        self.model
            .get_cell_content(sheet.0, row_i32(pos.row), col_i32(pos.col))
            .unwrap_or_default()
    }

    fn set_input(&mut self, sheet: SheetId, pos: CellPos, text: &str) -> Result<(), EngineError> {
        check_input(text)?;
        self.model
            .set_user_input(sheet.0, row_i32(pos.row), col_i32(pos.col), text)
            .map_err(rejected)
    }

    // One engine paste, so a whole paste, import or fill is a single undo step.
    fn set_inputs(
        &mut self,
        sheet: SheetId,
        origin: CellPos,
        rows: &[Vec<String>],
    ) -> Result<(), EngineError> {
        let height = rows.len() as u64;
        let width = rows.iter().map(Vec::len).max().unwrap_or(0) as u64;
        if u64::from(origin.row.get()) + height > u64::from(zenkai_types::MAX_ROWS)
            || u64::from(origin.col.get()) + width > u64::from(zenkai_types::MAX_COLS)
        {
            return Err(EngineError::Rejected(format!(
                "{height} rows by {width} columns starting at {origin} do not fit in a sheet"
            )));
        }
        rows.iter()
            .flatten()
            .try_for_each(|text| check_input(text))?;
        if rows.is_empty() || width == 0 {
            return Ok(());
        }
        let mut writer = csv::WriterBuilder::new()
            .delimiter(b'\t')
            .flexible(true)
            .from_writer(Vec::new());
        // The engine's TSV reader drops records whose width differs, so every row is
        // padded to the full width of the block.
        let padding = usize::try_from(width).unwrap_or(usize::MAX);
        for row in rows {
            let missing = padding.saturating_sub(row.len());
            let fields = row
                .iter()
                .map(String::as_str)
                .chain(std::iter::repeat_n("", missing));
            writer
                .write_record(fields)
                .map_err(|e| rejected(e.to_string()))?;
        }
        let tsv = writer
            .into_inner()
            .map_err(|e| rejected(e.to_string()))
            .and_then(|bytes| String::from_utf8(bytes).map_err(|e| rejected(e.to_string())))?;
        let area = Area {
            sheet: sheet.0,
            row: row_i32(origin.row),
            column: col_i32(origin.col),
            width: i32::try_from(width).map_err(|e| rejected(e.to_string()))?,
            height: i32::try_from(height).map_err(|e| rejected(e.to_string()))?,
        };
        let end = CellPos::new(
            origin
                .row
                .offset(i64::try_from(height).unwrap_or(i64::MAX) - 1),
            origin
                .col
                .offset(i64::try_from(width).unwrap_or(i64::MAX) - 1),
        );
        select(&mut self.model, sheet, Range::new(origin, end))?;
        self.model.paste_csv_string(&area, &tsv).map_err(rejected)
    }

    // Rows of `range` reordered by the `key` column, with Excel's order: numbers, text
    // (ignoring case), logicals, errors, and blanks always last; ties keep their order.
    // Formulas are rewritten for their new row, as Excel's sort does; formats stay put.
    fn sort(
        &mut self,
        sheet: SheetId,
        range: Range,
        key: ColIdx,
        descending: bool,
    ) -> Result<(), EngineError> {
        if range.cell_count() > MAX_FILL_CELLS {
            return Err(rejected(format!(
                "sorting {} cells at once is not supported",
                range.cell_count()
            )));
        }
        // Excel refuses too: merged areas would stay put while their contents move.
        if self
            .merged(sheet)
            .iter()
            .any(|merge| merge.intersects(&range))
        {
            return Err(rejected(
                "cells in the range are merged; unmerge them before sorting".to_string(),
            ));
        }
        let rows: Vec<RowIdx> = (range.start.row.get()..=range.end.row.get())
            .map(|row| RowIdx::clamped(i64::from(row)))
            .collect();
        let keys: Vec<SortKey> = rows
            .iter()
            .map(|row| SortKey::of(&self.cell(sheet, CellPos::new(*row, key))))
            .collect();
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|a, b| SortKey::compare(&keys[*a], &keys[*b], descending));
        if order.iter().enumerate().all(|(at, from)| at == *from) {
            return Ok(());
        }
        let model = self.model.get_model();
        let moved = order
            .iter()
            .zip(&rows)
            .map(|(from, to)| {
                (range.start.col.get()..=range.end.col.get())
                    .map(|col| {
                        model
                            .extend_to(
                                sheet.0,
                                row_i32(rows[*from]),
                                i32::from(col) + 1,
                                row_i32(*to),
                                i32::from(col) + 1,
                            )
                            .map_err(rejected)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.set_inputs(sheet, range.start, &moved)
    }

    // Ctrl+Enter: `text` typed at `from` goes into every cell of `range`, formulas
    // shifted for each cell, in one undo step.
    fn fill_with(
        &mut self,
        sheet: SheetId,
        from: CellPos,
        text: &str,
        range: Range,
    ) -> Result<(), EngineError> {
        // Refused before anything is written, so a huge selection leaves no partial edit.
        if range.cell_count() > MAX_FILL_CELLS {
            return Err(rejected(format!(
                "filling {} cells at once is not supported",
                range.cell_count()
            )));
        }
        self.set_input(sheet, from, text)?;
        let model = self.model.get_model();
        let rows = (range.start.row.get()..=range.end.row.get())
            .map(|row| {
                (range.start.col.get()..=range.end.col.get())
                    .map(|col| {
                        model
                            .extend_to(
                                sheet.0,
                                row_i32(from.row),
                                col_i32(from.col),
                                row as i32 + 1,
                                i32::from(col) + 1,
                            )
                            .map_err(rejected)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.set_inputs(sheet, range.start, &rows)
    }

    // The fill handle: the cells of `target` past `source` (below it or to its right)
    // repeat the source pattern, formulas shifted, in one undo step.
    fn extend(&mut self, sheet: SheetId, source: Range, target: Range) -> Result<(), EngineError> {
        let down = target.end.row > source.end.row;
        let rest = if down {
            Range::new(
                CellPos::new(source.end.row.offset(1), source.start.col),
                CellPos::new(target.end.row, source.end.col),
            )
        } else if target.end.col > source.end.col {
            Range::new(
                CellPos::new(source.start.row, source.end.col.offset(1)),
                CellPos::new(source.end.row, target.end.col),
            )
        } else {
            return Ok(());
        };
        if rest.cell_count() > MAX_FILL_CELLS {
            return Err(rejected(format!(
                "filling {} cells at once is not supported",
                rest.cell_count()
            )));
        }
        let lines: Vec<Option<Series>> = if down {
            (source.start.col.get()..=source.end.col.get())
                .map(|col| {
                    let line = (source.start.row.get()..=source.end.row.get()).map(|row| {
                        CellPos::new(
                            RowIdx::clamped(i64::from(row)),
                            ColIdx::clamped(i64::from(col)),
                        )
                    });
                    self.series(sheet, line)
                })
                .collect()
        } else {
            (source.start.row.get()..=source.end.row.get())
                .map(|row| {
                    let line = (source.start.col.get()..=source.end.col.get()).map(|col| {
                        CellPos::new(
                            RowIdx::clamped(i64::from(row)),
                            ColIdx::clamped(i64::from(col)),
                        )
                    });
                    self.series(sheet, line)
                })
                .collect()
        };
        let model = self.model.get_model();
        let (source_rows, source_cols) = (source.rows(), u32::from(source.cols()));
        let rows = (rest.start.row.get()..=rest.end.row.get())
            .map(|row| {
                (rest.start.col.get()..=rest.end.col.get())
                    .map(|col| {
                        let (line, step) = if down {
                            (col - source.start.col.get(), row - source.start.row.get())
                        } else {
                            (
                                u16::try_from(row - source.start.row.get()).unwrap_or(0),
                                u32::from(col - source.start.col.get()),
                            )
                        };
                        if let Some(Some(series)) = lines.get(usize::from(line)) {
                            return Ok(series.at(f64::from(step)));
                        }
                        let (from_row, from_col) = if down {
                            let offset = (row - source.start.row.get()) % source_rows;
                            (source.start.row.get() + offset, u32::from(col))
                        } else {
                            let offset =
                                (u32::from(col) - u32::from(source.start.col.get())) % source_cols;
                            (row, u32::from(source.start.col.get()) + offset)
                        };
                        model
                            .extend_to(
                                sheet.0,
                                from_row as i32 + 1,
                                from_col as i32 + 1,
                                row as i32 + 1,
                                i32::from(col) + 1,
                            )
                            .map_err(rejected)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.set_inputs(sheet, rest.start, &rows)
    }

    fn copy(&mut self, sheet: SheetId, range: Range) -> Result<Copied, EngineError> {
        let clipboard = self.model.select_without_invalidation(|model| {
            select(model, sheet, range)?;
            model.copy_to_clipboard().map_err(rejected)
        })?;
        let payload = serde_json::to_value(&clipboard).map_err(|e| rejected(e.to_string()))?;
        let text = payload
            .get("csv")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| rejected("clipboard payload lacks csv".to_string()))?
            .to_string();
        Ok(Copied { text, payload })
    }

    fn paste(
        &mut self,
        sheet: SheetId,
        at: CellPos,
        copied: &Copied,
        cut: bool,
    ) -> Result<(), EngineError> {
        let field = |name: &str| {
            copied
                .payload
                .get(name)
                .cloned()
                .ok_or_else(|| rejected(format!("clipboard payload lacks {name}")))
        };
        let parse = |e: serde_json::Error| rejected(e.to_string());
        let data: ClipboardData = serde_json::from_value(field("data")?).map_err(parse)?;
        let source_sheet: u32 = serde_json::from_value(field("sheet")?).map_err(parse)?;
        let source_range: (i32, i32, i32, i32) =
            serde_json::from_value(field("range")?).map_err(parse)?;
        select(&mut self.model, sheet, Range::single(at))?;
        self.model
            .paste_from_clipboard(source_sheet, source_range, &data, cut)
            .map_err(rejected)
    }

    // Ctrl+D / Ctrl+R: the first row (or column) of the target is extended over the rest
    // with references shifted; a single row (or column) extends the one above (or left).
    fn fill(&mut self, sheet: SheetId, target: Range, down: bool) -> Result<(), EngineError> {
        let source_line = if down {
            if target.rows() == 1 {
                target.start.row.get().checked_sub(1)
            } else {
                Some(target.start.row.get())
            }
        } else if target.cols() == 1 {
            target.start.col.get().checked_sub(1).map(u32::from)
        } else {
            Some(u32::from(target.start.col.get()))
        };
        let Some(source_line) = source_line else {
            return Ok(());
        };
        let rest = match (down, target.rows() == 1, target.cols() == 1) {
            (true, true, _) | (false, _, true) => target,
            (true, false, _) => Range::new(
                CellPos::new(target.start.row.offset(1), target.start.col),
                target.end,
            ),
            (false, _, false) => Range::new(
                CellPos::new(target.start.row, target.start.col.offset(1)),
                target.end,
            ),
        };
        if rest.cell_count() > MAX_FILL_CELLS {
            return Err(rejected(format!(
                "filling {} cells at once is not supported",
                rest.cell_count()
            )));
        }
        let model = self.model.get_model();
        let rows = (rest.start.row.get()..=rest.end.row.get())
            .map(|row| {
                (rest.start.col.get()..=rest.end.col.get())
                    .map(|col| {
                        let (from_row, from_col) = if down {
                            (source_line, u32::from(col))
                        } else {
                            (row, source_line)
                        };
                        model
                            .extend_to(
                                sheet.0,
                                from_row as i32 + 1,
                                from_col as i32 + 1,
                                row as i32 + 1,
                                i32::from(col) + 1,
                            )
                            .map_err(rejected)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.set_inputs(sheet, rest.start, &rows)
    }

    fn clear_formats(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        let Some(target) = self.style_target(sheet, range)? else {
            return Ok(());
        };
        self.model
            .range_clear_formatting(&area(sheet, target))
            .map_err(rejected)
    }

    fn clear_all(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        // Whole rows or columns can carry row or column styles, which are not cells, so
        // they need the formatting pass too (a second undo step). Otherwise one call on
        // the used part clears everything there is, in one step.
        if whole_lines(range) {
            self.clear_formats(sheet, range)?;
            return self.clear(sheet, range);
        }
        let Some(used) = range.clip_to(self.used_end(sheet)) else {
            return Ok(());
        };
        let Some(used) = self.style_target(sheet, used)? else {
            return Ok(());
        };
        self.model
            .range_clear_all(&area(sheet, used))
            .map_err(rejected)
    }

    // IronCalc visits every cell of the range it clears, so only the box around the
    // contents inside the range is cleared (a whole sheet has 17 billion cells, and a
    // formatted far cell makes the used area that big). Contents scattered so far apart
    // that the box is huge are cleared cell by cell. Nothing to clear writes nothing, so
    // no empty cells are created to grow the used area.
    fn clear(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        let cells = self.content_cells_in(sheet, range)?;
        let Some(first) = cells.first() else {
            return Ok(());
        };
        let (mut top, mut left, mut bottom, mut right) =
            (first.row, first.col, first.row, first.col);
        for pos in &cells {
            top = top.min(pos.row);
            bottom = bottom.max(pos.row);
            left = left.min(pos.col);
            right = right.max(pos.col);
        }
        let bounds = Range::new(CellPos::new(top, left), CellPos::new(bottom, right));
        if bounds.cell_count() > MAX_FILL_CELLS {
            let empties: Vec<(CellPos, String)> =
                cells.into_iter().map(|pos| (pos, String::new())).collect();
            return self.set_scattered_inputs(sheet, &empties);
        }
        self.model
            .range_clear_contents(&area(sheet, bounds))
            .map_err(rejected)
    }

    fn apply_style(
        &mut self,
        sheet: SheetId,
        range: Range,
        change: StyleChange,
    ) -> Result<(), EngineError> {
        let flag = |on: bool| if on { "true" } else { "false" }.to_string();
        let hex = |color: Option<Rgb>| color.map_or_else(String::new, |c| format!("#{:06X}", c.0));
        let (path, value) = match change {
            StyleChange::Bold(on) => ("font.b", flag(on)),
            StyleChange::Italic(on) => ("font.i", flag(on)),
            StyleChange::Underline(on) => ("font.u", flag(on)),
            StyleChange::Strike(on) => ("font.strike", flag(on)),
            StyleChange::Wrap(on) => ("alignment.wrap_text", flag(on)),
            // Excel's font sizes run from 1 to 409 points.
            StyleChange::FontSize(points) => ("font.size", points.clamp(1, 409).to_string()),
            StyleChange::Borders(preset) => return self.apply_borders(sheet, range, preset),
            StyleChange::FontColor(color) => ("font.color", hex(color)),
            StyleChange::Fill(color) => ("fill.fg_color", hex(color)),
            StyleChange::Align(align) => (
                "alignment.horizontal",
                match align {
                    HAlign::General => "general",
                    HAlign::Left => "left",
                    HAlign::Center => "center",
                    HAlign::Right => "right",
                }
                .to_string(),
            ),
            StyleChange::NumberFormat(format) => ("num_fmt", format.code().to_string()),
        };
        let target = self.formattable(sheet, range)?;
        self.model
            .update_range_style(&area(sheet, target), path, &value)
            .map_err(rejected)
    }

    fn set_number_format(
        &mut self,
        sheet: SheetId,
        range: Range,
        code: &str,
    ) -> Result<(), EngineError> {
        check_format_code(code)?;
        let target = self.formattable(sheet, range)?;
        self.model
            .update_range_style(&area(sheet, target), "num_fmt", code)
            .map_err(rejected)
    }

    // IronCalc has no batch write, so each cell is its own undo step; evaluation waits
    // until the last one so the workbook recalculates once.
    fn set_scattered_inputs(
        &mut self,
        sheet: SheetId,
        inputs: &[(CellPos, String)],
    ) -> Result<(), EngineError> {
        for (_, text) in inputs {
            check_input(text)?;
        }
        self.model.pause_evaluation();
        let written = inputs.iter().try_for_each(|(pos, text)| {
            self.model
                .set_user_input(sheet.0, row_i32(pos.row), col_i32(pos.col), text)
                .map_err(rejected)
        });
        self.model.resume_evaluation();
        self.model.evaluate();
        written
    }

    fn set_rows_hidden(
        &mut self,
        sheet: SheetId,
        range: Range,
        hidden: bool,
    ) -> Result<(), EngineError> {
        self.model
            .set_rows_hidden(
                sheet.0,
                row_i32(range.start.row),
                row_i32(range.end.row),
                hidden,
            )
            .map_err(rejected)
    }

    fn set_columns_hidden(
        &mut self,
        sheet: SheetId,
        range: Range,
        hidden: bool,
    ) -> Result<(), EngineError> {
        self.model
            .set_columns_hidden(
                sheet.0,
                col_i32(range.start.col),
                col_i32(range.end.col),
                hidden,
            )
            .map_err(rejected)
    }

    fn undo(&mut self) -> Result<(), EngineError> {
        self.model.undo().map_err(rejected)
    }

    fn redo(&mut self) -> Result<(), EngineError> {
        self.model.redo().map_err(rejected)
    }

    fn sizes(&self, sheet: SheetId) -> SheetSizes {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet.0) else {
            return SheetSizes::default();
        };
        let columns = ws
            .cols
            .iter()
            .map(|c| ColumnSpan {
                first: ColIdx::clamped(i64::from(c.min) - 1),
                last: ColIdx::clamped(i64::from(c.max) - 1),
                width: if c.hidden {
                    0.0
                } else {
                    (c.width * PIXELS_PER_CHAR + CHAR_WIDTH_PADDING).round() as f32
                },
            })
            .collect();
        let rows = ws
            .rows
            .iter()
            .filter(|r| r.custom_height || r.hidden)
            .map(|r| {
                let height = if r.hidden {
                    0.0
                } else {
                    (r.height * PIXELS_PER_POINT).round() as f32
                };
                (RowIdx::clamped(i64::from(r.r) - 1), height)
            })
            .collect();
        SheetSizes { columns, rows }
    }

    fn set_column_width(
        &mut self,
        sheet: SheetId,
        col: ColIdx,
        pixels: f32,
    ) -> Result<(), EngineError> {
        // Excel's widest column is 255 characters; a non-finite drag maps to zero.
        let pixels = if pixels.is_finite() {
            f64::from(pixels)
        } else {
            0.0
        };
        let chars = ((pixels - CHAR_WIDTH_PADDING) / PIXELS_PER_CHAR).clamp(0.0, 255.0);
        let column = col_i32(col);
        self.model
            .set_columns_width(sheet.0, column, column, chars * COLUMN_WIDTH_FACTOR)
            .map_err(rejected)
    }

    fn set_row_height(
        &mut self,
        sheet: SheetId,
        row: RowIdx,
        pixels: f32,
    ) -> Result<(), EngineError> {
        let row = row_i32(row);
        self.model
            .set_rows_height(
                sheet.0,
                row,
                row,
                // Excel's tallest row is 409.5 points.
                (if pixels.is_finite() {
                    f64::from(pixels)
                } else {
                    0.0
                } / PIXELS_PER_POINT)
                    .clamp(0.0, 409.5)
                    * ROW_HEIGHT_FACTOR,
            )
            .map_err(rejected)
    }

    fn frozen(&self, sheet: SheetId) -> (u32, u16) {
        let rows = self.model.get_frozen_rows_count(sheet.0).unwrap_or(0);
        let cols = self.model.get_frozen_columns_count(sheet.0).unwrap_or(0);
        (
            u32::try_from(rows).unwrap_or(0),
            u16::try_from(cols).unwrap_or(0),
        )
    }

    fn set_frozen(&mut self, sheet: SheetId, rows: u32, cols: u16) -> Result<(), EngineError> {
        let rows = i32::try_from(rows).map_err(|e| rejected(e.to_string()))?;
        self.model
            .set_frozen_rows_count(sheet.0, rows)
            .map_err(rejected)?;
        self.model
            .set_frozen_columns_count(sheet.0, i32::from(cols))
            .map_err(rejected)
    }

    fn insert_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError> {
        let count = i32::try_from(count).map_err(|e| rejected(e.to_string()))?;
        self.model
            .insert_rows(sheet.0, row_i32(at), count)
            .map_err(rejected)
    }

    fn delete_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError> {
        let count = i32::try_from(count).map_err(|e| rejected(e.to_string()))?;
        self.model
            .delete_rows(sheet.0, row_i32(at), count)
            .map_err(rejected)
    }

    fn insert_columns(
        &mut self,
        sheet: SheetId,
        at: ColIdx,
        count: u16,
    ) -> Result<(), EngineError> {
        self.model
            .insert_columns(sheet.0, col_i32(at), i32::from(count))
            .map_err(rejected)
    }

    fn delete_columns(
        &mut self,
        sheet: SheetId,
        at: ColIdx,
        count: u16,
    ) -> Result<(), EngineError> {
        self.model
            .delete_columns(sheet.0, col_i32(at), i32::from(count))
            .map_err(rejected)
    }

    fn merged(&self, sheet: SheetId) -> Vec<Range> {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map(|ws| {
                ws.merge_cells
                    .iter()
                    .filter_map(|m| {
                        let range = Range::parse_a1(m);
                        if range.is_none() {
                            tracing::warn!(merge = %m, "ignoring a merged range that does not parse");
                        }
                        range
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn filled_cells(&self, sheet: SheetId) -> Vec<CellPos> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet.0) else {
            return Vec::new();
        };
        let mut cells: Vec<CellPos> = ws
            .sheet_data
            .iter()
            .flat_map(|(row, columns)| {
                columns.keys().map(move |col| {
                    CellPos::new(
                        RowIdx::clamped(i64::from(*row) - 1),
                        ColIdx::clamped(i64::from(*col) - 1),
                    )
                })
            })
            .collect();
        cells.retain(|pos| !self.input(sheet, *pos).is_empty());
        cells.sort_by_key(|pos| (pos.row, pos.col));
        cells
    }

    fn used_end(&self, sheet: SheetId) -> CellPos {
        self.model.used_end(sheet)
    }

    fn add_sheet(&mut self) -> Result<SheetId, EngineError> {
        self.model.new_sheet().map_err(rejected)?;
        let count = self.model.get_worksheets_properties().len();
        Ok(SheetId(u32::try_from(count.saturating_sub(1)).unwrap_or(0)))
    }

    // The copy lands right after the source, with formulas that pointed at the source
    // now pointing at the copy, as in Excel.
    fn duplicate_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError> {
        self.model.duplicate_sheet(sheet.0).map_err(rejected)
    }

    fn rename_sheet(&mut self, sheet: SheetId, name: &str) -> Result<(), EngineError> {
        self.model.rename_sheet(sheet.0, name).map_err(rejected)
    }

    fn delete_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError> {
        self.model.delete_sheet(sheet.0).map_err(rejected)
    }

    fn move_sheet(&mut self, sheet: SheetId, to: u32) -> Result<(), EngineError> {
        self.model.move_sheet(sheet.0, to).map_err(rejected)
    }

    fn to_xlsx(&self) -> Result<Vec<u8>, EngineError> {
        let xlsx = save_xlsx_to_writer(self.model.get_model(), Cursor::new(Vec::new()))
            .map(Cursor::into_inner)
            .map_err(|e| rejected(format!("{e:?}")))?;
        crate::empty_rows::restore_empty_rows(self.model.get_model(), xlsx)
    }
}

enum SortKey {
    Number(f64),
    Text(String),
    Bool(bool),
    Error,
    Blank,
}

impl SortKey {
    fn of(view: &CellView) -> SortKey {
        match view.kind {
            ValueKind::Number => view.number.map_or(SortKey::Blank, SortKey::Number),
            ValueKind::Text => SortKey::Text(view.text.to_lowercase()),
            ValueKind::Bool => SortKey::Bool(view.text.eq_ignore_ascii_case("true")),
            ValueKind::Error => SortKey::Error,
            ValueKind::Empty => SortKey::Blank,
        }
    }

    fn rank(&self) -> u8 {
        match self {
            SortKey::Number(_) => 0,
            SortKey::Text(_) => 1,
            SortKey::Bool(_) => 2,
            SortKey::Error => 3,
            SortKey::Blank => 4,
        }
    }

    fn compare(a: &SortKey, b: &SortKey, descending: bool) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        // Blanks stay last in both directions, as in Excel.
        match (a, b) {
            (SortKey::Blank, SortKey::Blank) => return Ordering::Equal,
            (SortKey::Blank, _) => return Ordering::Greater,
            (_, SortKey::Blank) => return Ordering::Less,
            _ => {}
        }
        let ascending = match (a, b) {
            (SortKey::Number(x), SortKey::Number(y)) => x.total_cmp(y),
            (SortKey::Text(x), SortKey::Text(y)) => x.cmp(y),
            (SortKey::Bool(x), SortKey::Bool(y)) => x.cmp(y),
            _ => a.rank().cmp(&b.rank()),
        };
        if descending {
            ascending.reverse()
        } else {
            ascending
        }
    }
}

// A least-squares line, as Excel's AutoFill trend: the value at step k is first + slope * k.
#[derive(Clone, Copy, Debug)]
struct Trend {
    first: f64,
    slope: f64,
}

impl Trend {
    fn fit(values: &[f64]) -> Trend {
        let n = values.len() as f64;
        let mean_x = (n - 1.0) / 2.0;
        let mean_y = values.iter().sum::<f64>() / n;
        let (mut num, mut den) = (0.0, 0.0);
        for (i, y) in values.iter().enumerate() {
            let dx = i as f64 - mean_x;
            num += dx * (y - mean_y);
            den += dx * dx;
        }
        let slope = if den == 0.0 { 0.0 } else { num / den };
        Trend {
            first: mean_y - slope * mean_x,
            slope,
        }
    }

    // A single value counts up by one, as Excel does for a date or "Item 1".
    fn fit_or_step(values: &[f64]) -> Trend {
        match values {
            [only] => Trend {
                first: *only,
                slope: 1.0,
            },
            _ => Trend::fit(values),
        }
    }

    fn at(self, step: f64) -> f64 {
        self.first + self.slope * step
    }
}

#[derive(Clone, Debug)]
enum Series {
    Numbers(Trend),
    Dates(Trend),
    Text {
        prefix: String,
        width: usize,
        trend: Trend,
    },
}

impl Series {
    fn at(&self, step: f64) -> String {
        match self {
            // Twelve decimals hide binary noise such as 0.30000000000000004.
            Series::Numbers(trend) => {
                let rounded = (trend.at(step) * 1e12).round() / 1e12;
                format!("{rounded}")
            }
            // Written as an ISO date, which the engine reads back as a date.
            Series::Dates(trend) => {
                iso_date(trend.at(step).round().clamp(0.0, MAX_DATE_SERIAL) as i64)
            }
            Series::Text {
                prefix,
                width,
                trend,
            } => {
                let number = trend.at(step).round().max(0.0) as u64;
                format!("{prefix}{number:0width$}")
            }
        }
    }
}

fn is_date_format(code: &str) -> bool {
    let lower = code.to_ascii_lowercase();
    lower != "general" && (lower.contains('d') || lower.contains('y'))
}

// "Item 007" splits into ("Item ", 7, 3); text without trailing digits does not split.
fn split_trailing_number(text: &str) -> Option<(String, u64, usize)> {
    let digits = text.len() - text.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 || digits > 15 || digits == text.len() {
        return None;
    }
    let (prefix, number) = text.split_at(text.len() - digits);
    Some((prefix.to_string(), number.parse().ok()?, digits))
}

// Excel serial (days since 1899-12-30) to yyyy-mm-dd, by the civil-from-days algorithm.
fn iso_date(serial: i64) -> String {
    let days = serial - 25_569 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn format_preview(value: f64, code: &str) -> Result<String, EngineError> {
    check_format_code(code)?;
    let locale = ironcalc::base::locale::get_locale(LOCALE).map_err(rejected)?;
    let formatted = ironcalc::base::formatter::format::format_number(value, code, locale);
    match formatted.error {
        Some(error) => Err(rejected(error)),
        None => Ok(formatted.text),
    }
}
