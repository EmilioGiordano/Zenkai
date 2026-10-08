use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_types::{CellPos, CellStyle, ColIdx, Range, RowIdx, ValueKind};

use crate::actions::*;
use crate::autocomplete;
use crate::formula_refs;
use crate::layout::Layout;
use crate::paint::{self, Frame};

pub const HEADER_HEIGHT: f32 = 22.0;
const MIN_ROW_HEADER_WIDTH: f32 = 48.0;

// Wide enough for the largest row number in view, as Excel widens its row headers.
pub fn row_header_width(last_visible_row: RowIdx) -> f32 {
    let digits = (last_visible_row.get() + 1).to_string().len() as f32;
    MIN_ROW_HEADER_WIDTH.max(digits * 8.0 + 14.0)
}
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;

#[derive(Clone, Debug, PartialEq)]
pub struct GridCell {
    pub text: SharedString,
    pub kind: ValueKind,
    pub style: CellStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Enter,
    Edit,
}

#[derive(Clone, Debug)]
pub struct Editor {
    pub pos: CellPos,
    pub text: String,
    pub caret: usize,
    pub mode: EditMode,
    pub point: Option<PointRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointRef {
    pub anchor: CellPos,
    pub corner: CellPos,
    pub text_start: usize,
}

impl Editor {
    fn insert_reference(&mut self, anchor: CellPos, corner: CellPos) {
        let text_start = self
            .point
            .map_or(self.text.len(), |p| p.text_start)
            .min(self.text.len());
        self.text.truncate(text_start);
        self.text
            .push_str(&formula_refs::reference_text(anchor, corner));
        self.caret = self.text.len();
        self.point = Some(PointRef {
            anchor,
            corner,
            text_start,
        });
    }

    fn can_point(&self) -> bool {
        self.mode == EditMode::Enter
            && (self.point.is_some() || formula_refs::accepts_reference(&self.text, self.caret))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub active: CellPos,
    pub corner: CellPos,
}

impl Selection {
    pub fn range(&self) -> Range {
        Range::new(self.active, self.corner)
    }
}

#[derive(Clone, Debug)]
pub enum GridEvent {
    SelectionChanged,
    ViewportChanged,
    EditRequested(CellPos),
    EditChanged,
    Commit {
        pos: CellPos,
        text: String,
    },
    Jump {
        direction: Direction,
        extend: bool,
    },
    ClearRequested(Range),
    EndRequested {
        extend: bool,
    },
    ColumnResized {
        col: ColIdx,
        width: f32,
    },
    RowResized {
        row: RowIdx,
        height: f32,
    },
    FillRequested {
        source: Range,
        target: Range,
    },
    CommitToSelection {
        pos: CellPos,
        text: String,
        range: Range,
    },
    AutoFitRequested(ColIdx),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Column(ColIdx),
    Row(RowIdx),
}

#[derive(Clone, Copy, Debug)]
struct EdgeDrag {
    edge: Edge,
    start: f32,
    start_size: f32,
}

const RESIZE_GRIP: f32 = 4.0;
const MIN_COLUMN_WIDTH: f32 = 8.0;
const MIN_ROW_HEIGHT: f32 = 4.0;
const FILL_GRIP: f32 = 4.0;

pub struct Grid {
    focus: FocusHandle,
    layout: Rc<Layout>,
    cells: Rc<HashMap<CellPos, GridCell>>,
    top: RowIdx,
    left: ColIdx,
    visible_rows: u32,
    visible_cols: u16,
    bounds: Bounds<Pixels>,
    selection: Selection,
    editor: Option<Editor>,
    zoom: f32,
    dragging: bool,
    marquee: Option<Range>,
    tab_start: Option<ColIdx>,
    frozen_rows: u32,
    frozen_cols: u16,
    merges: Rc<Vec<Range>>,
    last_paint: Rc<Cell<Duration>>,
    active_formula: SharedString,
    edge_drag: Option<EdgeDrag>,
    resize_hover: Option<Edge>,
    fill_target: Option<Range>,
    fill_hover: bool,
    suggestion: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SheetView {
    pub layout: Layout,
    pub frozen_rows: u32,
    pub frozen_cols: u16,
    pub merges: Vec<Range>,
}

impl EventEmitter<GridEvent> for Grid {}

impl Focusable for Grid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Grid {
    pub fn new(cx: &mut Context<Self>) -> Grid {
        Grid {
            focus: cx.focus_handle(),
            layout: Rc::new(Layout::default()),
            cells: Rc::new(HashMap::new()),
            top: RowIdx::default(),
            left: ColIdx::default(),
            visible_rows: 40,
            visible_cols: 20,
            bounds: Bounds::default(),
            selection: Selection {
                active: CellPos::default(),
                corner: CellPos::default(),
            },
            editor: None,
            zoom: 1.0,
            dragging: false,
            marquee: None,
            tab_start: None,
            frozen_rows: 0,
            frozen_cols: 0,
            merges: Rc::new(Vec::new()),
            last_paint: Rc::new(Cell::new(Duration::ZERO)),
            active_formula: SharedString::default(),
            edge_drag: None,
            resize_hover: None,
            fill_target: None,
            fill_hover: false,
            suggestion: 0,
        }
    }

    pub fn set_active_formula(&mut self, formula: SharedString) {
        self.active_formula = formula;
    }

