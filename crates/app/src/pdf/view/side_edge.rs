use gpui_kit::*;
use zenkai_i18n::t;

use super::{PdfEvent, PdfView};
use crate::actions::{LeavePanelEdge, MoveEdgeLeft, MoveEdgeRight, RestorePanelWidth};
use crate::panel_edge::{EdgeSide, EdgeState, panel_edge};
use crate::panel_width::Resize;
use crate::pdf::side_width;

pub(super) struct SideDrag {
    start_x: f32,
    start_width: f32,
}

impl PdfView {
    fn view_width(&self) -> f32 {
        let pages = f32::from(self.bounds.size.width);
        if pages <= 0.0 {
            f32::INFINITY
        } else {
            pages + self.drawn_side
        }
    }

    pub(super) fn side_width(&self) -> f32 {
        side_width::within(self.side_width, self.view_width())
    }

    pub fn set_side_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = side_width::stored(width);
        if width != self.side_width {
            self.side_width = width;
            self.settled_side = Some(self.side_width());
            cx.notify();
        }
    }

    fn settle_side(&mut self, cx: &mut Context<Self>) {
        self.settled_side = Some(self.side_width());
        cx.emit(PdfEvent::SideResized(self.side_width));
        if std::mem::take(&mut self.layout_stale) && self.is_ready() {
            self.relayout();
            self.request_visible();
        }
        cx.notify();
    }

    pub fn step_side(&mut self, resize: Resize, cx: &mut Context<Self>) {
        self.side_drag = None;
        self.side_width = side_width::stepped(self.side_width(), resize, self.view_width());
        self.settle_side(cx);
    }

    fn restore_side(&mut self, cx: &mut Context<Self>) {
        self.side_drag = None;
        self.side_width = side_width::DEFAULT;
        self.settle_side(cx);
    }

    fn begin_side_drag(&mut self, x: f32) {
        self.side_drag = Some(SideDrag {
            start_x: x,
            start_width: self.side_width(),
        });
    }

    pub(super) fn drag_side(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &self.side_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_side_drag(cx);
            return;
        }
        let requested = drag.start_width + f32::from(event.position.x) - drag.start_x;
        self.side_width = side_width::within(requested, self.view_width());
        cx.notify();
    }

    pub(super) fn end_side_drag(&mut self, cx: &mut Context<Self>) {
        if self.side_drag.take().is_some() {
            self.settle_side(cx);
        }
    }

    pub(super) fn resizing_side(&self) -> bool {
        self.side_drag.is_some()
    }

    fn leave_side_edge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub(super) fn render_side_edge(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let state = EdgeState {
            id: "pdf-side-edge",
            label: t!("panel.edge.pdf"),
            side: EdgeSide::Right,
            width: self.side_width(),
            limits: side_width::limits(self.view_width()),
            dragging: self.side_drag.is_some(),
        };
        panel_edge(state, &self.side_edge_focus, window, cx)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.leave_side_edge(window, cx);
                    if event.click_count >= 2 {
                        this.restore_side(cx);
                    } else {
                        this.begin_side_drag(f32::from(event.position.x));
                    }
                    cx.notify();
                }),
            )
            .on_action(
                cx.listener(|this, _: &MoveEdgeLeft, _, cx| this.step_side(Resize::Narrower, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveEdgeRight, _, cx| this.step_side(Resize::Wider, cx)),
            )
            .on_action(cx.listener(|this, _: &RestorePanelWidth, _, cx| this.restore_side(cx)))
            .on_action(
                cx.listener(|this, _: &LeavePanelEdge, window, cx| {
                    this.leave_side_edge(window, cx)
                }),
            )
    }
}
