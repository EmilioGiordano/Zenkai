use std::io::Cursor;

use ironcalc::base::expressions::types::Area;
use ironcalc::base::types::{Color, HorizontalAlignment, Style};
use ironcalc::base::{ClipboardData, UserModel};
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::load_from_xlsx_bytes;

use crate::error::EngineError;
use zenkai_types::{
    CellPos, CellStyle, CellView, ColIdx, ColumnSpan, HAlign, Range, Rgb, RowIdx, SheetId,
    SheetInfo, SheetSizes, StyleChange, ValueKind,
};

const LOCALE: &str = "en";
const MAX_FILL_CELLS: u64 = 1_000_000;
const LANGUAGE: &str = "en";
// Stored widths are in Excel characters and heights in points; these give Excel's pixels.
const PIXELS_PER_CHAR: f64 = 7.0;
const CHAR_WIDTH_PADDING: f64 = 5.0;
const PIXELS_PER_POINT: f64 = 4.0 / 3.0;
// IronCalc's setter takes its own pixels: characters times this factor.
const COLUMN_WIDTH_FACTOR: f64 = 9.0;
// And its row height setter takes points times this factor.
const ROW_HEIGHT_FACTOR: f64 = 1.5625;

pub trait Engine: Send {
    fn sheets(&self) -> Vec<SheetInfo>;
    fn cell(&self, sheet: SheetId, pos: CellPos) -> CellView;
    fn input(&self, sheet: SheetId, pos: CellPos) -> String;
    fn number(&self, sheet: SheetId, pos: CellPos) -> Option<f64>;
    fn set_input(&mut self, sheet: SheetId, pos: CellPos, text: &str) -> Result<(), EngineError>;
    fn set_inputs(
        &mut self,
        sheet: SheetId,
        origin: CellPos,
        rows: &[Vec<String>],
    ) -> Result<(), EngineError>;
    fn clear(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError>;
    fn fill(&mut self, sheet: SheetId, target: Range, down: bool) -> Result<(), EngineError>;
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
    fn rename_sheet(&mut self, sheet: SheetId, name: &str) -> Result<(), EngineError>;
    fn delete_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError>;
    fn move_sheet(&mut self, sheet: SheetId, to: u32) -> Result<(), EngineError>;
    fn to_xlsx(&self) -> Result<Vec<u8>, EngineError>;
}

pub struct Workbook {
    model: UserModel<'static>,
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
        crate::preflight::check_formula_text(text).map_err(EngineError::Rejected)?;
    }
    Ok(())
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
    pub fn new_empty() -> Result<Workbook, EngineError> {
        let model =
            UserModel::new_empty("Book1", LOCALE, timezone(), LANGUAGE).map_err(rejected)?;
        Ok(Workbook { model })
    }

    pub fn from_xlsx_bytes(bytes: &[u8], name: &str) -> Result<Workbook, EngineError> {
        let book = load_from_xlsx_bytes(bytes, name, LOCALE, timezone())
            .map_err(|e| EngineError::InvalidFile(format!("{e:?}")))?;
        let model = ironcalc::base::Model::from_workbook(book, LANGUAGE)
            .map_err(EngineError::InvalidFile)?;
        let mut model = UserModel::from_model(model);
        model.evaluate();
        Ok(Workbook { model })
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
        CellStyle {
            bold: style.font.b,
            italic: style.font.i,
            underline: style.font.u,
            strike: style.font.strike,
            font_size: (style.font.sz > 0).then_some(style.font.sz as f32),
            font_color: self.resolve(&style.font.color),
            fill: self.resolve(&style.fill.color),
            align,
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

    fn number(&self, sheet: SheetId, pos: CellPos) -> Option<f64> {
        match self.model.get_model().get_cell_value_by_index(
            sheet.0,
            row_i32(pos.row),
            col_i32(pos.col),
        ) {
            Ok(ironcalc::base::cell::CellValue::Number(n)) => Some(n),
            _ => None,
        }
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

    fn copy(&mut self, sheet: SheetId, range: Range) -> Result<Copied, EngineError> {
        select(&mut self.model, sheet, range)?;
        let clipboard = self.model.copy_to_clipboard().map_err(rejected)?;
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

    fn clear(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError> {
        self.model
            .range_clear_contents(&area(sheet, range))
            .map_err(rejected)
    }

    fn apply_style(
        &mut self,
        sheet: SheetId,
        range: Range,
        change: StyleChange,
    ) -> Result<(), EngineError> {
        let flag = |on: bool| if on { "true" } else { "false" };
        let (path, value) = match change {
            StyleChange::Bold(on) => ("font.b", flag(on)),
            StyleChange::Italic(on) => ("font.i", flag(on)),
            StyleChange::Underline(on) => ("font.u", flag(on)),
            StyleChange::Align(align) => (
                "alignment.horizontal",
                match align {
                    HAlign::General => "general",
                    HAlign::Left => "left",
                    HAlign::Center => "center",
                    HAlign::Right => "right",
                },
            ),
            StyleChange::NumberFormat(format) => ("num_fmt", format.code()),
        };
        self.model
            .update_range_style(&area(sheet, range), path, value)
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
        let chars = ((f64::from(pixels) - CHAR_WIDTH_PADDING) / PIXELS_PER_CHAR).max(0.0);
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
                f64::from(pixels) / PIXELS_PER_POINT * ROW_HEIGHT_FACTOR,
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
        let dimension = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map(|ws| ws.dimension());
        match dimension {
            Ok(d) => CellPos::new(
                RowIdx::clamped(i64::from(d.max_row) - 1),
                ColIdx::clamped(i64::from(d.max_column) - 1),
            ),
            Err(_) => CellPos::default(),
        }
    }

    fn add_sheet(&mut self) -> Result<SheetId, EngineError> {
        self.model.new_sheet().map_err(rejected)?;
        let count = self.model.get_worksheets_properties().len();
        Ok(SheetId(u32::try_from(count.saturating_sub(1)).unwrap_or(0)))
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