    pub fn last_paint(&self) -> Duration {
        self.last_paint.get()
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    fn row_header(&self) -> f32 {
        let origin = self.scroll_origin();
        row_header_width(origin.row.offset(i64::from(self.visible_rows)))
    }

    fn scroll_origin(&self) -> CellPos {
        CellPos::new(
            self.top.max(RowIdx::clamped(i64::from(self.frozen_rows))),
            self.left.max(ColIdx::clamped(i64::from(self.frozen_cols))),
        )
    }

    pub fn visible_counts(&self) -> (u32, u16) {
        (self.visible_rows, self.visible_cols)
    }

    pub fn cached_ranges(&self) -> Vec<Range> {
        cached_ranges(
            self.scroll_origin(),
            self.visible_rows,
            self.visible_cols,
            self.frozen_rows,
            self.frozen_cols,
        )
    }

    pub fn reset(&mut self, view: SheetView, cx: &mut Context<Self>) {
        self.layout = Rc::new(view.layout);
        self.frozen_rows = view.frozen_rows;
        self.frozen_cols = view.frozen_cols;
        self.merges = Rc::new(view.merges);
        self.cells = Rc::new(HashMap::new());
        self.top = RowIdx::default();
        self.left = ColIdx::default();
        self.editor = None;
        self.marquee = None;
        self.selection = Selection {
            active: CellPos::default(),
            corner: CellPos::default(),
        };
        self.viewport_changed(cx);
        cx.emit(GridEvent::SelectionChanged);
    }

    pub fn update_view(&mut self, view: SheetView, cx: &mut Context<Self>) {
        self.layout = Rc::new(view.layout);
        self.frozen_rows = view.frozen_rows;
        self.frozen_cols = view.frozen_cols;
        self.merges = Rc::new(view.merges);
        self.viewport_changed(cx);
    }

    pub fn set_cells(&mut self, cells: HashMap<CellPos, GridCell>, cx: &mut Context<Self>) {
        self.cells = Rc::new(cells);
        cx.notify();
    }

    pub fn update_cells(
        &mut self,
        changes: impl IntoIterator<Item = (CellPos, GridCell)>,
        cx: &mut Context<Self>,
    ) {
        let cells = Rc::make_mut(&mut self.cells);
        for (pos, cell) in changes {
            cells.insert(pos, cell);
        }
        cx.notify();
    }

    pub fn set_marquee(&mut self, range: Option<Range>, cx: &mut Context<Self>) {
        self.marquee = range;
        cx.notify();
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.recompute_viewport(cx);
        cx.notify();
    }

    pub fn show_range(&mut self, range: Range, cx: &mut Context<Self>) {
        self.tab_start = None;
        self.selection = Selection {
            active: range.start,
            corner: range.end,
        };
        self.scroll_into_view(range.start, cx);
        cx.emit(GridEvent::SelectionChanged);
        cx.notify();
    }

    pub fn select(&mut self, active: CellPos, corner: CellPos, cx: &mut Context<Self>) {
        self.tab_start = None;
        self.selection = Selection { active, corner };
        self.scroll_into_view(corner, cx);
        cx.emit(GridEvent::SelectionChanged);
        cx.notify();
    }

    pub fn begin_edit(
        &mut self,
        pos: CellPos,
        text: String,
        mode: EditMode,
        cx: &mut Context<Self>,
    ) {
        let caret = text.len();
        self.editor = Some(Editor {
            pos,
            text,
            caret,
            mode,
            point: None,
        });
        cx.emit(GridEvent::EditChanged);
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.editor.take().is_some() {
            cx.emit(GridEvent::EditChanged);
            cx.notify();
        }
    }

    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.take() {
            cx.emit(GridEvent::Commit {
                pos: editor.pos,
                text: editor.text,
            });
            cx.emit(GridEvent::EditChanged);
        }
    }

    fn move_active(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let pos = self.step_visible(self.selection.active, direction);
        self.select(pos, pos, cx);
    }

    // Arrow keys pass over hidden rows and columns, as in Excel; at the sheet edge, or
    // when everything beyond is hidden, the position stays put.
    fn step_visible(&self, from: CellPos, direction: Direction) -> CellPos {
        let mut pos = from;
        loop {
            let next = step(pos, direction, 1);
            if next == pos {
                return from;
            }
            pos = next;
            let hidden = match direction {
                Direction::Up | Direction::Down => self.layout.row_height(pos.row) == 0.0,
                Direction::Left | Direction::Right => self.layout.col_width(pos.col) == 0.0,
            };
            if !hidden {
                return pos;
            }
        }
    }

    fn navigate(&mut self, direction: Direction, extend: bool, cx: &mut Context<Self>) {
        let shown = self.suggestions().len();
        if shown > 0 && !extend && matches!(direction, Direction::Up | Direction::Down) {
            self.suggestion = match direction {
                Direction::Up => (self.suggestion + shown - 1) % shown,
                _ => (self.suggestion + 1) % shown,
            };
            cx.notify();
            return;
        }
        if let Some(editor) = &mut self.editor {
            if editor.can_point() {
                let (anchor, corner) = match editor.point {
                    Some(point) if extend => (point.anchor, step(point.corner, direction, 1)),
                    Some(point) => {
                        let moved = step(point.corner, direction, 1);
                        (moved, moved)
                    }
                    None => {
                        let first = step(editor.pos, direction, 1);
                        (first, first)
                    }
                };
                editor.insert_reference(anchor, corner);
                self.scroll_into_view(corner, cx);
                cx.emit(GridEvent::EditChanged);
                cx.notify();
                return;
            }
            if editor.mode == EditMode::Edit {
                match direction {
                    Direction::Left => editor.caret = prev_boundary(&editor.text, editor.caret),
                    Direction::Right => editor.caret = next_boundary(&editor.text, editor.caret),
                    Direction::Up | Direction::Down => {}
                }
                cx.notify();
                return;
            }
            self.commit_edit(cx);
        }
        if extend {
            let corner = self.step_visible(self.selection.corner, direction);
            self.select(self.selection.active, corner, cx);
        } else {
            self.move_active(direction, cx);
        }
    }

