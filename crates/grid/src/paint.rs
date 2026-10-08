use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_types::{CellPos, ColIdx, HAlign, Range, Rgb, RowIdx, ValueKind};

use crate::grid::{EditMode, Editor, GridCell, HEADER_HEIGHT, ROW_HEADER_WIDTH};
use crate::layout::Layout;

const CELL_PADDING: f32 = 4.0;
const BASE_FONT_SIZE: f32 = 13.0;
const DEFAULT_POINTS: f32 = 11.0;

#[derive(Clone, Copy)]
pub struct Colors {
    background: Hsla,
    foreground: Hsla,
    gridline: Hsla,
    header: Hsla,
    header_text: Hsla,
    header_active: Hsla,
    header_active_text: Hsla,
    accent: Hsla,
    selection: Hsla,
    error: Hsla,
}

impl Colors {
    pub fn from_theme(cx: &App) -> Colors {
        let theme = cx.theme();
        Colors {
            background: theme.background,
            foreground: theme.foreground,
            gridline: theme.border.opacity(0.6),
            header: theme.table_head,
            header_text: theme.muted_foreground,
            header_active: theme.accent,
            header_active_text: theme.accent_foreground,
            accent: theme.primary,
            selection: theme.primary.opacity(0.12),
            error: theme.danger,
        }
    }
}

pub struct Frame {
    pub layout: Rc<Layout>,
    pub cells: Rc<HashMap<CellPos, GridCell>>,
    pub top: RowIdx,
    pub left: ColIdx,
    pub rows: u32,
    pub cols: u16,
    pub selection: Range,
    pub active: CellPos,
    pub editor: Option<Editor>,
    pub marquee: Option<Range>,
    pub zoom: f32,
    pub focused: bool,
    pub colors: Colors,
    pub font: SharedString,
}

struct Columns {
    xs: Vec<(ColIdx, f32, f32)>,
}

impl Columns {
    fn find(&self, col: ColIdx) -> Option<(f32, f32)> {
        self.xs
            .iter()
            .find(|(c, _, _)| *c == col)
            .map(|(_, x, w)| (*x, *w))
    }
}

struct Rows {
    ys: Vec<(RowIdx, f32, f32)>,
}

impl Rows {
    fn find(&self, row: RowIdx) -> Option<(f32, f32)> {
        self.ys
            .iter()
            .find(|(r, _, _)| *r == row)
            .map(|(_, y, h)| (*y, *h))
    }
}

fn rgb_to_hsla(color: Rgb) -> Hsla {
    rgb(color.0).into()
}

fn rect(origin: Point<Pixels>, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(origin.x + px(x), origin.y + px(y)),
        size(px(w.max(0.0)), px(h.max(0.0))),
    )
}

