use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_types::{CellPos, ColIdx, HAlign, Range, Rgb, RowIdx, VAlign, ValueKind};

use crate::grid::{EditMode, Editor, GridCell, HEADER_HEIGHT};
use crate::layout::Layout;
use crate::paint_failure::PaintFailure;

const CELL_PADDING: f32 = 4.0;
const BASE_FONT_SIZE: f32 = 13.0;
const DEFAULT_POINTS: f32 = 11.0;

/// Set by the app while the high-contrast theme is active.
pub struct HighContrast(pub bool);

impl Global for HighContrast {}

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
    frozen: Hsla,
}

impl Colors {
    pub fn from_theme(cx: &App) -> Colors {
        let theme = cx.theme();
        // Excel's selection green; a lighter tone keeps contrast on dark backgrounds.
        let high_contrast = cx.try_global::<HighContrast>().is_some_and(|h| h.0);
        let accent: Hsla = if high_contrast {
            rgb(0xFF_FF_00).into()
        } else if theme.mode.is_dark() {
            rgb(0x4C_AF_7A).into()
        } else {
            rgb(0x21_73_46).into()
        };
        Colors {
            background: theme.background,
            foreground: theme.foreground,
            gridline: theme.border.opacity(0.6),
            header: theme.table_head,
            header_text: theme.muted_foreground,
            header_active: theme.border,
            header_active_text: accent,
            accent,
            selection: accent.opacity(if high_contrast { 0.3 } else { 0.14 }),
            error: theme.danger,
            frozen: theme.muted_foreground.opacity(0.6),
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
    pub fill_target: Option<Range>,
    pub zoom: f32,
    pub focused: bool,
    pub colors: Colors,
    pub font: SharedString,
    pub frozen_rows: u32,
    pub frozen_cols: u16,
    pub merges: Rc<Vec<Range>>,
    pub formula_refs: Vec<Range>,
    pub row_header: f32,
}

const FILL_HANDLE: f32 = 7.0;
const MAX_WRAPPED_CHARS: usize = 2_000;

// Excel's reference colours while editing a formula, in order of appearance.
const REFERENCE_COLORS: [u32; 6] = [0x1F6FD1, 0xD0342C, 0x7A3FB5, 0x1E8C4E, 0xB5651D, 0xC2185B];

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

fn columns(frame: &Frame, width: f32) -> Columns {
    let z = frame.zoom;
    let mut x = frame.row_header * z;
    let mut xs = Vec::with_capacity(usize::from(frame.cols) + 1);
    let frozen = (0..frame.frozen_cols).map(|c| ColIdx::clamped(i64::from(c)));
    let start = frame
        .left
        .max(ColIdx::clamped(i64::from(frame.frozen_cols)));
    let scrolled = (start.get()..=ColIdx::LAST.get()).map(|c| ColIdx::clamped(i64::from(c)));
    for col in frozen.chain(scrolled) {
        if x >= width {
            break;
        }
        let w = frame.layout.col_width(col) * z;
        xs.push((col, x, w));
        x += w;
    }
    Columns { xs }
}

fn rows(frame: &Frame, height: f32) -> Rows {
    let z = frame.zoom;
    let mut y = HEADER_HEIGHT * z;
    let mut ys = Vec::with_capacity(frame.rows as usize + 1);
    let frozen = (0..frame.frozen_rows).map(|r| RowIdx::clamped(i64::from(r)));
    let start = frame.top.max(RowIdx::clamped(i64::from(frame.frozen_rows)));
    let scrolled = (start.get()..=RowIdx::LAST.get()).map(|r| RowIdx::clamped(i64::from(r)));
    for row in frozen.chain(scrolled) {
        if y >= height {
            break;
        }
        let h = frame.layout.row_height(row) * z;
        ys.push((row, y, h));
        y += h;
    }
    Rows { ys }
}

pub fn paint(frame: &Frame, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let z = frame.zoom;
    let c = frame.colors;
    let origin = bounds.origin;
    window.paint_quad(fill(bounds, c.background));

    let columns = columns(frame, f32::from(bounds.size.width));
    let rows = rows(frame, f32::from(bounds.size.height));
    let right = columns.xs.last().map_or(0.0, |(_, x, w)| x + w);
    let bottom = rows.ys.last().map_or(0.0, |(_, y, h)| y + h);

    let font_size = BASE_FONT_SIZE * z;
    let line_height = px(font_size * 1.3);
    let editing = frame.editor.as_ref().map(|e| e.pos);
    let visible = match (
        columns.xs.first(),
        columns.xs.last(),
        rows.ys.first(),
        rows.ys.last(),
    ) {
        (Some(c0), Some(c1), Some(r0), Some(r1)) => Some(Range::new(
            CellPos::new(r0.0, c0.0),
            CellPos::new(r1.0, c1.0),
        )),
        _ => None,
    };
    let merges: Vec<Range> = frame
        .merges
        .iter()
        .filter(|m| visible.is_some_and(|v| v.intersects(m)))
        .copied()
        .collect();
    let merged = |pos: CellPos| merges.iter().any(|m| m.contains(pos));

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (row, y, h) in &rows.ys {
            for (col, x, w) in &columns.xs {
                let Some(cell) = frame.cells.get(&CellPos::new(*row, *col)) else {
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
                rect(origin, frame.row_header * z, y + h - 1.0, right, 1.0),
                c.gridline,
            ));
        }

        for merge in &merges {
            let Some(area) = range_rect(*merge, &columns, &rows, origin, z) else {
                continue;
            };
            let cell = frame.cells.get(&merge.start);
            let background = cell
                .and_then(|cell| cell.style.fill)
                .map_or(c.background, rgb_to_hsla);
            let inner = Bounds::new(
                area.origin,
                size(area.size.width - px(1.0), area.size.height - px(1.0)),
            );
            window.paint_quad(fill(inner, background));
            if let Some(cell) = cell
                && !cell.text.is_empty()
                && Some(merge.start) != editing
            {
                paint_cell_text(frame, cell, area, area.size.width, window, cx);
            }
        }

        for (row, y, h) in &rows.ys {
            for (col_index, (col, x, w)) in columns.xs.iter().enumerate() {
                let pos = CellPos::new(*row, *col);
                if Some(pos) == editing || merged(pos) {
                    continue;
                }
                let Some(cell) = frame.cells.get(&pos) else {
                    continue;
                };
                // Top and left lines sit on the neighbour's bottom and right pixel, so a
                // border two adjacent cells share stays one pixel thin.
                // The first row and column have no neighbour pixel: that one is the header's.
                let top = if *y > HEADER_HEIGHT { y - 1.0 } else { *y };
                let left = if *x > frame.row_header { x - 1.0 } else { *x };
                if cell.style.border_top {
                    window.paint_quad(fill(
                        rect(origin, left, top, w + x - left, 1.0),
                        c.foreground,
                    ));
                }
                if cell.style.border_left {
                    window.paint_quad(fill(
                        rect(origin, left, top, 1.0, h + y - top),
                        c.foreground,
                    ));
                }
                if cell.style.border_bottom {
                    window.paint_quad(fill(rect(origin, *x, y + h - 1.0, *w, 1.0), c.foreground));
                }
                if cell.style.border_right {
                    window.paint_quad(fill(rect(origin, x + w - 1.0, *y, 1.0, *h), c.foreground));
                }
                if cell.text.is_empty() {
                    continue;
                }
                let overflow = if overflows_right(cell) {
                    columns.xs[col_index + 1..]
                        .iter()
                        .take_while(|(next, _, _)| {
                            let next = CellPos::new(*row, *next);
                            !frame.cells.get(&next).is_some_and(|n| !n.text.is_empty())
                                && !merged(next)
                                && Some(next) != editing
                        })
                        .map(|(_, _, w)| *w)
                        .sum::<f32>()
                } else {
                    0.0
                };
                let area = rect(origin, *x, *y, *w, *h);
                paint_cell_text(frame, cell, area, px(w + overflow), window, cx);
            }
        }

        if frame.frozen_rows > 0
            && let Some((_, y, h)) = rows.ys.get(frame.frozen_rows as usize - 1)
        {
            window.paint_quad(fill(rect(origin, 0.0, y + h - 1.0, right, 1.0), c.frozen));
        }
        if frame.frozen_cols > 0
            && let Some((_, x, w)) = columns.xs.get(usize::from(frame.frozen_cols) - 1)
        {
            window.paint_quad(fill(rect(origin, x + w - 1.0, 0.0, 1.0, bottom), c.frozen));
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

fn overflows_right(cell: &GridCell) -> bool {
    cell.kind == ValueKind::Text
        && !cell.style.wrap
        && matches!(cell.style.align, HAlign::General | HAlign::Left)
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
    max_width: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let c = frame.colors;
    // Files store automatic text as black; on a dark sheet it follows the theme,
    // unless the cell has its own fill, which was designed for the file's colours.
    let automatic_black = matches!(cell.style.font_color, None | Some(Rgb(0)));
    let color = match (cell.kind, cell.style.font_color) {
        (ValueKind::Error, _) => c.error,
        _ if automatic_black && cell.style.fill.is_none() => c.foreground,
        (_, Some(rgb_color)) => rgb_to_hsla(rgb_color),
        (_, None) => rgb(0x000000).into(),
    };
    let points = cell.style.font_size.unwrap_or(DEFAULT_POINTS);
    let font_size = px(BASE_FONT_SIZE * points / DEFAULT_POINTS * frame.zoom);
    let line_height = font_size * 1.3;
    let padding = px(CELL_PADDING * frame.zoom);
    let shape = |text: SharedString, window: &mut Window| {
        let run = text_run(frame, cell, text.len(), color);
        window
            .text_system()
            .shape_line(text, font_size, &[run], None)
    };
    if cell.style.wrap && cell.kind == ValueKind::Text {
        paint_wrapped_text(
            frame,
            cell,
            bounds,
            color,
            font_size,
            line_height,
            padding,
            window,
            cx,
        );
        return;
    }
    // Like wrapped text, a single line never shows more than a couple of thousand
    // characters, so a 32k-character cell is not shaped in full every frame.
    let shown: SharedString = match cell.text.char_indices().nth(MAX_WRAPPED_CHARS) {
        Some((cut, _)) => cell.text[..cut].to_string().into(),
        None => cell.text.clone(),
    };
    let mut line = shape(shown, window);
    let fits = |width: Pixels| width + padding * 2.0 <= bounds.size.width;
    if cell.kind == ValueKind::Number && !fits(line.width) && cell.style.num_fmt == "general" {
        for spelling in crate::general::shorter_spellings(&cell.text) {
            let shorter = shape(spelling.into(), window);
            if fits(shorter.width) {
                line = shorter;
                break;
            }
        }
    }
    // Excel never truncates a number: one that still does not fit shows as #### instead.
    if cell.kind == ValueKind::Number && !fits(line.width) {
        let hash = shape("#".into(), window).width.max(px(1.0));
        let count = ((bounds.size.width - padding * 2.0) / hash)
            .floor()
            .max(1.0) as usize;
        line = shape("#".repeat(count).into(), window);
    }
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
    let y = vertical_origin(cell.style.valign, bounds, line_height, frame.zoom);
    let needed = width + padding * 2.0;
    let clip_width = if needed > bounds.size.width && max_width > bounds.size.width {
        let spill_width = needed.min(max_width);
        let spill = Bounds::new(
            point(
                bounds.origin.x + bounds.size.width - px(1.0),
                bounds.origin.y,
            ),
            size(
                spill_width - bounds.size.width,
                bounds.size.height - px(1.0),
            ),
        );
        let background = cell.style.fill.map_or(frame.colors.background, rgb_to_hsla);
        window.paint_quad(fill(spill, background));
        spill_width
    } else {
        bounds.size.width
    };
    let clip = Bounds::new(
        bounds.origin,
        size(clip_width - px(1.0), bounds.size.height - px(1.0)),
    );
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        if let Err(error) = line.paint(point(x, y), line_height, TextAlign::Left, None, window, cx)
            && PaintFailure::CellText.first_occurrence()
        {
            tracing::warn!(%error, "failed to paint cell text");
        }
    });
}

fn vertical_origin(valign: VAlign, bounds: Bounds<Pixels>, height: Pixels, zoom: f32) -> Pixels {
    match valign {
        VAlign::Top => bounds.origin.y + px(1.0 * zoom),
        VAlign::Center => bounds.origin.y + (bounds.size.height - height) / 2.0,
        VAlign::Bottom => bounds.origin.y + bounds.size.height - height - px(2.0 * zoom),
    }
}

// Wrapped text breaks at the cell width and stays inside the cell, as in Excel; lines
// that do not fit the row height are clipped.
#[allow(clippy::too_many_arguments)]
fn paint_wrapped_text(
    frame: &Frame,
    cell: &GridCell,
    bounds: Bounds<Pixels>,
    color: Hsla,
    font_size: Pixels,
    line_height: Pixels,
    padding: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let width = (bounds.size.width - padding * 2.0).max(px(1.0));
    // A cell shows a few hundred characters at most; shaping all 32k of a hostile cell
    // every frame would stall the grid.
    let text: SharedString = match cell.text.char_indices().nth(MAX_WRAPPED_CHARS) {
        Some((cut, _)) => cell.text[..cut].to_string().into(),
        None => cell.text.clone(),
    };
    let run = text_run(frame, cell, text.len(), color);
    let lines = match window
        .text_system()
        .shape_text(text, font_size, &[run], Some(width), None)
    {
        Ok(lines) => lines,
        Err(error) => {
            if PaintFailure::WrappedShape.first_occurrence() {
                tracing::warn!(%error, "failed to shape wrapped cell text");
            }
            return;
        }
    };
    let height = lines
        .iter()
        .map(|line| line.size(line_height).height)
        .fold(px(0.0), |total, h| total + h);
    let align = match cell.style.align {
        HAlign::Center => TextAlign::Center,
        HAlign::Right => TextAlign::Right,
        HAlign::Left | HAlign::General => TextAlign::Left,
    };
    let mut y = vertical_origin(cell.style.valign, bounds, height, frame.zoom).max(bounds.origin.y);
    let clip = Bounds::new(
        bounds.origin,
        size(bounds.size.width - px(1.0), bounds.size.height - px(1.0)),
    );
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        for line in &lines {
            if y > bounds.origin.y + bounds.size.height {
                break;
            }
            let origin = point(bounds.origin.x + padding, y);
            if let Err(error) = line.paint(
                origin,
                line_height,
                align,
                Some(Bounds::new(origin, size(width, line_height))),
                window,
                cx,
            ) && PaintFailure::WrappedText.first_occurrence()
            {
                tracing::warn!(%error, "failed to paint wrapped cell text");
            }
            y += line.size(line_height).height;
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
        .map_or(first_col.1 - 2.0, |(x, _)| x);
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
            origin.x + px(frame.row_header * z),
            origin.y + px(HEADER_HEIGHT * z),
        ),
        size(
            bounds.size.width - px(frame.row_header * z),
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
            // Only where it can be grabbed: the selection's own corner, not a clipped edge.
            let end = frame.selection.end;
            if frame.editor.is_none()
                && columns.find(end.col).is_some()
                && rows.find(end.row).is_some()
            {
                let side = px(FILL_HANDLE * z);
                let corner = area.bottom_right() - point(side / 2.0, side / 2.0);
                window.paint_quad(quad(
                    Bounds::new(corner, size(side, side)),
                    px(0.0),
                    c.accent,
                    px(1.0),
                    c.background,
                    BorderStyle::Solid,
                ));
            }
        }
        if let Some(target) = frame.fill_target
            && target != frame.selection
            && let Some(area) = range_rect(target, columns, rows, origin, z)
        {
            window.paint_quad(quad(
                area,
                px(0.0),
                transparent_black(),
                px(1.0),
                c.frozen,
                BorderStyle::Dashed,
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
        for (index, reference) in frame.formula_refs.iter().enumerate() {
            if let Some(area) = range_rect(*reference, columns, rows, origin, z) {
                let color: Hsla = rgb(REFERENCE_COLORS[index % REFERENCE_COLORS.len()]).into();
                window.paint_quad(fill(area, color.opacity(0.08)));
                window.paint_quad(quad(
                    area,
                    px(0.0),
                    transparent_black(),
                    px(2.0),
                    color,
                    BorderStyle::Solid,
                ));
            }
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
    let header_w = frame.row_header * z;
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
        // Hidden columns have no width and no label.
        if *w <= 0.0 {
            continue;
        }
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
        if *h <= 0.0 {
            continue;
        }
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
    if let Err(error) = line.paint(point(x, y), line_height, TextAlign::Left, None, window, cx)
        && PaintFailure::HeaderLabel.first_occurrence()
    {
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
    if let Err(error) = line.paint(text_origin, line_height, TextAlign::Left, None, window, cx)
        && PaintFailure::EditorText.first_occurrence()
    {
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