    fn page(&mut self, down: bool, cx: &mut Context<Self>) {
        let delta = i64::from(self.visible_rows.saturating_sub(1).max(1));
        let delta = if down { delta } else { -delta };
        self.top = self.top.offset(delta);
        let pos = CellPos::new(
            self.selection.active.row.offset(delta),
            self.selection.active.col,
        );
        self.select(pos, pos, cx);
        self.viewport_changed(cx);
    }

    // Frozen rows and columns are always on screen, so only the scrolled pane moves.
    fn scroll_into_view(&mut self, pos: CellPos, cx: &mut Context<Self>) {
        let origin = self.scroll_origin();
        let mut top = origin.row;
        let mut left = origin.col;
        if pos.row.get() >= self.frozen_rows {
            if pos.row < top {
                top = pos.row;
            } else if pos.row.get() >= top.get() + self.visible_rows.saturating_sub(1) {
                top = RowIdx::clamped(
                    i64::from(pos.row.get()) - i64::from(self.visible_rows.saturating_sub(2)),
                );
            }
        }
        if pos.col.get() >= self.frozen_cols {
            if pos.col < left {
                left = pos.col;
            } else if pos.col.get() >= left.get() + self.visible_cols.saturating_sub(1) {
                left = ColIdx::clamped(
                    i64::from(pos.col.get()) - i64::from(self.visible_cols.saturating_sub(2)),
                );
            }
        }
        if (top, left) != (self.top, self.left) {
            self.top = top;
            self.left = left;
            self.viewport_changed(cx);
        }
    }

    fn viewport_changed(&mut self, cx: &mut Context<Self>) {
        self.recompute_viewport(cx);
        cx.emit(GridEvent::ViewportChanged);
        cx.notify();
    }

    fn recompute_viewport(&mut self, _cx: &mut Context<Self>) {
        let (frozen_w, frozen_h) = self.frozen_size();
        let origin = self.scroll_origin();
        let width =
            (f32::from(self.bounds.size.width) / self.zoom - self.row_header() - frozen_w).max(0.0);
        let height =
            (f32::from(self.bounds.size.height) / self.zoom - HEADER_HEIGHT - frozen_h).max(0.0);
        self.visible_cols = self.layout.visible_cols(origin.col, width);
        self.visible_rows = self.layout.visible_rows(origin.row, height);
    }

    fn frozen_size(&self) -> (f32, f32) {
        let width = (0..self.frozen_cols)
            .map(|c| self.layout.col_width(ColIdx::clamped(i64::from(c))))
            .sum();
        let height = (0..self.frozen_rows)
            .map(|r| self.layout.row_height(RowIdx::clamped(i64::from(r))))
            .sum();
        (width, height)
    }

    fn set_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.bounds != bounds {
            self.bounds = bounds;
            self.viewport_changed(cx);
        }
    }

    // The column edge under the pointer in the column header, or the row edge under it in
    // the row header, with the pointer coordinate along the axis that resizes.
    fn edge_at(&self, position: Point<Pixels>) -> Option<(Edge, f32)> {
        let x = f32::from(position.x - self.bounds.origin.x) / self.zoom;
        let y = f32::from(position.y - self.bounds.origin.y) / self.zoom;
        let origin = self.scroll_origin();
        if (0.0..HEADER_HEIGHT).contains(&y) {
            let frozen = (0..self.frozen_cols).map(|c| ColIdx::clamped(i64::from(c)));
            let scrolled = (0..self.visible_cols).map(|c| origin.col.offset(i64::from(c)));
            let mut edge = self.row_header();
            for col in frozen.chain(scrolled) {
                edge += self.layout.col_width(col);
                if (x - edge).abs() <= RESIZE_GRIP {
                    return Some((Edge::Column(col), x));
                }
                if edge > x + RESIZE_GRIP {
                    return None;
                }
            }
        } else if (0.0..self.row_header()).contains(&x) {
            let frozen = (0..self.frozen_rows).map(|r| RowIdx::clamped(i64::from(r)));
            let scrolled = (0..self.visible_rows).map(|r| origin.row.offset(i64::from(r)));
            let mut edge = HEADER_HEIGHT;
            for row in frozen.chain(scrolled) {
                edge += self.layout.row_height(row);
                if (y - edge).abs() <= RESIZE_GRIP {
                    return Some((Edge::Row(row), y));
                }
                if edge > y + RESIZE_GRIP {
                    return None;
                }
            }
        }
        None
    }

    // Right edge of a visible column and bottom edge of a visible row, in unzoomed grid
    // coordinates.
    fn col_right(&self, target: ColIdx) -> Option<f32> {
        let origin = self.scroll_origin();
        let frozen = (0..self.frozen_cols).map(|c| ColIdx::clamped(i64::from(c)));
        let scrolled = (0..self.visible_cols).map(|c| origin.col.offset(i64::from(c)));
        let mut edge = self.row_header();
        for col in frozen.chain(scrolled) {
            edge += self.layout.col_width(col);
            if col == target {
                return Some(edge);
            }
        }
        None
    }

    fn row_bottom(&self, target: RowIdx) -> Option<f32> {
        let origin = self.scroll_origin();
        let frozen = (0..self.frozen_rows).map(|r| RowIdx::clamped(i64::from(r)));
        let scrolled = (0..self.visible_rows).map(|r| origin.row.offset(i64::from(r)));
        let mut edge = HEADER_HEIGHT;
        for row in frozen.chain(scrolled) {
            edge += self.layout.row_height(row);
            if row == target {
                return Some(edge);
            }
        }
        None
    }

    // The fill handle: the small square at the bottom-right corner of the selection.
    fn on_fill_handle(&self, position: Point<Pixels>) -> bool {
        if self.editor.is_some() {
            return false;
        }
        let end = self.selection.range().end;
        let (Some(right), Some(bottom)) = (self.col_right(end.col), self.row_bottom(end.row))
        else {
            return false;
        };
        let x = f32::from(position.x - self.bounds.origin.x) / self.zoom;
        let y = f32::from(position.y - self.bounds.origin.y) / self.zoom;
        (x - right).abs() <= FILL_GRIP && (y - bottom).abs() <= FILL_GRIP
    }