pub fn paint(frame: &Frame, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let z = frame.zoom;
    let c = frame.colors;
    let origin = bounds.origin;
    window.paint_quad(fill(bounds, c.background));

    let mut x = ROW_HEADER_WIDTH * z;
    let mut xs = Vec::with_capacity(usize::from(frame.cols) + 1);
    let mut col = frame.left;
    while x < f32::from(bounds.size.width) {
        let w = frame.layout.col_width(col) * z;
        xs.push((col, x, w));
        x += w;
        if col == ColIdx::LAST {
            break;
        }
        col = col.offset(1);
    }
    let columns = Columns { xs };

    let mut y = HEADER_HEIGHT * z;
    let mut ys = Vec::with_capacity(frame.rows as usize + 1);
    let mut row = frame.top;
    while y < f32::from(bounds.size.height) {
        let h = frame.layout.row_height(row) * z;
        ys.push((row, y, h));
        y += h;
        if row == RowIdx::LAST {
            break;
        }
        row = row.offset(1);
    }
    let rows = Rows { ys };
    let right = x;
    let bottom = y;

    let font_size = BASE_FONT_SIZE * z;
    let line_height = px(font_size * 1.3);
    let editing = frame.editor.as_ref().map(|e| e.pos);

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (row, y, h) in &rows.ys {
            for (col, x, w) in &columns.xs {
                let pos = CellPos::new(*row, *col);
                let Some(cell) = frame.cells.get(&pos) else {
                    continue;
                };
                if let Some(fill_color) = cell.style.fill {
                    window.paint_quad(fill(rect(origin, *x, *y, *w, *h), rgb_to_hsla(fill_color)));
                }
            }
        }

        for (_, x, w) in &columns.xs {
            window.paint_quad(fill(
                rect(origin, x + w - 1.0, HEADER_HEIGHT * z, 1.0, bottom),
                c.gridline,
            ));
        }
        for (_, y, h) in &rows.ys {
            window.paint_quad(fill(
                rect(origin, ROW_HEADER_WIDTH * z, y + h - 1.0, right, 1.0),
                c.gridline,
            ));
        }

        for (row, y, h) in &rows.ys {
            for (col, x, w) in &columns.xs {
                let pos = CellPos::new(*row, *col);
                if Some(pos) == editing {
                    continue;
                }
                let Some(cell) = frame.cells.get(&pos) else {
                    continue;
                };
                if cell.style.border_bottom {
                    window.paint_quad(fill(rect(origin, *x, y + h - 1.0, *w, 1.0), c.foreground));
                }
                if cell.style.border_right {
                    window.paint_quad(fill(rect(origin, x + w - 1.0, *y, 1.0, *h), c.foreground));
                }
                if cell.text.is_empty() {
                    continue;
                }
                paint_cell_text(frame, cell, rect(origin, *x, *y, *w, *h), window, cx);
            }
        }

        paint_selection(frame, &columns, &rows, origin, bounds, window);
        paint_headers(
            frame,
            &columns,
            &rows,
            origin,
            bounds,
            line_height,
            window,
            cx,
        );
        if let Some(editor) = &frame.editor {
            paint_editor(frame, editor, &columns, &rows, origin, window, cx);
        }
    });
}

fn text_run(frame: &Frame, cell: &GridCell, len: usize, color: Hsla) -> TextRun {
    let mut font = font(frame.font.clone());
    if cell.style.bold {
        font.weight = FontWeight::BOLD;
    }
    if cell.style.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: cell.style.underline.then(|| UnderlineStyle {
            thickness: px(1.0),
            color: Some(color),
            wavy: false,
        }),
        strikethrough: cell.style.strike.then(|| StrikethroughStyle {
            thickness: px(1.0),
            color: Some(color),
        }),
    }
}

fn paint_cell_text(
    frame: &Frame,
    cell: &GridCell,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let c = frame.colors;
    let color = match (cell.kind, cell.style.font_color) {
        (ValueKind::Error, _) => c.error,
        (_, Some(rgb_color)) => rgb_to_hsla(rgb_color),
        (_, None) => c.foreground,
    };
    let points = cell.style.font_size.unwrap_or(DEFAULT_POINTS);
    let font_size = px(BASE_FONT_SIZE * points / DEFAULT_POINTS * frame.zoom);
    let line_height = font_size * 1.3;
    let run = text_run(frame, cell, cell.text.len(), color);
    let line = window
        .text_system()
        .shape_line(cell.text.clone(), font_size, &[run], None);
    let padding = px(CELL_PADDING * frame.zoom);
    let width = line.width;
    let align = match cell.style.align {
        HAlign::Left => HAlign::Left,
        HAlign::Center => HAlign::Center,
        HAlign::Right => HAlign::Right,
        HAlign::General => match cell.kind {
            ValueKind::Number => HAlign::Right,
            ValueKind::Bool | ValueKind::Error => HAlign::Center,
            ValueKind::Text | ValueKind::Empty => HAlign::Left,
        },
    };
    let x = match align {
        HAlign::Right => bounds.origin.x + bounds.size.width - padding - width,
        HAlign::Center => bounds.origin.x + (bounds.size.width - width) / 2.0,
        HAlign::Left | HAlign::General => bounds.origin.x + padding,
    };
    let y = bounds.origin.y + bounds.size.height - line_height - px(2.0 * frame.zoom);
    let clip = Bounds::new(
        bounds.origin,
        size(bounds.size.width - px(1.0), bounds.size.height - px(1.0)),
    );
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        if let Err(error) = line.paint(point(x, y), line_height, TextAlign::Left, None, window, cx)
        {
            tracing::warn!(%error, "failed to paint cell text");
        }
    });
}

