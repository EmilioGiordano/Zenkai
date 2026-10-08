use std::io::Cursor;

use ironcalc::base::UserModel;
use ironcalc::base::expressions::types::Area;
use ironcalc::base::types::{Color, HorizontalAlignment, Style};
use ironcalc::export::save_xlsx_to_writer;
use ironcalc::import::load_from_xlsx_bytes;

use crate::error::EngineError;
use zenkai_types::{
    CellPos, CellStyle, CellView, ColIdx, ColumnSpan, HAlign, Range, Rgb, RowIdx, SheetId,
    SheetInfo, SheetSizes, StyleChange, ValueKind,
};

const LOCALE: &str = "en";
const TIMEZONE: &str = "UTC";
const LANGUAGE: &str = "en";
// Stored widths are in Excel characters and heights in points; these give Excel's pixels.
const PIXELS_PER_CHAR: f64 = 7.0;
const CHAR_WIDTH_PADDING: f64 = 5.0;
const PIXELS_PER_POINT: f64 = 4.0 / 3.0;

pub trait Engine: Send {
    fn sheets(&self) -> Vec<SheetInfo>;
    fn cell(&self, sheet: SheetId, pos: CellPos) -> CellView;
    fn input(&self, sheet: SheetId, pos: CellPos) -> String;
    fn set_input(&mut self, sheet: SheetId, pos: CellPos, text: &str) -> Result<(), EngineError>;
    fn clear(&mut self, sheet: SheetId, range: Range) -> Result<(), EngineError>;
    fn apply_style(
        &mut self,
        sheet: SheetId,
        range: Range,
        change: StyleChange,
    ) -> Result<(), EngineError>;
    fn undo(&mut self) -> Result<(), EngineError>;
    fn redo(&mut self) -> Result<(), EngineError>;
    fn sizes(&self, sheet: SheetId) -> SheetSizes;
    fn frozen(&self, sheet: SheetId) -> (u32, u16);
    fn used_end(&self, sheet: SheetId) -> CellPos;
    fn add_sheet(&mut self) -> Result<SheetId, EngineError>;
    fn rename_sheet(&mut self, sheet: SheetId, name: &str) -> Result<(), EngineError>;
    fn delete_sheet(&mut self, sheet: SheetId) -> Result<(), EngineError>;
    fn move_sheet(&mut self, sheet: SheetId, to: u32) -> Result<(), EngineError>;
    fn to_xlsx(&self) -> Result<Vec<u8>, EngineError>;
}

pub struct Workbook {
    model: UserModel<'static>,
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
        let model = UserModel::new_empty("Book1", LOCALE, TIMEZONE, LANGUAGE).map_err(rejected)?;
        Ok(Workbook { model })
    }

    pub fn from_xlsx_bytes(bytes: &[u8], name: &str) -> Result<Workbook, EngineError> {
        let book = load_from_xlsx_bytes(bytes, name, LOCALE, TIMEZONE)
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
        let text = self
            .model
            .get_formatted_cell_value(sheet.0, row, col)
            .unwrap_or_default();
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

    fn input(&self, sheet: SheetId, pos: CellPos) -> String {
        self.model
            .get_cell_content(sheet.0, row_i32(pos.row), col_i32(pos.col))
            .unwrap_or_default()
    }

    fn set_input(&mut self, sheet: SheetId, pos: CellPos, text: &str) -> Result<(), EngineError> {
        self.model
            .set_user_input(sheet.0, row_i32(pos.row), col_i32(pos.col), text)
            .map_err(rejected)
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

    fn frozen(&self, sheet: SheetId) -> (u32, u16) {
        let rows = self.model.get_frozen_rows_count(sheet.0).unwrap_or(0);
        let cols = self.model.get_frozen_columns_count(sheet.0).unwrap_or(0);
        (
            u32::try_from(rows).unwrap_or(0),
            u16::try_from(cols).unwrap_or(0),
        )
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
        save_xlsx_to_writer(self.model.get_model(), Cursor::new(Vec::new()))
            .map(Cursor::into_inner)
            .map_err(|e| rejected(format!("{e:?}")))
    }
}
