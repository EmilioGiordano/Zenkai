use std::io::Cursor;
use std::time::{Duration, Instant};

use ironcalc::base::expressions::types::{Area, CellReferenceIndex};
use ironcalc::base::types::{
    Cell, CellType, Color, DefaultSizes, FormulaValue, HorizontalAlignment, SpillValue, Style,
    VerticalAlignment,
};
use ironcalc::base::{BorderArea, ClipboardData, Model, Recalculation, UserModel};
use ironcalc::import::load_from_xlsx_bytes;

use crate::column_units;
use crate::edit_profile::{EditProfile, RecalcMode};
use crate::error::EngineError;
use crate::file::Unsupported;
use crate::inputs_plan::plan_inputs;
use crate::model::{
    CachedModel, HistoryStep, content_cells_up_to, content_cells_within, for_each_within,
};
use crate::rectangles::covering_rectangles;
use crate::save::{ModelData, SaveSnapshot, SaveTarget};
use crate::saved::{FileOnDisk, KnownFile, SavedSlot};
use crate::sheet_settings::{Carried, SheetSettings};
use crate::shown_cells::UNREADABLE_CELL;
use zenkai_types::{
    BorderPreset, CellPos, CellStyle, CellView, ColIdx, ColumnSpan, ContentSpan, Contents,
    DefinedName, HAlign, LineAxis, LineChange, LineOp, LineSize, Range, Rgb, RowIdx, SheetChange,
    SheetId, SheetInfo, SheetSizes, SheetVisibility, StyleChange, VAlign, ValueKind,
};

const LOCALE: &str = "en";
const MAX_FILL_CELLS: u64 = 1_000_000;
// IronCalc's undo history keeps about 1.5 KB per styled cell (measured: 2.8M cells cost
// 4.3 GB, 6.4 GB peak on undo), so styling is capped by memory, not by what the sheet holds.
const STYLE_HISTORY_BYTES_PER_CELL: u64 = 1536;
const STYLE_HISTORY_BUDGET_BYTES: u64 = 1536 * 1024 * 1024;
const MAX_STYLED_CELLS: u64 = STYLE_HISTORY_BUDGET_BYTES / STYLE_HISTORY_BYTES_PER_CELL;
pub(crate) const LANGUAGE: &str = "en";
// Stored heights are in points, which give Excel's pixels; widths are in `column_units`.
const PIXELS_PER_POINT: f64 = 4.0 / 3.0;
// IronCalc's setter takes its own pixels: the stored width times this factor.
const COLUMN_WIDTH_FACTOR: f64 = 9.0;
// Excel's tallest row.
const MAX_POINTS: f64 = 409.5;
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
    // Many rows or columns at once, in one undo step; the setters above scan the entries
    // for every line. A column run is `(first, last, pixels)`.
    fn set_row_heights(
        &mut self,
        sheet: SheetId,
        heights: &[(RowIdx, f32)],
    ) -> Result<(), EngineError>;
    fn set_column_widths(
        &mut self,
        sheet: SheetId,
        runs: &[(ColIdx, ColIdx, f32)],
    ) -> Result<(), EngineError>;
    // What one column or row measures, without reading the sizes of the whole sheet.
    fn column_size(&self, sheet: SheetId, col: ColIdx) -> LineSize;
    fn row_size(&self, sheet: SheetId, row: RowIdx) -> LineSize;
    // The size of every column or row of the sheet that has no entry of its own, as Excel
    // sets it for a whole-sheet selection. The pixels must be above zero. Rows that carry a
    // stored automatic height take it too, as Excel's rows do; rows and columns with a size
    // of their own are the caller's to set.
    fn set_default_column_width(&mut self, sheet: SheetId, pixels: f32) -> Result<(), EngineError>;
    fn set_default_row_height(&mut self, sheet: SheetId, pixels: f32) -> Result<(), EngineError>;
    fn frozen(&self, sheet: SheetId) -> (u32, u16);
    fn set_frozen(&mut self, sheet: SheetId, rows: u32, cols: u16) -> Result<(), EngineError>;
    fn gridlines_shown(&self, sheet: SheetId) -> bool;
    fn set_gridlines_shown(&mut self, sheet: SheetId, shown: bool) -> Result<(), EngineError>;
    fn headings_shown(&self, sheet: SheetId) -> bool;
    fn set_headings_shown(&mut self, sheet: SheetId, shown: bool) -> Result<(), EngineError>;
    fn insert_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError>;
    fn delete_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError>;
    fn insert_columns(&mut self, sheet: SheetId, at: ColIdx, count: u16)
    -> Result<(), EngineError>;
    fn delete_columns(&mut self, sheet: SheetId, at: ColIdx, count: u16)
    -> Result<(), EngineError>;
    fn merged(&self, sheet: SheetId) -> Vec<Range>;
    fn used_end(&self, sheet: SheetId) -> CellPos;
    fn defined_names(&self) -> Vec<DefinedName>;
    fn filled_cells(&self, sheet: SheetId) -> Vec<CellPos>;
    // The first `limit` filled cells inside `range`, by row; the rest of the sheet is not read.
    fn filled_cells_within(&self, sheet: SheetId, range: Range, limit: usize) -> Vec<CellPos>;
    fn add_sheet(&mut self) -> Result<SheetId, EngineError>;
    fn duplicate_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError>;
    fn rename_sheet(&mut self, sheet: SheetId, name: &str) -> Result<(), EngineError>;
    fn delete_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError>;
    fn move_sheet(&mut self, sheet: SheetId, to: u32) -> Result<(), EngineError>;
    fn to_xlsx(&self) -> Result<Vec<u8>, EngineError>;
}

pub struct Workbook {
    pub(crate) model: CachedModel,
    pub(crate) carried: Carried,
    pub(crate) saved: SavedSlot,
    // What the file held when it was opened, for a save that knows nothing newer.
    pub(crate) opened: Vec<Unsupported>,
}

// What a copy produced: the text Excel and other apps understand, plus the engine's
// own payload so a paste inside Zenkai keeps formulas (shifted) and styles.
#[derive(Clone, Debug)]
pub struct Copied {
    pub text: String,
    payload: serde_json::Value,
    styled: bool,
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
    load_from_xlsx_bytes(bytes, name, LOCALE, timezone()).map_err(read_error)
}

fn read_error(error: ironcalc::error::XlsxError) -> EngineError {
    match error {
        ironcalc::error::XlsxError::Workbook(message)
            if message.starts_with(ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX) =>
        {
            EngineError::TooLarge(message)
        }
        other => EngineError::InvalidFile(format!("{other:?}")),
    }
}

pub(crate) fn read_book_fast(
    bytes: &[u8],
    name: &str,
    inspect: &zenkai_xlsx_reader::Inspect<'_>,
) -> Result<ironcalc::base::types::Workbook, zenkai_xlsx_reader::ReadError> {
    zenkai_xlsx_reader::read_xlsx(bytes, name, LOCALE, timezone(), inspect)
        .map(|opened| opened.book)
}