    // Excel extends down when the pointer passes the last row, else right when it
    // passes the last column; up and left are not supported yet.
    fn fill_target_for(&self, pos: CellPos) -> Range {
        let source = self.selection.range();
        if pos.row > source.end.row {
            Range::new(source.start, CellPos::new(pos.row, source.end.col))
        } else if pos.col > source.end.col {
            Range::new(source.start, CellPos::new(source.end.row, pos.col))
        } else {
            source
        }
    }

    fn finish_fill_drag(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.fill_target.take() else {
            return;
        };
        let source = self.selection.range();
        if target != source {
            cx.emit(GridEvent::FillRequested { source, target });
            self.select(target.start, target.end, cx);
        }
        cx.notify();
    }

    fn edge_size(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Column(col) => self.layout.col_width(col),
            Edge::Row(row) => self.layout.row_height(row),
        }
    }

    fn set_edge_size(&mut self, edge: Edge, size: f32) {
        let layout = Rc::make_mut(&mut self.layout);
        match edge {
            Edge::Column(col) => layout.set_col_width(col, size.max(MIN_COLUMN_WIDTH)),
            Edge::Row(row) => layout.set_row_height(row, size.max(MIN_ROW_HEIGHT)),
        }
    }

    fn finish_edge_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.edge_drag.take() else {
            return;
        };
        let size = self.edge_size(drag.edge);
        // A click without movement is not an edit.
        if (size - drag.start_size).abs() < 0.5 {
            self.set_edge_size(drag.edge, drag.start_size);
        } else {
            cx.emit(match drag.edge {
                Edge::Column(col) => GridEvent::ColumnResized { col, width: size },
                Edge::Row(row) => GridEvent::RowResized { row, height: size },
            });
        }
        self.viewport_changed(cx);
    }