fn range_rect(
    range: Range,
    columns: &Columns,
    rows: &Rows,
    origin: Point<Pixels>,
    z: f32,
) -> Option<Bounds<Pixels>> {
    let first_col = columns.xs.first()?;
    let last_col = columns.xs.last()?;
    let first_row = rows.ys.first()?;
    let last_row = rows.ys.last()?;
    if range.end.col < first_col.0 || range.start.col > last_col.0 {
        return None;
    }
    if range.end.row < first_row.0 || range.start.row > last_row.0 {
        return None;
    }
    let x0 = columns
        .find(range.start.col)
        .map_or(ROW_HEADER_WIDTH * z - 2.0, |(x, _)| x);
    let x1 = columns
        .find(range.end.col)
        .map_or(last_col.1 + last_col.2 + 2.0, |(x, w)| x + w);
    let y0 = rows
        .find(range.start.row)
        .map_or(HEADER_HEIGHT * z - 2.0, |(y, _)| y);
    let y1 = rows
        .find(range.end.row)
        .map_or(last_row.1 + last_row.2 + 2.0, |(y, h)| y + h);
    Some(rect(origin, x0, y0, x1 - x0, y1 - y0))
}

fn paint_selection(
    frame: &Frame,
    columns: &Columns,
    rows: &Rows,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let c = frame.colors;
    let z = frame.zoom;
    let body = Bounds::new(
        point(
            origin.x + px(ROW_HEADER_WIDTH * z),
            origin.y + px(HEADER_HEIGHT * z),
        ),
        size(
            bounds.size.width - px(ROW_HEADER_WIDTH * z),
            bounds.size.height - px(HEADER_HEIGHT * z),
        ),
    );
    window.with_content_mask(Some(ContentMask { bounds: body }), |window| {
        if let Some(area) = range_rect(frame.selection, columns, rows, origin, z) {
            if frame.selection.cell_count() > 1 {
                window.paint_quad(fill(area, c.selection));
            }
            let border = if frame.focused { 2.0 } else { 1.0 };
            window.paint_quad(quad(
                area,
                px(0.0),
                transparent_black(),
                px(border),
                c.accent,
                BorderStyle::Solid,
            ));
        }
        if let Some(active) = range_rect(Range::single(frame.active), columns, rows, origin, z) {
            window.paint_quad(quad(
                active,
                px(0.0),
                transparent_black(),
                px(2.0),
                c.accent,
                BorderStyle::Solid,
            ));
        }
        if let Some(marquee) = frame.marquee
            && let Some(area) = range_rect(marquee, columns, rows, origin, z)
        {
            window.paint_quad(quad(
                area,
                px(0.0),
                transparent_black(),
                px(2.0),
                c.accent,
                BorderStyle::Dashed,
            ));
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn paint_headers(
    frame: &Frame,
    columns: &Columns,
    rows: &Rows,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let c = frame.colors;
    let z = frame.zoom;
    let header_h = HEADER_HEIGHT * z;
    let header_w = ROW_HEADER_WIDTH * z;
    window.paint_quad(fill(
        rect(origin, 0.0, 0.0, f32::from(bounds.size.width), header_h),
        c.header,
    ));
    window.paint_quad(fill(
        rect(origin, 0.0, 0.0, header_w, f32::from(bounds.size.height)),
        c.header,
    ));
    let font_size = px(12.0 * z);
    let sel = frame.selection;
    for (col, x, w) in &columns.xs {
        let active = (sel.start.col..=sel.end.col).contains(col);
        let area = rect(origin, *x, 0.0, *w, header_h);
        let text_color = if active {
            window.paint_quad(fill(area, c.header_active));
            c.header_active_text
        } else {
            c.header_text
        };
        window.paint_quad(fill(
            rect(origin, x + w - 1.0, 0.0, 1.0, header_h),
            c.gridline,
        ));
        centered_label(
            &col.letters(),
            area,
            font_size,
            line_height,
            text_color,
            frame,
            window,
            cx,
        );
    }
    for (row, y, h) in &rows.ys {
        let active = (sel.start.row..=sel.end.row).contains(row);
        let area = rect(origin, 0.0, *y, header_w, *h);
        let text_color = if active {
            window.paint_quad(fill(area, c.header_active));
            c.header_active_text
        } else {
            c.header_text
        };
        window.paint_quad(fill(
            rect(origin, 0.0, y + h - 1.0, header_w, 1.0),
            c.gridline,
        ));
        centered_label(
            &row.to_string(),
            area,
            font_size,
            line_height,
            text_color,
            frame,
            window,
            cx,
        );
    }
    window.paint_quad(fill(
        rect(
            origin,
            0.0,
            header_h - 1.0,
            f32::from(bounds.size.width),
            1.0,
        ),
        c.gridline,
    ));
    window.paint_quad(fill(
        rect(
            origin,
            header_w - 1.0,
            0.0,
            1.0,
            f32::from(bounds.size.height),
        ),
        c.gridline,
    ));
    window.paint_quad(fill(rect(origin, 0.0, 0.0, header_w, header_h), c.header));
}

#[allow(clippy::too_many_arguments)]
fn centered_label(
    text: &str,
    area: Bounds<Pixels>,
    font_size: Pixels,
    line_height: Pixels,
    color: Hsla,
    frame: &Frame,
    window: &mut Window,
    cx: &mut App,
) {
    let run = TextRun {
        len: text.len(),
        font: font(frame.font.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        font_size,
        &[run],
        None,
    );
    let x = area.origin.x + (area.size.width - line.width) / 2.0;
    let y = area.origin.y + (area.size.height - line_height) / 2.0;
    if let Err(error) = line.paint(point(x, y), line_height, TextAlign::Left, None, window, cx) {
        tracing::warn!(%error, "failed to paint header label");
    }
}

fn paint_editor(
    frame: &Frame,
    editor: &Editor,
    columns: &Columns,
    rows: &Rows,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let c = frame.colors;
    let Some(area) = range_rect(Range::single(editor.pos), columns, rows, origin, frame.zoom)
    else {
        return;
    };
    let font_size = px(BASE_FONT_SIZE * frame.zoom);
    let line_height = font_size * 1.3;
    let run = TextRun {
        len: editor.text.len(),
        font: font(frame.font.clone()),
        color: c.foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(editor.text.clone()),
        font_size,
        &[run],
        None,
    );
    let padding = px(CELL_PADDING * frame.zoom);
    let width = (line.width + padding * 3.0).max(area.size.width);
    let editor_area = Bounds::new(area.origin, size(width, area.size.height));
    window.paint_quad(quad(
        editor_area,
        px(0.0),
        c.background,
        px(2.0),
        c.accent,
        BorderStyle::Solid,
    ));
    let text_origin = point(
        editor_area.origin.x + padding,
        editor_area.origin.y + editor_area.size.height - line_height - px(2.0 * frame.zoom),
    );
    if let Err(error) = line.paint(text_origin, line_height, TextAlign::Left, None, window, cx) {
        tracing::warn!(%error, "failed to paint editor text");
    }
    let caret_x = text_origin.x + line.x_for_index(editor.caret);
    let caret_color = match editor.mode {
        EditMode::Enter => c.foreground,
        EditMode::Edit => c.accent,
    };
    window.paint_quad(fill(
        Bounds::new(point(caret_x, text_origin.y), size(px(1.5), line_height)),
        caret_color,
    ));
}