pub(crate) fn read_book_streaming(
    path: &std::path::Path,
    name: &str,
    inspect: &zenkai_xlsx_reader::Inspect<'_>,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<zenkai_xlsx_reader::OpenedBook, zenkai_xlsx_reader::ReadError> {
    zenkai_xlsx_reader::read_xlsx_file_cancellable(path, name, LOCALE, timezone(), inspect, cancel)
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

fn profiled(
    model: &mut UserModel<'static>,
    change: impl FnOnce(&mut UserModel<'static>) -> Result<(), String>,
) -> Result<EditProfile, String> {
    model.pause_evaluation();
    let started = Instant::now();
    let changed = change(model);
    let edit = started.elapsed();
    model.resume_evaluation();
    let started = Instant::now();
    model.evaluate();
    let recalculation = started.elapsed();
    let mode = match model.get_model().last_recalculation() {
        Recalculation::Full => RecalcMode::Full,
        Recalculation::Incremental => RecalcMode::Incremental,
    };
    changed.map(|()| EditProfile {
        edit,
        recalculation,
        mode,
        warm_used_areas: Duration::ZERO,
    })
}

fn rejected(message: String) -> EngineError {
    EngineError::Rejected(message)
}

// A file can say anything: what is not a height is hidden, and what is too tall is the
// tallest row.
fn row_pixels(points: f64) -> f32 {
    if !points.is_finite() || points <= 0.0 {
        return 0.0;
    }
    (points.min(MAX_POINTS) * PIXELS_PER_POINT).round() as f32
}

// IronCalc's row height for `pixels`: Excel's tallest row is 409.5 points, and what is not
// a size maps to zero.
fn height_of(pixels: f32) -> f64 {
    let pixels = if pixels.is_finite() {
        f64::from(pixels)
    } else {
        0.0
    };
    (pixels / PIXELS_PER_POINT).clamp(0.0, MAX_POINTS) * ROW_HEIGHT_FACTOR
}

// IronCalc's own entries (a whole-column style) carry its default width without customWidth
// and draw at the sheet's width; any other stored width is the file's. `None` is no size
// of its own.
fn own_width(col: &ironcalc::base::types::Col) -> Option<f32> {
    if col.hidden {
        Some(0.0)
    } else if col.has_default_width() {
        None
    } else {
        Some(column_units::pixels_of(col.width))
    }
}

// A default of zero would hide every line, which a sheet default never does.
fn visible_pixels(pixels: f32) -> Result<f64, EngineError> {
    if pixels.is_finite() && pixels > 0.0 {
        Ok(f64::from(pixels))
    } else {
        Err(rejected(format!("{pixels} is not a size a line can have")))
    }
}

impl Workbook {
    // What a column and a row with no size of their own measure, when the sheet says.
    fn default_sizes_of(
        &self,
        ws: &ironcalc::base::types::Worksheet,
    ) -> (Option<f32>, Option<f32>) {
        let settings = self.carried.get(ws.sheet_id);
        let column = ws
            .default_sizes
            .column_width
            .or_else(|| settings.and_then(SheetSettings::file_column_width))
            .map(column_units::pixels_of);
        let row = ws
            .default_sizes
            .row_height
            .or_else(|| settings.and_then(SheetSettings::file_row_height))
            .map(row_pixels);
        (column, row)
    }

    fn change_default_sizes(
        &mut self,
        sheet: SheetId,
        change: impl FnOnce(&mut DefaultSizes),
    ) -> Result<(), EngineError> {
        self.model
            .edit_keeping_cells(|model| {
                let mut sizes = model.get_default_sizes(sheet.0)?;
                change(&mut sizes);
                model.set_default_sizes(sheet.0, sizes)
            })
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

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
        Ok(move |pos: CellPos| sheet_data.get(row_i32(pos.row), col_i32(pos.col)))
    }
}

pub(crate) fn row_i32(row: RowIdx) -> i32 {
    row.get() as i32 + 1
}

pub(crate) fn col_i32(col: ColIdx) -> i32 {
    i32::from(col.get()) + 1
}

// Rows of one length written as one engine paste. The engine's TSV reader drops records
// whose width differs from the first, so a block of rows of mixed lengths is pasted as
// one run per length, never padded: padding a wide row's block makes the work follow the
// area, not the fields in it.
fn paste_run(
    model: &mut UserModel<'static>,
    sheet: SheetId,
    origin: CellPos,
    rows: &[Vec<String>],
) -> Result<(), EngineError> {
    let width = rows[0].len();
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .from_writer(Vec::new());
    for row in rows {
        writer
            .write_record(row)
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
        height: i32::try_from(rows.len()).map_err(|e| rejected(e.to_string()))?,
    };
    let end = CellPos::new(
        origin.row.offset(rows.len() as i64 - 1),
        origin.col.offset(width as i64 - 1),
    );
    select(model, sheet, Range::new(origin, end))?;
    model.paste_csv_string(&area, &tsv).map_err(rejected)
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
    // What a command wrote or cleared, for a save over a CSV to know which cells are the
    // user's. Logged before the write: a command that fails leaves a cell marked that was not
    // changed, which is written as shown, never lost.
    fn log_content(&mut self, sheet: SheetId, range: Range) {
        match self.sheet_key(sheet) {
            Some(key) => self
                .model
                .record_change(key, SheetChange::Content(ContentSpan::of(range))),
            None => self.model.record_unknown(),
        }
    }

    fn log_lines(&mut self, sheet: SheetId, op: LineOp) {
        match self.sheet_key(sheet) {
            Some(key) => self.model.record_change(key, SheetChange::Lines(op)),
            None => self.model.record_unknown(),
        }
    }

    // Whether copying cells of `range` carries a style, resolved as IronCalc's copy resolves it:
    // the cell's own, else its row's or column's. A style counts when it differs from the
    // workbook's default, which is what a cell with the first style looks like. Cheap: one
    // look per distinct style, and a sheet that cannot be read counts as styled.
    fn range_has_style(&self, sheet: SheetId, range: Range) -> bool {
        const MOST_STYLES_COMPARED: usize = 64;
        let model = self.model.get_model();
        let Ok(worksheet) = model.workbook.worksheet(sheet.0) else {
            return true;
        };
        if self.model.sheet_has_line_styles(sheet) {
            let (first_row, last_row) = (row_i32(range.start.row), row_i32(range.end.row));
            let (first_col, last_col) = (col_i32(range.start.col), col_i32(range.end.col));
            let styled_row = worksheet.rows.iter().any(|row| {
                row.custom_format && row.s != 0 && (first_row..=last_row).contains(&row.r)
            });
            let styled_column = worksheet.cols.iter().any(|col| {
                col.style.is_some_and(|style| style != 0)
                    && col.min <= last_col
                    && col.max >= first_col
            });
            if styled_row || styled_column {
                return true;
            }
        }
        let mut seen: Vec<(i32, i32, i32)> = Vec::new();
        for_each_within(&worksheet.sheet_data, range, |row, col, cell| {
            let style = cell.get_style();
            if seen.len() <= MOST_STYLES_COMPARED && !seen.iter().any(|(known, ..)| *known == style)
            {
                seen.push((style, row, col));
            }
        });
        if seen.len() > MOST_STYLES_COMPARED {
            return true;
        }
        let style_at = |row: i32, col: i32| model.get_style_for_cell(sheet.0, row, col).ok();
        let default = seen
            .iter()
            .find(|(style, ..)| *style == 0)
            .and_then(|(_, row, col)| style_at(*row, *col))
            .unwrap_or_default();
        seen.iter()
            .filter(|(style, ..)| *style != 0)
            .any(|(_, row, col)| style_at(*row, *col).is_none_or(|style| style != default))
    }

    // Fills each line of `rest` as one undo step. A line whose source holds a plain formula
    // is filled by sharing it, so no text is built or parsed per cell; any other line gets
    // the text `text_at` gives for each of its cells.
    fn fill_lines(
        &mut self,
        sheet: SheetId,
        rest: Range,
        lines: Vec<(Option<CellPos>, Range)>,
        number_formats: &[(Range, String)],
        style_sources: &[(CellPos, Range)],
        text_at: impl Fn(&Model, CellPos) -> Result<String, EngineError>,
    ) -> Result<(), EngineError> {
        self.model.edit_keeping_cells(|model| {
            select(model, sheet, rest)?;
            model.begin_edit_group().map_err(rejected)
        })?;
        let filled = self
            .fill_lines_in_group(sheet, lines, text_at)
            .and_then(|()| {
                number_formats.iter().try_for_each(|(line, code)| {
                    self.model
                        .write_within(sheet, *line, |model| {
                            model.update_range_style(&area(sheet, *line), "num_fmt", code)
                        })
                        .map_err(rejected)
                })
            })
            .and_then(|()| {
                style_sources.iter().try_for_each(|(source, line)| {
                    self.model
                        .write_within(sheet, *line, |model| {
                            model.fill_style(
                                (row_i32(source.row), col_i32(source.col)),
                                &area(sheet, *line),
                            )
                        })
                        .map_err(rejected)
                })
            });
        let ended = self
            .model
            .edit_keeping_cells(|model| model.end_edit_group())
            .map_err(rejected);
        filled?;
        ended?;
        self.model
            .select_without_invalidation(|model| select(model, sheet, rest))
    }

    fn fill_lines_in_group(
        &mut self,
        sheet: SheetId,
        lines: Vec<(Option<CellPos>, Range)>,
        text_at: impl Fn(&Model, CellPos) -> Result<String, EngineError>,
    ) -> Result<(), EngineError> {
        for (source, line) in lines {
            if let Some(source) = source {
                let shared = self
                    .model
                    .write_within(sheet, line, |model| {
                        model.fill_formula(
                            (row_i32(source.row), col_i32(source.col)),
                            &area(sheet, line),
                        )
                    })
                    .map_err(rejected)?;
                if shared {
                    continue;
                }
            }
            let model = self.model.get_model();
            let texts = (line.start.row.get()..=line.end.row.get())
                .flat_map(|row| {
                    (line.start.col.get()..=line.end.col.get()).map(move |col| {
                        CellPos::new(
                            RowIdx::clamped(i64::from(row)),
                            ColIdx::clamped(i64::from(col)),
                        )
                    })
                })
                .map(|pos| text_at(model, pos))
                .collect::<Result<Vec<_>, _>>()?;
            let rows: Vec<Vec<String>> = if line.cols() == 1 {
                texts.into_iter().map(|text| vec![text]).collect()
            } else {
                vec![texts]
            };
            self.set_inputs(sheet, line.start, &rows)?;
        }
        Ok(())
    }

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
        Ok(content_cells_within(&ws.sheet_data, range))
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
            return Some(Series::Dates {
                trend: Trend::fit_or_step(&serials),
                format: cells[0].1.style.num_fmt.clone(),
            });
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
            carried: Carried::default(),
            saved: SavedSlot::default(),
            opened: Vec::new(),
        })
    }

    pub fn from_xlsx_bytes(bytes: &[u8], name: &str) -> Result<Workbook, EngineError> {
        Workbook::from_book(read_book(bytes, name)?)
    }

    pub(crate) fn from_book(
        book: ironcalc::base::types::Workbook,
    ) -> Result<Workbook, EngineError> {
        let model = engine_model(book)?;
        evaluated(model, Carried::default(), SavedSlot::default(), Vec::new())
    }

    // `settings` are by position among the file's worksheets, the order IronCalc loads them
    // in. The file is remembered by the values it stores, before the engine recalculates
    // them, so a sheet whose stored values were stale is written again by the next save.
    pub(crate) fn from_file(
        book: ironcalc::base::types::Workbook,
        settings: Vec<SheetSettings>,
        file: Option<FileOnDisk<'_>>,
        known: Option<KnownFile>,
        opened: &[Unsupported],
    ) -> Result<Workbook, EngineError> {
        let model = tracing::info_span!("open_engine_model").in_scope(|| engine_model(book))?;
        let carried = carried_by_position(&model, settings);
        let saved = SavedSlot::default();
        saved.set_known(known);
        if let Some(file) = file {
            saved.set(
                tracing::info_span!("open_saved_parts")
                    .in_scope(|| crate::saved::opened(file, ModelData::of(&model), &carried)),
            );
        }
        evaluated(model, carried, saved, opened.to_vec())
    }

    // The identity of a sheet that its place in the workbook does not change.
    pub fn sheet_key(&self, sheet: SheetId) -> Option<u32> {
        self.model.sheet_key(sheet.0)
    }

    fn rows_moved(&mut self, sheet: SheetId, from: RowIdx) {
        if let Some(id) = self.sheet_key(sheet) {
            self.carried.rows_moved(id, from.get() + 1);
        }
    }

    // The ids of the sheets a replayed step moves rows or columns on, read before the
    // replay while the step's sheet indices still hold; every sheet when the step also
    // changes the sheets themselves.
    fn replay_moves(&mut self, step: HistoryStep) -> Option<Vec<u32>> {
        if !self.model.can_redo() {
            self.carried.redo_emptied();
        }
        let moved = match step {
            HistoryStep::Undo => self.model.sheets_moved_by_undo(),
            HistoryStep::Redo => self.model.sheets_moved_by_redo(),
        };
        moved?
            .into_iter()
            .map(|index| self.model.sheet_key(index))
            .collect()
    }

    // A row-level save copies what the model does not hold from the file, which a
    // replayed move would leave in the wrong place. Only the sheets that step moved
    // count; a failed replay may have stopped anywhere, so it counts every sheet.
    fn history_replayed(&mut self, moved: Option<Vec<u32>>, replayed: bool, step: HistoryStep) {
        if moved.as_ref().is_some_and(|ids| !ids.is_empty()) {
            match replayed {
                true => self.carried.move_replayed(step),
                false => self.carried.move_replay_failed(step),
            }
        }
        let sheet_ids = match moved.filter(|_| replayed) {
            Some(ids) => ids,
            None => {
                let worksheets = &self.model.get_model().workbook.worksheets;
                worksheets.iter().map(|ws| ws.sheet_id).collect()
            }
        };
        self.carried.history_replayed(sheet_ids);
    }

    fn columns_moved(&mut self, sheet: SheetId, from: ColIdx) {
        if let Some(id) = self.sheet_key(sheet) {
            self.carried.columns_moved(id, u32::from(from.get()) + 1);
        }
    }

    // Copies what a save of this workbook to `path` writes; take it under the read lock and
    // write it with `save_xlsx` after releasing the lock.
    pub fn save_snapshot(
        &self,
        path: &std::path::Path,
        target: SaveTarget,
    ) -> Result<SaveSnapshot, EngineError> {
        let model = self.model.get_model();
        crate::save::snapshot(
            path,
            target,
            model,
            &self.carried,
            &self.saved,
            &self.opened,
        )
    }

    // The one way a single cell is written: `write` gets the model and the cell's row and
    // column, and decides when the workbook is evaluated.
    fn write_cell<T>(
        &mut self,
        sheet: SheetId,
        pos: CellPos,
        text: &str,
        write: impl FnOnce(&mut UserModel<'static>, i32, i32) -> Result<T, String>,
    ) -> Result<T, EngineError> {
        check_input(text)?;
        self.model
            .write_within(sheet, Range::new(pos, pos), |model| {
                write(model, row_i32(pos.row), col_i32(pos.col))
            })
            .map_err(rejected)
    }

    // `set_input` with the edit, the recalculation and the refresh of the used areas timed
    // apart, for the edit benchmark.
    pub fn set_input_profiled(
        &mut self,
        sheet: SheetId,
        pos: CellPos,
        text: &str,
    ) -> Result<EditProfile, EngineError> {
        let profile = self.write_cell(sheet, pos, text, |model, row, col| {
            profiled(model, |model| model.set_user_input(sheet.0, row, col, text))
        })?;
        Ok(self.warmed(profile))
    }

    // `edit` must not fill, extend or move rows or columns: a fill opens its own undo step
    // and the carried row data counts one move per step. Inside a step that is open, the
    // edits join it.
    pub fn as_one_undo_step<T>(
        &mut self,
        edit: impl FnOnce(&mut Workbook) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        if self.model.is_edit_group_open() {
            return edit(self);
        }
        self.model
            .edit_keeping_cells(|model| {
                model.begin_edit_group()?;
                // A group pauses evaluation; each edit here evaluates as it would alone, so
                // reads between them (the widening) see computed formula values.
                model.resume_evaluation();
                Ok::<(), String>(())
            })
            .map_err(rejected)?;
        let edited = edit(self);
        let ended = self
            .model
            .edit_keeping_cells(|model| model.end_edit_group())
            .map_err(rejected);
        let value = edited?;
        ended?;
        Ok(value)
    }

    // `undo` and `redo` timed like `set_input_profiled`, for the edit benchmark.
    pub fn undo_profiled(&mut self) -> Result<EditProfile, EngineError> {
        self.step_profiled(HistoryStep::Undo)
    }

    pub fn redo_profiled(&mut self) -> Result<EditProfile, EngineError> {
        self.step_profiled(HistoryStep::Redo)
    }

    fn step_profiled(&mut self, step: HistoryStep) -> Result<EditProfile, EngineError> {
        let moved = self.replay_moves(step);
        let profile = self
            .model
            .step(step, |model, take| profiled(model, take))
            .map_err(rejected);
        self.history_replayed(moved, profile.is_ok(), step);
        Ok(self.warmed(profile?))
    }

    fn warmed(&self, mut profile: EditProfile) -> EditProfile {
        let started = Instant::now();
        self.warm_used_areas();
        profile.warm_used_areas = started.elapsed();
        profile
    }

    // When the file the workbook came from was last read or written by it; `None` if it
    // did not come from a file it can copy from.
    pub fn file_stamp(&self) -> Option<crate::saved::Stamp> {
        self.saved.current().map(|file| file.stamp)
    }

    pub fn format_edits(&self) -> crate::model::FormatEdits {
        self.model.format_edits()
    }

    // The changes made after the one numbered `seen`; `None` when some were dropped.
    pub fn changes_since(&self, seen: u64) -> Option<Vec<zenkai_types::LoggedChange>> {
        self.model.changes_since(seen)
    }

    // For a workbook built with the commands a change is logged for, once it is built.
    pub fn forget_changes(&mut self) {
        self.model.forget_changes();
    }

    // For what typing a cell does on its own (widening a column, wrapping text): it is content
    // entry, so the commands it runs are not counted as formatting.
    pub fn without_counting<T>(&mut self, edit: impl FnOnce(&mut Workbook) -> T) -> T {
        let before = self.model.format_edits();
        let result = edit(self);
        self.model.set_format_edits(before);
        result
    }

    // Call on a background thread after building or editing, so the UI thread never walks a sheet.
    pub fn warm_used_areas(&self) {
        for sheet in self.sheets() {
            self.model.used_end(sheet.id);
        }
    }

    #[cfg(test)]
    pub(crate) fn walked_end(&self, sheet: SheetId) -> Option<CellPos> {
        self.model.walked_end(sheet)
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
        let Some(text) = self.formatted_text(sheet, pos) else {
            return CellView {
                text: UNREADABLE_CELL.to_string(),
                kind: ValueKind::Error,
                ..CellView::default()
            };
        };
        let stored_type = model
            .workbook
            .worksheet(sheet.0)
            .map(|worksheet| worksheet.cell(row, col).map(Cell::get_type));
        let is_error = matches!(stored_type, Ok(Some(CellType::ErrorValue)));
        let (kind, number) = match model.get_cell_value_by_index(sheet.0, row, col) {
            Ok(ironcalc::base::cell::CellValue::Number(n)) => (ValueKind::Number, Some(n)),
            Ok(ironcalc::base::cell::CellValue::Boolean(_)) => (ValueKind::Bool, None),
            Ok(ironcalc::base::cell::CellValue::String(_)) if is_error => (ValueKind::Error, None),
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
            Some(Cell::NumberCell { v, .. }) => Contents::Number(*v),
            Some(Cell::SpillCell(spill)) => match spill.v {
                SpillValue::Number(v) => Contents::Number(v),
                _ => Contents::NonNumeric,
            },
            Some(cell) => match cell.formula_value() {
                Some(FormulaValue::Number(v)) => Contents::Number(*v),
                _ => Contents::NonNumeric,
            },
        })
    }

    fn input(&self, sheet: SheetId, pos: CellPos) -> String {
        self.model
            .get_cell_content(sheet.0, row_i32(pos.row), col_i32(pos.col))
            .unwrap_or_default()
    }

    fn set_input(&mut self, sheet: SheetId, pos: CellPos, text: &str) -> Result<(), EngineError> {
        self.log_content(sheet, Range::single(pos));
        self.write_cell(sheet, pos, text, |model, row, col| {
            model.set_user_input(sheet.0, row, col, text)
        })
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
        let end = CellPos::new(
            origin
                .row
                .offset(i64::try_from(height).unwrap_or(i64::MAX) - 1),
            origin
                .col
                .offset(i64::try_from(width).unwrap_or(i64::MAX) - 1),
        );
        let target = Range::new(origin, end);
        self.log_content(sheet, target);
        let plan = {
            let sheet_data = &self
                .model
                .get_model()
                .workbook
                .worksheet(sheet.0)
                .map_err(|_| EngineError::UnknownSheet(sheet))?
                .sheet_data;
            plan_inputs(sheet_data, origin, rows, width as usize)?
        };
        self.model.write_within(sheet, target, |model| {
            // Inside a caller's edit group the begin fails and the caller ends the group.
            let own_group = model.begin_edit_group().is_ok();
            let was_paused = model.is_evaluation_paused();
            model.pause_evaluation();
            let mut written = plan.tail.iter().try_for_each(|pos| {
                model
                    .range_clear_contents(&Area {
                        sheet: sheet.0,
                        row: row_i32(pos.row),
                        column: col_i32(pos.col),
                        width: 1,
                        height: 1,
                    })
                    .map_err(rejected)
            });
            for run in &plan.runs {
                if written.is_err() {
                    break;
                }
                written = paste_run(model, sheet, run.origin, run.rows);
            }
            if written.is_ok() {
                written = select(model, sheet, target);
            }
            let ended = if own_group {
                model.end_edit_group().map_err(rejected)
            } else {
                if !was_paused {
                    model.resume_evaluation();
                    model.evaluate();
                }
                Ok(())
            };
            written.and(ended)
        })
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
            .inspect(|()| self.model.count_moved_edit())
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
        // The typed text is extended, not the cell it would produce, so a date, a percent
        // or quoted text fills as typed, as in Excel.
        let source = CellReferenceIndex {
            sheet: sheet.0,
            row: row_i32(from.row),
            column: col_i32(from.col),
        };
        let rows = self.model.select_without_invalidation(|model| {
            (range.start.row.get()..=range.end.row.get())
                .map(|row| {
                    (range.start.col.get()..=range.end.col.get())
                        .map(|col| {
                            let target = CellReferenceIndex {
                                sheet: sheet.0,
                                row: row as i32 + 1,
                                column: i32::from(col) + 1,
                            };
                            model
                                .extend_copied_value(text, &source, &target)
                                .map_err(rejected)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
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
        self.log_content(sheet, rest);
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
        let (source_rows, source_cols) = (source.rows(), u32::from(source.cols()));
        let lines_to_fill = if down {
            (rest.start.col.get()..=rest.end.col.get())
                .map(|col| {
                    let col = ColIdx::clamped(i64::from(col));
                    let shared = (source_rows == 1
                        && matches!(
                            lines.get(usize::from(col.get() - source.start.col.get())),
                            Some(None)
                        ))
                    .then(|| CellPos::new(source.start.row, col));
                    let line = Range::new(
                        CellPos::new(rest.start.row, col),
                        CellPos::new(rest.end.row, col),
                    );
                    (shared, line)
                })
                .collect()
        } else {
            (rest.start.row.get()..=rest.end.row.get())
                .map(|row| {
                    let row = RowIdx::clamped(i64::from(row));
                    let shared = (source_cols == 1
                        && matches!(
                            lines.get((row.get() - source.start.row.get()) as usize),
                            Some(None)
                        ))
                    .then(|| CellPos::new(row, source.start.col));
                    let line = Range::new(
                        CellPos::new(row, rest.start.col),
                        CellPos::new(row, rest.end.col),
                    );
                    (shared, line)
                })
                .collect()
        };
        let date_formats = lines
            .iter()
            .enumerate()
            .filter_map(|(index, series)| match series {
                Some(Series::Dates { format, .. }) => {
                    let line = if down {
                        let col = source.start.col.offset(index as i64);
                        Range::new(
                            CellPos::new(rest.start.row, col),
                            CellPos::new(rest.end.row, col),
                        )
                    } else {
                        let row = source.start.row.offset(index as i64);
                        Range::new(
                            CellPos::new(row, rest.start.col),
                            CellPos::new(row, rest.end.col),
                        )
                    };
                    Some((line, format.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let styled = self.range_has_style(sheet, source);
        self.fill_lines(
            sheet,
            rest,
            lines_to_fill,
            &date_formats,
            &[],
            |model, pos| {
                let (row, col) = (pos.row.get(), pos.col.get());
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
                    let offset = (u32::from(col) - u32::from(source.start.col.get())) % source_cols;
                    (row, u32::from(source.start.col.get()) + offset)
                };
                model
                    .extend_to(
                        sheet.0,
                        from_row as i32 + 1,
                        from_col as i32 + 1,
                        row_i32(pos.row),
                        col_i32(pos.col),
                    )
                    .map_err(rejected)
            },
        )
        .inspect(|()| {
            if styled {
                self.model.count_copy_edit();
            }
        })
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
        let styled = self.range_has_style(sheet, range);
        Ok(Copied {
            text,
            payload,
            styled,
        })
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
        let (first_row, first_col, last_row, last_col) = source_range;
        let (height, width) = (
            i64::from(last_row) - i64::from(first_row),
            i64::from(last_col) - i64::from(first_col),
        );
        self.log_content(
            sheet,
            Range::new(
                at,
                CellPos::new(at.row.offset(height), at.col.offset(width)),
            ),
        );
        if cut {
            let corner = |row: i32, col: i32| {
                CellPos::new(
                    RowIdx::clamped(i64::from(row) - 1),
                    ColIdx::clamped(i64::from(col) - 1),
                )
            };
            self.log_content(
                SheetId(source_sheet),
                Range::new(corner(first_row, first_col), corner(last_row, last_col)),
            );
        }
        self.model
            .paste_from_clipboard(source_sheet, source_range, &data, cut)
            .map_err(rejected)
            .inspect(|()| {
                if copied.styled {
                    self.model.count_copy_edit();
                }
                // Other cells' formulas that pointed into the cut range were rewritten too,
                // and none of them is logged.
                if cut {
                    self.model.record_unknown();
                }
            })
    }

    // Ctrl+D / Ctrl+R: the first row (or column) of the target is extended over the rest
    // with references shifted and its cells' formats copied whole, as in Excel; a single
    // row (or column) extends the one above (or left).
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
        self.log_content(sheet, rest);
        let lines: Vec<(Option<CellPos>, Range)> = if down {
            (rest.start.col.get()..=rest.end.col.get())
                .map(|col| {
                    let col = ColIdx::clamped(i64::from(col));
                    let source = CellPos::new(RowIdx::clamped(i64::from(source_line)), col);
                    let line = Range::new(
                        CellPos::new(rest.start.row, col),
                        CellPos::new(rest.end.row, col),
                    );
                    (Some(source), line)
                })
                .collect()
        } else {
            (rest.start.row.get()..=rest.end.row.get())
                .map(|row| {
                    let row = RowIdx::clamped(i64::from(row));
                    let source = CellPos::new(row, ColIdx::clamped(i64::from(source_line)));
                    let line = Range::new(
                        CellPos::new(row, rest.start.col),
                        CellPos::new(row, rest.end.col),
                    );
                    (Some(source), line)
                })
                .collect()
        };
        let style_sources: Vec<(CellPos, Range)> = lines
            .iter()
            .filter_map(|(source, line)| source.map(|source| (source, *line)))
            .collect();
        let styled = match (
            style_sources.iter().map(|(source, _)| source.row).min(),
            style_sources.iter().map(|(source, _)| source.row).max(),
            style_sources.iter().map(|(source, _)| source.col).min(),
            style_sources.iter().map(|(source, _)| source.col).max(),
        ) {
            (Some(first_row), Some(last_row), Some(first_col), Some(last_col)) => self
                .range_has_style(
                    sheet,
                    Range::new(
                        CellPos::new(first_row, first_col),
                        CellPos::new(last_row, last_col),
                    ),
                ),
            _ => false,
        };
        self.fill_lines(sheet, rest, lines, &[], &style_sources, |model, pos| {
            let (from_row, from_col) = if down {
                (source_line, u32::from(pos.col.get()))
            } else {
                (pos.row.get(), source_line)
            };
            model
                .extend_to(
                    sheet.0,
                    from_row as i32 + 1,
                    from_col as i32 + 1,
                    row_i32(pos.row),
                    col_i32(pos.col),
                )
                .map_err(rejected)
        })
        .inspect(|()| {
            if styled {
                self.model.count_copy_edit();
            }
        })
    }

    fn clear_formats(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        let Some(target) = self.style_target(sheet, range)? else {
            return Ok(());
        };
        let cleared = self
            .model
            .range_clear_formatting(&area(sheet, target))
            .map_err(rejected);
        if cleared.is_ok() {
            self.model.count_number_format_edit();
            self.model.count_style_edit();
        }
        cleared
    }

    fn clear_all(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        self.log_content(sheet, range);
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

    // IronCalc visits every cell of an area it clears, so only the cells holding contents
    // are cleared, as rectangles of them: the work and the undo record follow the contents,
    // never the selection (a whole sheet has 17 billion cells), and gaps get no empty cells
    // that would grow the used area. All rectangles go in one call, so one undo step.
    fn clear(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        self.log_content(sheet, range);
        let cells = self.content_cells_in(sheet, range)?;
        if cells.is_empty() {
            return Ok(());
        }
        let areas: Vec<Area> = covering_rectangles(cells)
            .into_iter()
            .map(|rectangle| area(sheet, rectangle))
            .collect();
        self.model
            .edit_keeping_cells(|model| {
                model.range_clear_contents_of_areas(&area(sheet, range), &areas)
            })
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
            StyleChange::Borders(preset) => {
                let applied = self.apply_borders(sheet, range, preset);
                if applied.is_ok() {
                    self.model.count_style_edit();
                }
                return applied;
            }
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
        let applied = self
            .model
            .write_within(sheet, target, |model| {
                model.update_range_style(&area(sheet, target), path, &value)
            })
            .map_err(rejected);
        if applied.is_ok() {
            if matches!(change, StyleChange::NumberFormat(_)) {
                self.model.count_number_format_edit();
            } else {
                self.model.count_style_edit();
            }
        }
        applied
    }

    fn set_number_format(
        &mut self,
        sheet: SheetId,
        range: Range,
        code: &str,
    ) -> Result<(), EngineError> {
        check_format_code(code)?;
        let target = self.formattable(sheet, range)?;
        let applied = self
            .model
            .write_within(sheet, target, |model| {
                model.update_range_style(&area(sheet, target), "num_fmt", code)
            })
            .map_err(rejected);
        if applied.is_ok() {
            self.model.count_number_format_edit();
        }
        applied
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
        for (pos, _) in inputs {
            self.log_content(sheet, Range::single(*pos));
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
            .edit_keeping_cells(|model| {
                model.set_rows_hidden_in_bulk(
                    sheet.0,
                    row_i32(range.start.row),
                    row_i32(range.end.row),
                    hidden,
                )
            })
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn set_columns_hidden(
        &mut self,
        sheet: SheetId,
        range: Range,
        hidden: bool,
    ) -> Result<(), EngineError> {
        self.model
            .edit_keeping_cells(|model| {
                model.set_columns_hidden(
                    sheet.0,
                    col_i32(range.start.col),
                    col_i32(range.end.col),
                    hidden,
                )
            })
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn undo(&mut self) -> Result<(), EngineError> {
        let moved = self.replay_moves(HistoryStep::Undo);
        let undone = self
            .model
            .step(HistoryStep::Undo, |model, take| take(model))
            .map_err(rejected);
        self.history_replayed(moved, undone.is_ok(), HistoryStep::Undo);
        undone
    }

    fn redo(&mut self) -> Result<(), EngineError> {
        let moved = self.replay_moves(HistoryStep::Redo);
        let redone = self
            .model
            .step(HistoryStep::Redo, |model, take| take(model))
            .map_err(rejected);
        self.history_replayed(moved, redone.is_ok(), HistoryStep::Redo);
        redone
    }

    fn sizes(&self, sheet: SheetId) -> SheetSizes {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet.0) else {
            return SheetSizes::default();
        };
        // Inserting columns keeps the ranges it pushes past the sheet, so undo restores them.
        let entries = ws
            .cols
            .iter()
            .filter(|c| c.min <= i32::from(zenkai_types::MAX_COLS))
            .map(|c| (c.min.max(1), c.max, own_width(c)));
        let mut columns: Vec<ColumnSpan> = column_units::first_wins(entries)
            .into_iter()
            .filter_map(|(first, last, width)| {
                Some(ColumnSpan {
                    first: ColIdx::clamped(i64::from(first) - 1),
                    last: ColIdx::clamped(i64::from(last) - 1),
                    width: width?,
                })
            })
            .collect();
        columns.sort_by_key(|span| span.first);
        let rows = ws
            .rows
            .iter()
            .filter(|r| r.custom_height || r.hidden)
            .map(|r| {
                let height = if r.hidden { 0.0 } else { row_pixels(r.height) };
                (RowIdx::clamped(i64::from(r.r) - 1), height)
            })
            .collect();
        let (default_column_width, default_row_height) = self.default_sizes_of(ws);
        SheetSizes {
            columns,
            rows,
            default_column_width,
            default_row_height,
        }
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
        let width = column_units::stored_width(pixels);
        let column = col_i32(col);
        self.model
            .edit_keeping_cells(|model| {
                model.set_columns_width(sheet.0, column, column, width * COLUMN_WIDTH_FACTOR)
            })
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn set_row_height(
        &mut self,
        sheet: SheetId,
        row: RowIdx,
        pixels: f32,
    ) -> Result<(), EngineError> {
        let row = row_i32(row);
        let height = height_of(pixels);
        self.model
            .edit_keeping_cells(|model| model.set_rows_height(sheet.0, row, row, height))
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn set_default_column_width(&mut self, sheet: SheetId, pixels: f32) -> Result<(), EngineError> {
        let pixels = visible_pixels(pixels)?;
        let width = column_units::stored_width(pixels);
        let styled: Vec<(ColIdx, ColIdx, f32)> = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map_err(rejected)?
            .cols
            .iter()
            .filter(|col| !col.hidden && col.has_default_width())
            .map(|col| {
                (
                    ColIdx::clamped(i64::from(col.min) - 1),
                    ColIdx::clamped(i64::from(col.max) - 1),
                    pixels as f32,
                )
            })
            .collect();
        self.as_one_undo_step(|book| {
            book.change_default_sizes(sheet, |sizes| sizes.column_width = Some(width))?;
            book.set_column_widths(sheet, &styled)
        })
    }

    fn set_default_row_height(&mut self, sheet: SheetId, pixels: f32) -> Result<(), EngineError> {
        let points = (visible_pixels(pixels)? / PIXELS_PER_POINT).clamp(0.0, MAX_POINTS);
        let automatic: Vec<i32> = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map_err(rejected)?
            .rows
            .iter()
            .filter(|row| !row.custom_height && !row.hidden)
            .map(|row| row.r)
            .collect();
        self.as_one_undo_step(|book| {
            book.change_default_sizes(sheet, |sizes| sizes.row_height = Some(points))?;
            book.model
                .edit_keeping_cells(|model| {
                    let heights: Vec<(i32, f64)> = automatic
                        .iter()
                        .map(|row| (*row, points * ROW_HEIGHT_FACTOR))
                        .collect();
                    model.set_row_heights(sheet.0, &heights)
                })
                .map_err(rejected)
        })
    }

    fn set_row_heights(
        &mut self,
        sheet: SheetId,
        heights: &[(RowIdx, f32)],
    ) -> Result<(), EngineError> {
        let heights: Vec<(i32, f64)> = heights
            .iter()
            .map(|(row, pixels)| (row_i32(*row), height_of(*pixels)))
            .collect();
        self.model
            .edit_keeping_cells(|model| model.set_row_heights(sheet.0, &heights))
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn set_column_widths(
        &mut self,
        sheet: SheetId,
        runs: &[(ColIdx, ColIdx, f32)],
    ) -> Result<(), EngineError> {
        let runs: Vec<(i32, i32, f64)> = runs
            .iter()
            .map(|(first, last, pixels)| {
                let width = column_units::stored_width(f64::from(*pixels));
                (col_i32(*first), col_i32(*last), width * COLUMN_WIDTH_FACTOR)
            })
            .collect();
        self.model
            .edit_keeping_cells(|model| model.set_column_widths(sheet.0, &runs))
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn column_size(&self, sheet: SheetId, col: ColIdx) -> LineSize {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet.0) else {
            return LineSize {
                pixels: zenkai_types::DEFAULT_COL_WIDTH,
                own: false,
            };
        };
        let at = col_i32(col);
        let own = ws
            .cols
            .iter()
            .find(|entry| entry.min <= at && at <= entry.max)
            .and_then(own_width);
        match own {
            Some(pixels) => LineSize { pixels, own: true },
            None => LineSize {
                pixels: self
                    .default_sizes_of(ws)
                    .0
                    .unwrap_or(zenkai_types::DEFAULT_COL_WIDTH),
                own: false,
            },
        }
    }

    fn row_size(&self, sheet: SheetId, row: RowIdx) -> LineSize {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet.0) else {
            return LineSize {
                pixels: zenkai_types::DEFAULT_ROW_HEIGHT,
                own: false,
            };
        };
        let at = row_i32(row);
        match ws.rows.iter().find(|entry| entry.r == at) {
            Some(entry) if entry.hidden => LineSize {
                pixels: 0.0,
                own: true,
            },
            Some(entry) if entry.custom_height => LineSize {
                pixels: row_pixels(entry.height),
                own: true,
            },
            _ => LineSize {
                pixels: self
                    .default_sizes_of(ws)
                    .1
                    .unwrap_or(zenkai_types::DEFAULT_ROW_HEIGHT),
                own: false,
            },
        }
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
            .edit_keeping_cells(|model| {
                // Paused before the group so closing it does not evaluate: freezing panes
                // changes no value, and an evaluation would re-roll volatile formulas.
                let was_paused = model.is_evaluation_paused();
                model.pause_evaluation();
                let set = model.begin_edit_group().and_then(|()| {
                    let frozen = model
                        .set_frozen_rows_count(sheet.0, rows)
                        .and_then(|()| model.set_frozen_columns_count(sheet.0, i32::from(cols)));
                    frozen.and(model.end_edit_group())
                });
                if !was_paused {
                    model.resume_evaluation();
                }
                set
            })
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    // A sheet the model does not hold reads as shown, the state of a file that says nothing.
    fn gridlines_shown(&self, sheet: SheetId) -> bool {
        self.model.get_show_grid_lines(sheet.0).unwrap_or(true)
    }

    fn set_gridlines_shown(&mut self, sheet: SheetId, shown: bool) -> Result<(), EngineError> {
        self.model
            .edit_keeping_cells(|model| model.set_show_grid_lines(sheet.0, shown))
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn headings_shown(&self, sheet: SheetId) -> bool {
        self.model.get_show_row_col_headers(sheet.0).unwrap_or(true)
    }

    fn set_headings_shown(&mut self, sheet: SheetId, shown: bool) -> Result<(), EngineError> {
        self.model
            .edit_keeping_cells(|model| model.set_show_row_col_headers(sheet.0, shown))
            .map_err(rejected)
            .inspect(|()| self.model.count_style_edit())
    }

    fn insert_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError> {
        let count = i32::try_from(count).map_err(|e| rejected(e.to_string()))?;
        self.model
            .insert_rows(sheet.0, row_i32(at), count)
            .map_err(rejected)?;
        self.log_lines(
            sheet,
            LineOp {
                axis: LineAxis::Row,
                change: LineChange::Insert,
                at: at.get(),
                count: count.unsigned_abs(),
            },
        );
        self.rows_moved(sheet, at);
        Ok(())
    }

    fn delete_rows(&mut self, sheet: SheetId, at: RowIdx, count: u32) -> Result<(), EngineError> {
        let count = i32::try_from(count).map_err(|e| rejected(e.to_string()))?;
        self.model
            .delete_rows(sheet.0, row_i32(at), count)
            .map_err(rejected)?;
        self.log_lines(
            sheet,
            LineOp {
                axis: LineAxis::Row,
                change: LineChange::Delete,
                at: at.get(),
                count: count.unsigned_abs(),
            },
        );
        self.rows_moved(sheet, at);
        Ok(())
    }

    fn insert_columns(
        &mut self,
        sheet: SheetId,
        at: ColIdx,
        count: u16,
    ) -> Result<(), EngineError> {
        self.model
            .insert_columns(sheet.0, col_i32(at), i32::from(count))
            .map_err(rejected)?;
        self.log_lines(
            sheet,
            LineOp {
                axis: LineAxis::Column,
                change: LineChange::Insert,
                at: u32::from(at.get()),
                count: u32::from(count),
            },
        );
        self.columns_moved(sheet, at);
        Ok(())
    }

    fn delete_columns(
        &mut self,
        sheet: SheetId,
        at: ColIdx,
        count: u16,
    ) -> Result<(), EngineError> {
        self.model
            .delete_columns(sheet.0, col_i32(at), i32::from(count))
            .map_err(rejected)?;
        self.log_lines(
            sheet,
            LineOp {
                axis: LineAxis::Column,
                change: LineChange::Delete,
                at: u32::from(at.get()),
                count: u32::from(count),
            },
        );
        self.columns_moved(sheet, at);
        Ok(())
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
            .map(|(row, col, _)| {
                CellPos::new(
                    RowIdx::clamped(i64::from(row) - 1),
                    ColIdx::clamped(i64::from(col) - 1),
                )
            })
            .collect();
        cells.retain(|pos| !self.input(sheet, *pos).is_empty());
        cells.sort_by_key(|pos| (pos.row, pos.col));
        cells
    }

    fn filled_cells_within(&self, sheet: SheetId, range: Range, limit: usize) -> Vec<CellPos> {
        let Ok(ws) = self.model.get_model().workbook.worksheet(sheet.0) else {
            return Vec::new();
        };
        let mut cells = content_cells_up_to(&ws.sheet_data, range, limit);
        cells.retain(|pos| !self.input(sheet, *pos).is_empty());
        cells
    }

    fn used_end(&self, sheet: SheetId) -> CellPos {
        self.model.used_end(sheet)
    }

    fn defined_names(&self) -> Vec<DefinedName> {
        // The raw list: the display form re-parses every formula against all names.
        self.model
            .get_model()
            .workbook
            .get_defined_names_with_scope()
            .into_iter()
            .map(|(name, scope, refers_to)| DefinedName {
                name,
                scope: scope.map(SheetId),
                refers_to,
            })
            .collect()
    }

    fn add_sheet(&mut self) -> Result<SheetId, EngineError> {
        self.model.new_sheet().map_err(rejected)?;
        let count = self.model.get_worksheets_properties().len();
        Ok(SheetId(u32::try_from(count.saturating_sub(1)).unwrap_or(0)))
    }

    // The copy lands right after the source, with formulas that pointed at the source
    // now pointing at the copy, as in Excel.
    fn duplicate_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError> {
        self.model.duplicate_sheet(sheet.0).map_err(rejected)?;
        self.model.record_unknown();
        Ok(())
    }

    fn rename_sheet(&mut self, sheet: SheetId, name: &str) -> Result<(), EngineError> {
        self.model.rename_sheet(sheet.0, name).map_err(rejected)
    }

    fn delete_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError> {
        self.model.delete_sheet(sheet.0).map_err(rejected)?;
        // A sheet added next may take the identity of this one, so what is logged for it
        // no longer says what it did.
        self.model.record_unknown();
        Ok(())
    }

    fn move_sheet(&mut self, sheet: SheetId, to: u32) -> Result<(), EngineError> {
        self.model.move_sheet(sheet.0, to).map_err(rejected)
    }

    fn to_xlsx(&self) -> Result<Vec<u8>, EngineError> {
        let plan = crate::save::full_plan(self.model.get_model(), &self.carried)?;
        let (xlsx, _) = crate::save::write_full(plan, Cursor::new(Vec::new()))?;
        Ok(xlsx.into_inner())
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
    Dates {
        trend: Trend,
        format: String,
    },
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
            // Written as the serial and given the source's date format afterwards: no typed
            // text reads back as serial 0, which Excel shows as 1/0/1900.
            Series::Dates { trend, .. } => {
                format!("{}", trend.at(step).round().clamp(0.0, MAX_DATE_SERIAL))
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

// Formats IronCalc cannot parse (a "[DBNum1]" prefix, "ss.000") still fill as dates when
// a d or y is left once quoted text, escapes and brackets are dropped.
fn has_day_or_year_letter(code: &str) -> bool {
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => chars.by_ref().take_while(|&q| q != '"').for_each(drop),
            '[' => chars.by_ref().take_while(|&q| q != ']').for_each(drop),
            '\\' | '_' | '*' => {
                chars.next();
            }
            'd' | 'y' => return true,
            _ => {}
        }
    }
    false
}

pub(crate) fn is_date_format(code: &str) -> bool {
    use ironcalc::base::formatter::parser::{ParsePart, Parser, TextToken};
    // The lexer reads only lowercase d, m and y; Excel accepts "YYYY-MM-DD" too. Brackets
    // keep their case because "Color 3" and "[H]" are read as written.
    let mut inside_brackets = false;
    let lowered: String = code
        .chars()
        .map(|c| {
            match c {
                '[' => inside_brackets = true,
                ']' => inside_brackets = false,
                _ => {}
            }
            if inside_brackets {
                c
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    let mut parser = Parser::new(&lowered);
    parser.parse();
    let part = match parser.parts.first() {
        Some(ParsePart::Date(part)) => part,
        Some(ParsePart::Error(_)) => return has_day_or_year_letter(&lowered),
        _ => return false,
    };
    // The parser reads "mm" before any time unit as a month, so a part with a time unit
    // counts as a date only when it also has a day, a year or a month name.
    let mut has_time_unit = false;
    let mut has_month_number = false;
    for token in &part.tokens {
        match token {
            TextToken::Day
            | TextToken::DayPadded
            | TextToken::DayNameShort
            | TextToken::DayName
            | TextToken::MonthNameShort
            | TextToken::MonthName
            | TextToken::MonthLetter
            | TextToken::YearShort
            | TextToken::Year => return true,
            TextToken::Month | TextToken::MonthPadded => has_month_number = true,
            TextToken::Hour
            | TextToken::HourPadded
            | TextToken::Minute
            | TextToken::MinutePadded
            | TextToken::Second
            | TextToken::SecondPadded
            | TextToken::ElapsedHour
            | TextToken::ElapsedHourPadded
            | TextToken::ElapsedMinute
            | TextToken::ElapsedMinutePadded
            | TextToken::ElapsedSecond
            | TextToken::ElapsedSecondPadded
            | TextToken::AMPM => has_time_unit = true,
            _ => {}
        }
    }
    has_month_number && !has_time_unit
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

pub fn format_preview(value: f64, code: &str) -> Result<String, EngineError> {
    check_format_code(code)?;
    let locale = ironcalc::base::locale::get_locale(LOCALE).map_err(rejected)?;
    let formatted = ironcalc::base::formatter::format::format_number(value, code, locale);
    match formatted.error {
        Some(error) => Err(rejected(error)),
        None => Ok(formatted.text),
    }
}

fn engine_model(book: ironcalc::base::types::Workbook) -> Result<Model<'static>, EngineError> {
    Model::from_workbook(book, LANGUAGE).map_err(model_error)
}

fn model_error(error: String) -> EngineError {
    if error.starts_with(ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX) {
        EngineError::TooLarge(error)
    } else {
        EngineError::InvalidFile(error)
    }
}

// The first evaluation can still refuse the file: the text its formulas return is only
// known once evaluated. A refused model is dropped half evaluated, never shown.
fn evaluated(
    model: Model<'static>,
    carried: Carried,
    saved: SavedSlot,
    opened: Vec<Unsupported>,
) -> Result<Workbook, EngineError> {
    let mut model = UserModel::from_model(model);
    tracing::info_span!("open_evaluate")
        .in_scope(|| model.evaluate_opened())
        .map_err(model_error)?;
    Ok(warmed(model, carried, saved, opened))
}

// A woken workbook held these results in memory when it was hibernated, so evaluating
// it again is not charged to the open budget, and waking never refuses it for that.
pub(crate) fn evaluated_user_model(
    mut model: UserModel<'static>,
    carried: Carried,
    saved: SavedSlot,
    opened: Vec<Unsupported>,
) -> Workbook {
    tracing::info_span!("open_evaluate").in_scope(|| model.evaluate());
    warmed(model, carried, saved, opened)
}

fn warmed(
    model: UserModel<'static>,
    carried: Carried,
    saved: SavedSlot,
    opened: Vec<Unsupported>,
) -> Workbook {
    let workbook = Workbook {
        model: CachedModel::new(model),
        carried,
        saved,
        opened,
    };
    tracing::info_span!("open_warm_used_areas").in_scope(|| workbook.warm_used_areas());
    workbook
}

fn carried_by_position(model: &Model<'_>, settings: Vec<SheetSettings>) -> Carried {
    let ids: Vec<u32> = model
        .workbook
        .worksheets
        .iter()
        .map(|sheet| sheet.sheet_id)
        .collect();
    if ids.len() != settings.len() {
        tracing::error!(
            sheets = ids.len(),
            read = settings.len(),
            "sheet settings do not match the loaded sheets; they are not kept"
        );
        return Carried::default();
    }
    Carried::new(ids.into_iter().zip(settings).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironcalc::base::open_budget::BUDGET_REFUSAL_PREFIX;
    use ironcalc::error::XlsxError;

    #[test]
    fn a_row_height_a_file_makes_up_never_reaches_the_pixels() {
        assert_eq!(row_pixels(f64::NAN), 0.0);
        assert_eq!(row_pixels(f64::NEG_INFINITY), 0.0);
        assert_eq!(row_pixels(-3.0), 0.0);
        assert_eq!(row_pixels(1.0e-300), 0.0);
        assert_eq!(row_pixels(1.0e308), 546.0);
        assert_eq!(row_pixels(409.5), 546.0);
        assert_eq!(row_pixels(15.0), 20.0);
    }

    fn counts(book: &Workbook) -> (u64, u64, u64) {
        let edits = book.format_edits();
        (edits.numbers, edits.other, edits.moved)
    }

    #[test]
    fn successful_format_commands_are_counted_and_typing_is_not() {
        let mut book = Workbook::new_empty().unwrap();
        let a1 = Range::single(CellPos::default());
        book.set_input(SheetId(0), CellPos::default(), "0.5")
            .unwrap();
        assert_eq!(counts(&book), (0, 0, 0));
        book.set_number_format(SheetId(0), a1, "0.00%").unwrap();
        book.apply_style(SheetId(0), a1, StyleChange::Bold(true))
            .unwrap();
        book.apply_style(
            SheetId(0),
            a1,
            StyleChange::NumberFormat(zenkai_types::NumberFormat::Number),
        )
        .unwrap();
        let counted = book.format_edits();
        assert_eq!((counted.numbers, counted.other), (2, 1));
        assert!(
            book.set_number_format(SheetId(0), a1, &"x".repeat(300))
                .is_err()
        );
        assert_eq!(book.format_edits(), counted);
    }

    #[test]
    fn only_commands_that_can_change_a_style_or_move_lines_are_counted_after_they_succeed() {
        let sheet = SheetId(0);
        let a1 = CellPos::default();
        let b2 = CellPos::new(RowIdx::clamped(1), ColIdx::clamped(1));
        let c3 = CellPos::new(RowIdx::clamped(2), ColIdx::clamped(2));
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(sheet, a1, "0.5").unwrap();
        book.set_inputs(sheet, a1, &[vec!["1".to_string(), "2".to_string()]])
            .unwrap();
        book.set_scattered_inputs(sheet, &[(a1, "3".to_string())])
            .unwrap();
        book.clear(sheet, Range::single(a1)).unwrap();
        book.undo().unwrap();
        book.redo().unwrap();
        let rows = Range::new(a1, CellPos::new(RowIdx::clamped(1), ColIdx::clamped(1)));
        book.fill_with(sheet, a1, "x", rows).unwrap();
        let copied = book.copy(sheet, Range::single(a1)).unwrap();
        book.paste(sheet, b2, &copied, false).unwrap();
        book.fill(sheet, rows, true).unwrap();
        book.extend(sheet, Range::single(a1), rows).unwrap();
        book.rename_sheet(sheet, "Renamed").unwrap();
        book.add_sheet().unwrap();
        book.move_sheet(sheet, 1).unwrap();
        book.duplicate_sheet(sheet).unwrap();
        assert_eq!(counts(&book), (0, 0, 1));
        let sheet = book.sheets()[0].id;

        book.insert_rows(sheet, RowIdx::clamped(1), 1).unwrap();
        book.delete_rows(sheet, RowIdx::clamped(1), 1).unwrap();
        book.insert_columns(sheet, ColIdx::clamped(1), 1).unwrap();
        book.delete_columns(sheet, ColIdx::clamped(1), 1).unwrap();
        assert_eq!(counts(&book), (0, 0, 1));

        book.set_column_width(sheet, ColIdx::clamped(0), 120.0)
            .unwrap();
        book.set_row_height(sheet, RowIdx::clamped(0), 40.0)
            .unwrap();
        book.set_rows_hidden(sheet, Range::single(c3), true)
            .unwrap();
        book.set_frozen(sheet, 1, 0).unwrap();
        let counted = book.format_edits();
        assert_eq!((counted.numbers, counted.other, counted.moved), (0, 4, 1));

        book.apply_style(sheet, Range::single(b2), StyleChange::Bold(true))
            .unwrap();
        let styled = book.copy(sheet, Range::single(b2)).unwrap();
        let before = book.format_edits();
        book.paste(sheet, c3, &styled, false).unwrap();
        let after = book.format_edits();
        assert_eq!(
            (after.numbers, after.other),
            (before.numbers + 1, before.other + 1)
        );

        let before = book.format_edits();
        book.without_counting(|book| book.set_column_width(sheet, ColIdx::clamped(1), 90.0))
            .unwrap();
        assert_eq!(book.format_edits(), before);
    }

    fn content(row: u32, col: u32, rows: u32, cols: u32) -> SheetChange {
        SheetChange::Content(ContentSpan {
            row,
            col,
            rows,
            cols,
        })
    }

    #[test]
    fn every_command_that_writes_content_is_logged_with_its_range_and_in_order() {
        let sheet = SheetId(0);
        let a1 = CellPos::default();
        let b2 = CellPos::new(RowIdx::clamped(1), ColIdx::clamped(1));
        let mut book = Workbook::new_empty().unwrap();
        assert_eq!(book.changes_since(0), Some(Vec::new()));
        book.set_input(sheet, b2, "x").unwrap();
        book.set_inputs(
            sheet,
            a1,
            &[
                vec!["1".to_string(), "2".to_string()],
                vec!["3".to_string()],
            ],
        )
        .unwrap();
        book.insert_rows(sheet, RowIdx::clamped(1), 2).unwrap();
        book.clear(sheet, Range::new(a1, b2)).unwrap();
        let key = book.sheet_key(sheet).unwrap();
        let logged = |change| zenkai_types::LoggedChange { sheet: key, change };
        let lines = SheetChange::Lines(LineOp {
            axis: LineAxis::Row,
            change: LineChange::Insert,
            at: 1,
            count: 2,
        });
        assert_eq!(
            book.changes_since(0),
            Some(vec![
                logged(content(1, 1, 1, 1)),
                logged(content(0, 0, 2, 2)),
                logged(lines),
                logged(content(0, 0, 2, 2)),
            ])
        );
        assert_eq!(book.changes_since(3).unwrap().len(), 1);
        assert_eq!(book.format_edits().seq, 4);
        assert_eq!(counts(&book), (0, 0, 0));
    }

    #[test]
    fn a_log_that_outgrew_its_bound_tells_a_reader_that_fell_behind() {
        let sheet = SheetId(0);
        let mut book = Workbook::new_empty().unwrap();
        for _ in 0..1100 {
            book.insert_rows(sheet, RowIdx::clamped(0), 1).unwrap();
        }
        assert_eq!(book.changes_since(0), None);
        assert_eq!(book.changes_since(1099).unwrap().len(), 1);
        assert_eq!(book.changes_since(1100), Some(Vec::new()));
        assert_eq!(book.changes_since(1101), None);
        book.forget_changes();
        assert_eq!(book.changes_since(0), Some(Vec::new()));
    }

    #[test]
    fn what_a_sheet_does_is_logged_by_its_identity_not_by_its_place() {
        let mut book = Workbook::new_empty().unwrap();
        let first = SheetId(0);
        let key = book.sheet_key(first).unwrap();
        book.add_sheet().unwrap();
        book.move_sheet(first, 1).unwrap();
        let moved = SheetId(1);
        assert_eq!(book.sheet_key(moved), Some(key));
        book.set_input(moved, CellPos::default(), "1").unwrap();
        let logged = book.changes_since(0).unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].sheet, key);
    }

    #[test]
    fn an_undo_logs_the_content_it_changes_and_a_deleted_sheet_makes_the_log_unknown() {
        let sheet = SheetId(0);
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(sheet, CellPos::default(), "1").unwrap();
        book.apply_style(
            sheet,
            Range::single(CellPos::default()),
            StyleChange::Bold(true),
        )
        .unwrap();
        book.undo().unwrap();
        assert_eq!(book.changes_since(1), Some(Vec::new()));
        book.undo().unwrap();
        let logged = book.changes_since(1).unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].change, content(0, 0, 1, 1));

        book.add_sheet().unwrap();
        book.delete_sheet(SheetId(1)).unwrap();
        let logged = book.changes_since(2).unwrap();
        assert_eq!(
            logged.last().map(|change| change.change),
            Some(SheetChange::Unknown)
        );
        assert_eq!(book.format_edits().moved, 1);
    }

    #[test]
    fn what_building_a_workbook_did_is_forgotten_and_only_what_follows_counts() {
        let sheet = SheetId(0);
        let a1 = CellPos::default();
        let mut book = Workbook::new_empty().unwrap();
        book.set_inputs(
            sheet,
            a1,
            &[vec!["12.345%".to_string(), "2026-10-25".to_string()]],
        )
        .unwrap();
        book.apply_style(sheet, Range::single(a1), StyleChange::Bold(true))
            .unwrap();
        book.forget_changes();
        assert_eq!(book.format_edits(), crate::model::FormatEdits::default());
        assert_eq!(book.changes_since(0), Some(Vec::new()));
        book.set_column_width(sheet, ColIdx::clamped(0), 90.0)
            .unwrap();
        assert_eq!(counts(&book), (0, 1, 0));
    }

    #[test]
    fn a_copy_of_a_sheet_makes_the_log_unknown_as_a_deleted_one_does() {
        let mut book = Workbook::new_empty().unwrap();
        book.duplicate_sheet(SheetId(0)).unwrap();
        let logged = book.changes_since(0).unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].change, SheetChange::Unknown);
        assert_eq!(book.format_edits().moved, 1);
    }

    #[test]
    fn cells_rearranged_without_a_line_operation_are_counted_as_moved() {
        let sheet = SheetId(0);
        let a1 = CellPos::default();
        let c1 = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(2));
        let mut book = Workbook::new_empty().unwrap();
        let column: Vec<Vec<String>> = ["3", "1", "2"]
            .iter()
            .map(|n| vec![n.to_string()])
            .collect();
        book.set_inputs(sheet, a1, &column).unwrap();
        let rows = Range::new(a1, CellPos::new(RowIdx::clamped(2), ColIdx::clamped(0)));
        book.sort(sheet, rows, ColIdx::clamped(0), false).unwrap();
        assert_eq!(book.format_edits().moved, 1);

        let copied = book.copy(sheet, Range::single(a1)).unwrap();
        book.paste(sheet, c1, &copied, false).unwrap();
        assert_eq!(book.format_edits().moved, 1);
        book.paste(sheet, c1, &copied, true).unwrap();
        assert_eq!(book.format_edits().moved, 2);
    }

    #[test]
    fn a_cut_paste_makes_the_log_unknown_as_it_rewrites_cells_it_does_not_name() {
        let sheet = SheetId(0);
        let a1 = CellPos::default();
        let c1 = CellPos::new(RowIdx::clamped(0), ColIdx::clamped(2));
        let mut book = Workbook::new_empty().unwrap();
        book.set_inputs(sheet, a1, &[vec!["1".to_string()], vec!["=A1".to_string()]])
            .unwrap();
        book.forget_changes();
        let copied = book.copy(sheet, Range::single(a1)).unwrap();
        book.paste(sheet, c1, &copied, false).unwrap();
        let after_copy = book.changes_since(0).unwrap();
        assert!(!after_copy.iter().any(|c| c.change == SheetChange::Unknown));
        book.paste(sheet, c1, &copied, true).unwrap();
        let after_cut = book.changes_since(0).unwrap();
        assert!(after_cut.iter().any(|c| c.change == SheetChange::Unknown));
    }

    #[test]
    fn date_formats_ignore_quoted_escaped_and_bracketed_letters() {
        for code in [
            "m/d/yyyy",
            "[$-409]d-mmm-yy",
            "YYYY-MM-DD",
            "dddd, mmmm d",
            "mm",
            "d/m/yyyy h:mm",
            "yyyy-mm-dd hh:mm:ss.000",
            "[DBNum1]d/m/yyyy",
        ] {
            assert!(is_date_format(code), "{code}");
        }
        for code in [
            "General",
            "0",
            "0\" days\"",
            "0;[Red]0",
            r"\d0",
            "[Red]0.00",
            "[>100]0",
            "h:mm:ss",
            "mm:ss",
            "h:mm AM/PM",
            "[mm]:ss",
            "[h]:mm",
        ] {
            assert!(!is_date_format(code), "{code}");
        }
    }

    #[test]
    fn a_budget_refusal_reaches_the_user_as_plain_text() {
        let message = format!("{BUDGET_REFUSAL_PREFIX}12.3 GB to open");
        let error = read_error(XlsxError::Workbook(message.clone()));
        assert!(matches!(&error, EngineError::TooLarge(text) if *text == message));
    }

    // A column of formulas returning a thousand characters each, without cached values.
    fn long_text_book() -> ironcalc::base::types::Workbook {
        use ironcalc::base::types::{Cell, FormulaValue};
        let mut model = Model::new_empty("budget", LOCALE, "UTC", LANGUAGE).unwrap();
        for row in 1..=100 {
            model
                .set_user_input(0, row, 1, format!("=REPT(\"x\",1000)&{row}"))
                .unwrap();
        }
        let mut book = model.workbook;
        for row in 1..=100 {
            if let Some(Cell::CellFormula { v, .. }) = book.worksheets[0].sheet_data.get_mut(row, 1)
            {
                *v = FormulaValue::Unevaluated;
            }
        }
        book
    }

    fn opened_within(budget: u64) -> Result<Workbook, EngineError> {
        let book = long_text_book();
        let model = Model::from_workbook_within(book, LANGUAGE, budget).map_err(model_error)?;
        evaluated(model, Carried::default(), SavedSlot::default(), Vec::new())
    }

    #[test]
    fn result_text_past_the_open_budget_refuses_the_file_as_too_large() {
        use ironcalc::base::open_budget::{needed_bytes, open_cost};
        let budget = needed_bytes(open_cost(&long_text_book()));
        let error = opened_within(budget).map(|_| ()).unwrap_err();
        assert!(
            matches!(&error, EngineError::TooLarge(text) if text.starts_with(BUDGET_REFUSAL_PREFIX)),
            "{error}"
        );
    }

    #[test]
    fn result_text_within_the_open_budget_opens_evaluated() {
        let book = opened_within(u64::MAX).unwrap();
        let pos = CellPos::parse_a1("A42").unwrap();
        assert_eq!(
            book.cell(SheetId(0), pos).text,
            format!("{}42", "x".repeat(1000))
        );
    }

    #[test]
    fn any_other_import_error_stays_an_invalid_file() {
        let error = read_error(XlsxError::Workbook("bad sheet".to_string()));
        assert!(matches!(error, EngineError::InvalidFile(_)));
    }

    #[test]
    fn hundreds_of_defined_names_are_all_listed_with_their_target() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = book.sheets()[0].name.clone();
        for index in 0..300 {
            let target = format!("{sheet}!$A${}", index + 1);
            book.model
                .new_defined_name(&format!("Name{index}"), None, &target)
                .unwrap();
        }
        let names = book.defined_names();
        assert_eq!(names.len(), 300);
        for (index, listed) in names.iter().enumerate() {
            assert_eq!(listed.name, format!("Name{index}"));
            assert_eq!(listed.scope, None);
            assert!(listed.refers_to.ends_with(&format!("$A${}", index + 1)));
        }
    }

    #[test]
    fn defined_names_list_their_scope_and_target() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = book.sheets()[0].name.clone();
        book.model
            .new_defined_name("Total", None, &format!("{sheet}!$B$2:$B$9"))
            .unwrap();
        book.model
            .new_defined_name("Local", Some(0), &format!("{sheet}!$A$1"))
            .unwrap();
        let names = book.defined_names();
        let find = |name: &str| names.iter().find(|n| n.name == name).unwrap();
        assert_eq!(find("Total").scope, None);
        assert_eq!(find("Local").scope, Some(SheetId(0)));
        assert!(find("Total").refers_to.ends_with("$B$2:$B$9"));
    }
}