    fn hit(&self, position: Point<Pixels>) -> Hit {
        let x = f32::from(position.x - self.bounds.origin.x) / self.zoom;
        let y = f32::from(position.y - self.bounds.origin.y) / self.zoom;
        let (frozen_w, frozen_h) = self.frozen_size();
        let origin = self.scroll_origin();
        let body_y = y - HEADER_HEIGHT;
        let body_x = x - self.row_header();
        let row = if body_y < frozen_h {
            self.layout.row_at(RowIdx::default(), body_y)
        } else {
            self.layout.row_at(origin.row, body_y - frozen_h)
        };
        let col = if body_x < frozen_w {
            self.layout.col_at(ColIdx::default(), body_x)
        } else {
            self.layout.col_at(origin.col, body_x - frozen_w)
        };
        match (x < self.row_header(), y < HEADER_HEIGHT) {
            (true, true) => Hit::Corner,
            (true, false) => Hit::RowHeader(row),
            (false, true) => Hit::ColHeader(col),
            (false, false) => Hit::Cell(CellPos::new(row, col)),
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if let Some((edge, start)) = self.edge_at(event.position) {
            match edge {
                Edge::Column(col) if event.click_count >= 2 => {
                    cx.emit(GridEvent::AutoFitRequested(col));
                }
                _ => {
                    self.edge_drag = Some(EdgeDrag {
                        edge,
                        start,
                        start_size: self.edge_size(edge),
                    });
                }
            }
            return;
        }
        if self.on_fill_handle(event.position) {
            self.fill_target = Some(self.selection.range());
            return;
        }
        let hit = self.hit(event.position);
        if let (Some(editor), Hit::Cell(pos)) = (&mut self.editor, hit)
            && editor.can_point()
        {
            let anchor = match editor.point {
                Some(point) if event.modifiers.shift => point.anchor,
                _ => pos,
            };
            editor.insert_reference(anchor, pos);
            self.dragging = true;
            cx.emit(GridEvent::EditChanged);
            cx.notify();
            return;
        }
        if self.editor.is_some() {
            self.commit_edit(cx);
        }
        let extend = event.modifiers.shift;
        match self.hit(event.position) {
            Hit::Corner => self.select_all(cx),
            Hit::RowHeader(row) => self.select(
                CellPos::new(row, ColIdx::default()),
                CellPos::new(row, ColIdx::LAST),
                cx,
            ),
            Hit::ColHeader(col) => self.select(
                CellPos::new(RowIdx::default(), col),
                CellPos::new(RowIdx::LAST, col),
                cx,
            ),
            Hit::Cell(pos) if extend => self.select(self.selection.active, pos, cx),
            Hit::Cell(pos) => {
                self.select(pos, pos, cx);
                self.dragging = true;
                if event.click_count >= 2 {
                    cx.emit(GridEvent::EditRequested(pos));
                }
            }
        }
    }

    // Like Excel, a right click outside the selection first selects what is under the
    // pointer, so the context menu acts on it; inside the selection it keeps it.
    fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if self.editor.is_some() {
            self.commit_edit(cx);
        }
        let range = self.selection.range();
        let whole_rows = range.start.col == ColIdx::default() && range.end.col == ColIdx::LAST;
        let whole_cols = range.start.row == RowIdx::default() && range.end.row == RowIdx::LAST;
        match self.hit(event.position) {
            Hit::Cell(pos) if !range.contains(pos) => self.select(pos, pos, cx),
            Hit::RowHeader(row)
                if !(whole_rows && (range.start.row..=range.end.row).contains(&row)) =>
            {
                self.select(
                    CellPos::new(row, ColIdx::default()),
                    CellPos::new(row, ColIdx::LAST),
                    cx,
                )
            }
            Hit::ColHeader(col)
                if !(whole_cols && (range.start.col..=range.end.col).contains(&col)) =>
            {
                self.select(
                    CellPos::new(RowIdx::default(), col),
                    CellPos::new(RowIdx::LAST, col),
                    cx,
                )
            }
            _ => {}
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.edge_drag {
            // Released outside the grid: end the drag where it is.
            if event.pressed_button != Some(MouseButton::Left) {
                self.finish_edge_drag(cx);
                return;
            }
            let now = match drag.edge {
                Edge::Column(_) => f32::from(event.position.x - self.bounds.origin.x),
                Edge::Row(_) => f32::from(event.position.y - self.bounds.origin.y),
            } / self.zoom;
            self.set_edge_size(drag.edge, drag.start_size + now - drag.start);
            self.recompute_viewport(cx);
            cx.notify();
            return;
        }
        if self.fill_target.is_some() {
            if event.pressed_button != Some(MouseButton::Left) {
                self.finish_fill_drag(cx);
                return;
            }
            if let Hit::Cell(pos) = self.hit(event.position) {
                let target = self.fill_target_for(pos);
                if self.fill_target != Some(target) {
                    self.fill_target = Some(target);
                    cx.notify();
                }
            }
            return;
        }
        let fill_hover = self.on_fill_handle(event.position);
        if fill_hover != self.fill_hover {
            self.fill_hover = fill_hover;
            cx.notify();
        }
        let hover = self.edge_at(event.position).map(|(edge, _)| edge);
        if hover != self.resize_hover {
            self.resize_hover = hover;
            cx.notify();
        }
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        let hit = self.hit(event.position);
        if let (Some(editor), Hit::Cell(pos)) = (&mut self.editor, hit)
            && let Some(point) = editor.point
        {
            if point.corner != pos {
                editor.insert_reference(point.anchor, pos);
                cx.emit(GridEvent::EditChanged);
                cx.notify();
            }
            return;
        }
        if let Hit::Cell(pos) = self.hit(event.position)
            && pos != self.selection.corner
        {
            self.selection.corner = pos;
            cx.emit(GridEvent::SelectionChanged);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line = px(20.0 * self.zoom);
        let delta = event.delta.pixel_delta(line);
        if event.modifiers.control {
            let step = if f32::from(delta.y) > 0.0 { 0.1 } else { -0.1 };
            self.set_zoom(self.zoom + step, cx);
            return;
        }
        let rows = -(f32::from(delta.y) / f32::from(line) * 3.0).round() as i64;
        let cols = -(f32::from(delta.x) / f32::from(line)).round() as i64;
        let (rows, cols) = if event.modifiers.shift {
            (0, rows)
        } else {
            (rows, cols)
        };
        if rows != 0 || cols != 0 {
            self.top = self.top.offset(rows);
            self.left = self.left.offset(cols);
            self.viewport_changed(cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let Some(typed) = keystroke.key_char.as_ref() else {
            return;
        };
        if typed.chars().any(char::is_control) {
            return;
        }
        match &mut self.editor {
            Some(editor) => {
                editor.text.insert_str(editor.caret, typed);
                editor.caret += typed.len();
                editor.point = None;
                self.suggestion = 0;
            }
            None => {
                let pos = self.selection.active;
                self.editor = Some(Editor {
                    pos,
                    text: typed.clone(),
                    caret: typed.len(),
                    mode: EditMode::Enter,
                    point: None,
                });
            }
        }
        cx.emit(GridEvent::EditChanged);
        cx.stop_propagation();
        cx.notify();
    }

    // Only while the edited cell is on screen, so keys never act on a list nobody sees.
    fn suggestions(&self) -> Vec<&'static str> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        if self.col_right(editor.pos.col).is_none() || self.row_bottom(editor.pos.row).is_none() {
            return Vec::new();
        }
        autocomplete::token_at(&editor.text, editor.caret)
            .map(|(_, prefix)| autocomplete::suggestions(prefix))
            .unwrap_or_default()
    }

    // Tab with the list open inserts the highlighted function and its "(", as in Excel.
    fn accept_suggestion(&mut self, cx: &mut Context<Self>) -> bool {
        let suggestions = self.suggestions();
        let index = self.suggestion.min(suggestions.len().saturating_sub(1));
        let Some(name) = suggestions.get(index).copied() else {
            return false;
        };
        let Some(editor) = &mut self.editor else {
            return false;
        };
        let Some((start, _)) = autocomplete::token_at(&editor.text, editor.caret) else {
            return false;
        };
        let inserted = format!("{name}(");
        editor.text.replace_range(start..editor.caret, &inserted);
        editor.caret = start + inserted.len();
        editor.point = None;
        self.suggestion = 0;
        cx.emit(GridEvent::EditChanged);
        cx.notify();
        true
    }

    fn render_suggestions(&self, cx: &App) -> Option<impl IntoElement> {
        let suggestions = self.suggestions();
        let editor = self.editor.as_ref()?;
        if suggestions.is_empty() {
            return None;
        }
        let right = self.col_right(editor.pos.col)?;
        let bottom = self.row_bottom(editor.pos.row)?;
        let left = right - self.layout.col_width(editor.pos.col);
        let theme = cx.theme();
        let highlighted = self.suggestion.min(suggestions.len() - 1);
        Some(
            div()
                .absolute()
                .left(px(left * self.zoom))
                .top(px(bottom * self.zoom + 2.0))
                .min_w(px(160.0))
                .py_1()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_md()
                .text_sm()
                .text_color(theme.popover_foreground)
                .children(suggestions.into_iter().enumerate().map(|(index, name)| {
                    div()
                        .px_2()
                        .when(index == highlighted, |row| {
                            row.bg(theme.accent).text_color(theme.accent_foreground)
                        })
                        .child(name)
                })),
        )
    }

    pub fn merge_at(&self, pos: CellPos) -> Option<Range> {
        self.merges.iter().find(|m| m.contains(pos)).copied()
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = Selection {
            active: CellPos::new(RowIdx::default(), ColIdx::default()),
            corner: CellPos::new(RowIdx::LAST, ColIdx::LAST),
        };
        cx.emit(GridEvent::SelectionChanged);
        cx.notify();
    }

    fn after_commit_move(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let range = self.selection.range();
        if range.cell_count() > 1 && range.contains(self.selection.active) {
            let next = cycle_within(range, self.selection.active, direction);
            self.selection.active = next;
            self.scroll_into_view(next, cx);
            cx.emit(GridEvent::SelectionChanged);
            cx.notify();
        } else {
            self.move_active(direction, cx);
        }
    }
}

enum Hit {
    Corner,
    RowHeader(RowIdx),
    ColHeader(ColIdx),
    Cell(CellPos),
}

pub fn step(pos: CellPos, direction: Direction, amount: i64) -> CellPos {
    match direction {
        Direction::Up => CellPos::new(pos.row.offset(-amount), pos.col),
        Direction::Down => CellPos::new(pos.row.offset(amount), pos.col),
        Direction::Left => CellPos::new(pos.row, pos.col.offset(-amount)),
        Direction::Right => CellPos::new(pos.row, pos.col.offset(amount)),
    }
}

// Enter and Tab walk inside a multi-cell selection and wrap, as Excel does.
fn cycle_within(range: Range, pos: CellPos, direction: Direction) -> CellPos {
    let (r0, r1) = (range.start.row.get(), range.end.row.get());
    let (c0, c1) = (range.start.col.get(), range.end.col.get());
    let (mut r, mut c) = (pos.row.get(), pos.col.get());
    match direction {
        Direction::Down => {
            if r < r1 {
                r += 1;
            } else {
                r = r0;
                c = if c < c1 { c + 1 } else { c0 };
            }
        }
        Direction::Right => {
            if c < c1 {
                c += 1;
            } else {
                c = c0;
                r = if r < r1 { r + 1 } else { r0 };
            }
        }
        Direction::Up => {
            if r > r0 {
                r -= 1;
            } else {
                r = r1;
                c = if c > c0 { c - 1 } else { c1 };
            }
        }
        Direction::Left => {
            if c > c0 {
                c -= 1;
            } else {
                c = c1;
                r = if r > r0 { r - 1 } else { r1 };
            }
        }
    }
    CellPos::new(RowIdx::clamped(i64::from(r)), ColIdx::clamped(i64::from(c)))
}

fn prev_boundary(text: &str, caret: usize) -> usize {
    text[..caret]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_boundary(text: &str, caret: usize) -> usize {
    text[caret..]
        .chars()
        .next()
        .map_or(caret, |c| caret + c.len_utf8())
}

impl Render for Grid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = Frame {
            layout: self.layout.clone(),
            cells: self.cells.clone(),
            top: self.top,
            left: self.left,
            rows: self.visible_rows,
            cols: self.visible_cols,
            selection: self.selection.range(),
            active: self.selection.active,
            editor: self.editor.clone(),
            marquee: self.marquee,
            fill_target: self.fill_target,
            zoom: self.zoom,
            focused: self.focus.is_focused(window),
            colors: paint::Colors::from_theme(cx),
            row_header: self.row_header(),
            frozen_rows: self.frozen_rows,
            frozen_cols: self.frozen_cols,
            merges: self.merges.clone(),
            font: cx.theme().font_family.clone(),
            formula_refs: self
                .editor
                .as_ref()
                .map(|editor| formula_refs::references(&editor.text))
                .unwrap_or_default(),
        };
        let weak = cx.entity().downgrade();
        let last_paint = self.last_paint.clone();
        let active = self.selection.active;
        let active_text = self
            .cells
            .get(&active)
            .map(|cell| cell.text.clone())
            .unwrap_or_default();
        let active_label = if active_text.is_empty() {
            format!("{active}, blank")
        } else {
            format!("{active}, {active_text}")
        };
        div()
            .id("grid")
            .role(Role::Grid)
            .aria_label("Sheet")
            .key_context("Grid")
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .on_action(cx.listener(|g, _: &MoveUp, _, cx| g.navigate(Direction::Up, false, cx)))
            .on_action(cx.listener(|g, _: &MoveDown, _, cx| g.navigate(Direction::Down, false, cx)))
            .on_action(cx.listener(|g, _: &MoveLeft, _, cx| g.navigate(Direction::Left, false, cx)))
            .on_action(
                cx.listener(|g, _: &MoveRight, _, cx| g.navigate(Direction::Right, false, cx)),
            )
            .on_action(cx.listener(|g, _: &SelectUp, _, cx| g.navigate(Direction::Up, true, cx)))
            .on_action(
                cx.listener(|g, _: &SelectDown, _, cx| g.navigate(Direction::Down, true, cx)),
            )
            .on_action(
                cx.listener(|g, _: &SelectLeft, _, cx| g.navigate(Direction::Left, true, cx)),
            )
            .on_action(
                cx.listener(|g, _: &SelectRight, _, cx| g.navigate(Direction::Right, true, cx)),
            )
            .on_action(cx.listener(|g, _: &JumpUp, _, cx| g.jump(Direction::Up, false, cx)))
            .on_action(cx.listener(|g, _: &JumpDown, _, cx| g.jump(Direction::Down, false, cx)))
            .on_action(cx.listener(|g, _: &JumpLeft, _, cx| g.jump(Direction::Left, false, cx)))
            .on_action(cx.listener(|g, _: &JumpRight, _, cx| g.jump(Direction::Right, false, cx)))
            .on_action(cx.listener(|g, _: &JumpSelectUp, _, cx| g.jump(Direction::Up, true, cx)))
            .on_action(
                cx.listener(|g, _: &JumpSelectDown, _, cx| g.jump(Direction::Down, true, cx)),
            )
            .on_action(
                cx.listener(|g, _: &JumpSelectLeft, _, cx| g.jump(Direction::Left, true, cx)),
            )
            .on_action(
                cx.listener(|g, _: &JumpSelectRight, _, cx| g.jump(Direction::Right, true, cx)),
            )
            .on_action(cx.listener(|g, _: &RowStart, _, cx| g.row_start(cx)))
            .on_action(cx.listener(|g, _: &SheetStart, _, cx| {
                let origin = CellPos::default();
                g.select(origin, origin, cx);
            }))
            .on_action(cx.listener(|g, _: &SheetEnd, _, cx| {
                cx.emit(GridEvent::EndRequested { extend: false });
                g.dragging = false;
            }))
            .on_action(cx.listener(|g, _: &PageDown, _, cx| g.page(true, cx)))
            .on_action(cx.listener(|g, _: &PageUp, _, cx| g.page(false, cx)))
            .on_action(cx.listener(|g, _: &SelectAll, _, cx| g.select_all(cx)))
            .on_action(cx.listener(|g, _: &SelectColumn, _, cx| {
                let col = g.selection.active.col;
                g.selection = Selection {
                    active: CellPos::new(RowIdx::default(), col),
                    corner: CellPos::new(RowIdx::LAST, g.selection.corner.col),
                };
                cx.emit(GridEvent::SelectionChanged);
                cx.notify();
            }))
            .on_action(cx.listener(|g, _: &SelectRow, _, cx| {
                let row = g.selection.active.row;
                g.selection = Selection {
                    active: CellPos::new(row, ColIdx::default()),
                    corner: CellPos::new(g.selection.corner.row, ColIdx::LAST),
                };
                cx.emit(GridEvent::SelectionChanged);
                cx.notify();
            }))
            .on_action(cx.listener(|g, _: &ConfirmInSelection, _, cx| {
                // Excel's Ctrl+Enter: the entry goes into every selected cell and the
                // selection stays.
                let Some(editor) = g.editor.take() else {
                    return;
                };
                g.tab_start = None;
                cx.emit(GridEvent::CommitToSelection {
                    pos: editor.pos,
                    text: editor.text,
                    range: g.selection.range(),
                });
                cx.emit(GridEvent::EditChanged);
                cx.notify();
            }))
            .on_action(cx.listener(|g, _: &InsertLineBreak, _, cx| {
                // Alt+Enter starts a new line inside the cell being edited.
                if let Some(editor) = &mut g.editor {
                    editor.text.insert(editor.caret, '\n');
                    editor.caret += 1;
                    editor.point = None;
                    cx.emit(GridEvent::EditChanged);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|g, _: &CycleReference, _, cx| {
                // Outside the editor F4 is Excel's "repeat", which the app handles.
                let Some(editor) = &mut g.editor else {
                    cx.propagate();
                    return;
                };
                if let Some((text, caret)) =
                    formula_refs::cycle_reference(&editor.text, editor.caret)
                {
                    editor.text = text;
                    editor.caret = caret;
                    editor.point = None;
                    cx.emit(GridEvent::EditChanged);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|g, _: &EditCell, _, cx| match &mut g.editor {
                Some(editor) => {
                    editor.mode = match editor.mode {
                        EditMode::Enter => EditMode::Edit,
                        EditMode::Edit => EditMode::Enter,
                    };
                    cx.notify();
                }
                None => cx.emit(GridEvent::EditRequested(g.selection.active)),
            }))
            .on_action(cx.listener(|g, _: &ConfirmDown, _, cx| g.confirm(Direction::Down, cx)))
            .on_action(cx.listener(|g, _: &ConfirmUp, _, cx| g.confirm(Direction::Up, cx)))
            .on_action(cx.listener(|g, _: &ConfirmRight, _, cx| g.confirm(Direction::Right, cx)))
            .on_action(cx.listener(|g, _: &ConfirmLeft, _, cx| g.confirm(Direction::Left, cx)))
            .on_action(cx.listener(|g, _: &Cancel, _, cx| {
                g.fill_target = None;
                g.cancel_edit(cx);
                g.set_marquee(None, cx);
            }))
            .on_action(
                cx.listener(|g, _: &DeleteForward, _, cx| match &mut g.editor {
                    Some(editor) => {
                        let end = next_boundary(&editor.text, editor.caret);
                        editor.text.replace_range(editor.caret..end, "");
                        editor.point = None;
                        cx.emit(GridEvent::EditChanged);
                        cx.notify();
                    }
                    None => cx.emit(GridEvent::ClearRequested(g.selection.range())),
                }),
            )
            .on_action(cx.listener(|g, _: &DeleteBackward, _, cx| {
                match &mut g.editor {
                    Some(editor) => {
                        let start = prev_boundary(&editor.text, editor.caret);
                        editor.text.replace_range(start..editor.caret, "");
                        editor.caret = start;
                        editor.point = None;
                    }
                    None => {
                        let pos = g.selection.active;
                        g.editor = Some(Editor {
                            pos,
                            text: String::new(),
                            caret: 0,
                            mode: EditMode::Enter,
                            point: None,
                        });
                    }
                }
                cx.emit(GridEvent::EditChanged);
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|g, _: &MouseUpEvent, _, cx| {
                    g.dragging = false;
                    g.finish_edge_drag(cx);
                    g.finish_fill_drag(cx);
                }),
            )
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .when(self.fill_hover || self.fill_target.is_some(), |grid| {
                grid.cursor(CursorStyle::Crosshair)
            })
            .when_some(
                self.edge_drag.map(|drag| drag.edge).or(self.resize_hover),
                |grid, edge| {
                    grid.cursor(match edge {
                        Edge::Column(_) => CursorStyle::ResizeLeftRight,
                        Edge::Row(_) => CursorStyle::ResizeUpDown,
                    })
                },
            )
            .child(
                div()
                    .id((
                        "active-cell",
                        active.row.get() as u64 * 16_384 + u64::from(active.col.get()),
                    ))
                    .role(Role::Cell)
                    .aria_label(active_label)
                    .aria_description(self.active_formula.clone())
                    .aria_selected(true)
                    .absolute()
                    .size_0(),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let weak = weak.clone();
                        cx.defer(move |cx| {
                            if let Err(error) = weak.update(cx, |g, cx| g.set_bounds(bounds, cx)) {
                                tracing::debug!(%error, "grid dropped before layout");
                            }
                        });
                        bounds
                    },
                    move |bounds, _, window, cx| {
                        let started = Instant::now();
                        paint::paint(&frame, bounds, window, cx);
                        last_paint.set(started.elapsed());
                    },
                )
                .size_full(),
            )
            .children(self.render_suggestions(cx))
    }
}

impl Grid {
    // Excel returns Enter to the column where a row of Tab entries began.
    fn confirm(&mut self, direction: Direction, cx: &mut Context<Self>) {
        if direction == Direction::Right && self.accept_suggestion(cx) {
            return;
        }
        self.commit_edit(cx);
        let single = self.selection.range().cell_count() == 1;
        let active = self.selection.active;
        match (direction, self.tab_start) {
            (Direction::Right, _) if single => {
                let start = self.tab_start.unwrap_or(active.col);
                self.after_commit_move(direction, cx);
                self.tab_start = Some(start);
            }
            (Direction::Down, Some(col)) if single => {
                let pos = CellPos::new(active.row.offset(1), col);
                self.select(pos, pos, cx);
            }
            _ => {
                self.tab_start = None;
                self.after_commit_move(direction, cx);
            }
        }
    }

    fn jump(&mut self, direction: Direction, extend: bool, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            self.commit_edit(cx);
        }
        cx.emit(GridEvent::Jump { direction, extend });
    }

    fn row_start(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.caret = 0;
            cx.notify();
            return;
        }
        let pos = CellPos::new(self.selection.active.row, ColIdx::default());
        self.select(pos, pos, cx);
    }
}

// One extra screen of rows above and below the viewport, so a short scroll while the
// workbook is busy still lands on cached values.
fn cached_ranges(
    origin: CellPos,
    visible_rows: u32,
    visible_cols: u16,
    frozen_rows: u32,
    frozen_cols: u16,
) -> Vec<Range> {
    let first_row = origin
        .row
        .offset(-i64::from(visible_rows))
        .max(RowIdx::clamped(i64::from(frozen_rows)));
    let last_row = origin.row.offset(2 * i64::from(visible_rows));
    let end_col = origin.col.offset(i64::from(visible_cols));
    let zero = CellPos::default();
    let mut ranges = vec![Range::new(
        CellPos::new(first_row, origin.col),
        CellPos::new(last_row, end_col),
    )];
    if frozen_rows > 0 {
        let last = RowIdx::clamped(i64::from(frozen_rows) - 1);
        ranges.push(Range::new(
            CellPos::new(zero.row, origin.col),
            CellPos::new(last, end_col),
        ));
    }
    if frozen_cols > 0 {
        let last = ColIdx::clamped(i64::from(frozen_cols) - 1);
        ranges.push(Range::new(
            CellPos::new(first_row, zero.col),
            CellPos::new(last_row, last),
        ));
    }
    if frozen_rows > 0 && frozen_cols > 0 {
        ranges.push(Range::new(
            zero,
            CellPos::new(
                RowIdx::clamped(i64::from(frozen_rows) - 1),
                ColIdx::clamped(i64::from(frozen_cols) - 1),
            ),
        ));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::{Direction, cached_ranges, cycle_within, next_boundary, prev_boundary};
    use zenkai_types::{CellPos, Range};

    fn pos(text: &str) -> CellPos {
        CellPos::parse_a1(text).unwrap()
    }

    #[test]
    fn enter_wraps_inside_selection() {
        let range = Range::new(pos("A1"), pos("B2"));
        assert_eq!(cycle_within(range, pos("A1"), Direction::Down), pos("A2"));
        assert_eq!(cycle_within(range, pos("A2"), Direction::Down), pos("B1"));
        assert_eq!(cycle_within(range, pos("B2"), Direction::Down), pos("A1"));
        assert_eq!(cycle_within(range, pos("B1"), Direction::Right), pos("A2"));
    }

    #[test]
    fn caret_moves_by_whole_characters() {
        let text = "añb";
        assert_eq!(next_boundary(text, 1), 3);
        assert_eq!(prev_boundary(text, 3), 1);
        assert_eq!(prev_boundary(text, 0), 0);
        assert_eq!(next_boundary(text, text.len()), text.len());
    }

    #[test]
    fn cache_covers_a_screen_above_and_below() {
        let ranges = cached_ranges(pos("C101"), 40, 10, 0, 0);
        assert_eq!(ranges, vec![Range::new(pos("C61"), pos("M181"))]);
    }

    #[test]
    fn cache_stops_at_the_first_row_and_below_the_frozen_rows() {
        assert_eq!(
            cached_ranges(pos("A11"), 40, 10, 0, 0),
            vec![Range::new(pos("A1"), pos("K91"))]
        );
        let ranges = cached_ranges(pos("A11"), 5, 10, 8, 0);
        assert_eq!(ranges[0], Range::new(pos("A9"), pos("K21")));
    }

    #[test]
    fn cache_keeps_the_frozen_panes() {
        let ranges = cached_ranges(pos("C101"), 40, 10, 2, 2);
        assert_eq!(ranges.len(), 4);
        assert!(ranges.contains(&Range::new(pos("C1"), pos("M2"))));
        assert!(ranges.contains(&Range::new(pos("A61"), pos("B181"))));
        assert!(ranges.contains(&Range::new(pos("A1"), pos("B2"))));
    }
}
