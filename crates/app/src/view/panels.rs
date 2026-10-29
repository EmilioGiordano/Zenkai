use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use crate::actions::*;
use crate::chat_layout::{ChatLayout, SideSplit};
use crate::panel_edge::{self, EdgeSide, EdgeState};
use crate::panel_width::{Direction, KEY_STEP, Panel, PanelWidths, Resize, Shown};

struct Drag {
    panel: Panel,
    start_x: f32,
    start_width: f32,
}

pub(super) struct PanelsState {
    pub widths: PanelWidths,
    side_split: SideSplit,
    dragging: Option<Drag>,
    sidebar_edge: FocusHandle,
    chat_edge: FocusHandle,
}

impl PanelsState {
    pub fn new(cx: &mut App) -> PanelsState {
        PanelsState {
            widths: PanelWidths::default(),
            side_split: SideSplit::default(),
            dragging: None,
            sidebar_edge: cx.focus_handle().tab_stop(true),
            chat_edge: cx.focus_handle().tab_stop(true),
        }
    }

    fn edge_focus(&self, panel: Panel) -> FocusHandle {
        match panel {
            Panel::Sidebar => self.sidebar_edge.clone(),
            Panel::Chat => self.chat_edge.clone(),
        }
    }
}

impl Workspace {
    fn shown_panels(&self) -> Shown {
        Shown {
            sidebar: self.sidebar.visible,
            chat: self.chat_open() && self.chat_layout() == ChatLayout::Docked,
        }
    }

    pub(super) fn panel_width(&self, panel: Panel, window: &Window) -> f32 {
        let window_width = f32::from(window.viewport_size().width);
        match (panel, self.chat_layout()) {
            (Panel::Chat, ChatLayout::SideBySide) => self
                .panels
                .side_split
                .chat_width(self.side_by_side_room(window)),
            (Panel::Chat, ChatLayout::FullWindow) => window_width,
            _ => self
                .panels
                .widths
                .fitted(window_width, self.shown_panels())
                .width_of(panel),
        }
    }

    fn side_by_side_room(&self, window: &Window) -> f32 {
        let sidebar = if self.sidebar.visible {
            self.panel_width(Panel::Sidebar, window)
        } else {
            0.0
        };
        f32::from(window.viewport_size().width) - sidebar
    }

    fn splits_beside_sheet(&self, panel: Panel) -> bool {
        panel == Panel::Chat && self.chat_layout() == ChatLayout::SideBySide
    }

    fn resize_side_split(&mut self, requested: f32, window: &Window, cx: &mut Context<Self>) {
        let split = self
            .panels
            .side_split
            .with_chat_width(requested, self.side_by_side_room(window));
        if split != self.panels.side_split {
            self.panels.side_split = split;
            cx.notify();
        }
    }

    fn change_panel_widths(
        &mut self,
        change: impl FnOnce(PanelWidths, f32, Shown) -> PanelWidths,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let window_width = f32::from(window.viewport_size().width);
        let widths = change(self.panels.widths, window_width, self.shown_panels());
        if widths != self.panels.widths {
            self.panels.widths = widths;
            cx.notify();
        }
    }

    pub(super) fn step_panel(
        &mut self,
        panel: Panel,
        resize: Resize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.splits_beside_sheet(panel) {
            let current = self.panel_width(panel, window);
            let requested = match resize {
                Resize::Wider => current + KEY_STEP,
                Resize::Narrower => current - KEY_STEP,
            };
            self.resize_side_split(requested, window, cx);
            return;
        }
        self.change_panel_widths(
            |widths, window_width, shown| widths.stepped(panel, resize, window_width, shown),
            window,
            cx,
        );
        self.persist_session(cx);
    }

    fn restore_panel_width(&mut self, panel: Panel, window: &Window, cx: &mut Context<Self>) {
        if self.splits_beside_sheet(panel) {
            self.panels.side_split = SideSplit::default();
            cx.notify();
            return;
        }
        self.change_panel_widths(
            |widths, window_width, shown| widths.restored(panel, window_width, shown),
            window,
            cx,
        );
        self.persist_session(cx);
    }

    fn begin_panel_drag(&mut self, panel: Panel, x: f32, window: &Window) {
        self.panels.dragging = Some(Drag {
            panel,
            start_x: x,
            start_width: self.panel_width(panel, window),
        });
    }

    pub(super) fn drag_panel(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &self.panels.dragging else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_panel_drag(cx);
            return;
        }
        let moved = f32::from(event.position.x) - drag.start_x;
        let (panel, requested) = match drag.panel {
            Panel::Sidebar => (Panel::Sidebar, drag.start_width + moved),
            Panel::Chat => (Panel::Chat, drag.start_width - moved),
        };
        if self.splits_beside_sheet(panel) {
            self.resize_side_split(requested, window, cx);
            return;
        }
        self.change_panel_widths(
            |widths, window_width, shown| widths.resized(panel, requested, window_width, shown),
            window,
            cx,
        );
    }

    pub(super) fn end_panel_drag(&mut self, cx: &mut Context<Self>) {
        if self.panels.dragging.take().is_some() {
            self.persist_session(cx);
            cx.notify();
        }
    }

    pub(super) fn panels_dragging(&self) -> bool {
        self.panels.dragging.is_some()
    }

    fn leave_panel_edge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
    }

    pub(super) fn panel_edge(
        &self,
        panel: Panel,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let focus = self.panels.edge_focus(panel);
        let dragging = self
            .panels
            .dragging
            .as_ref()
            .is_some_and(|drag| drag.panel == panel);
        let limits = if self.splits_beside_sheet(panel) {
            SideSplit::limits(self.side_by_side_room(window))
        } else {
            panel.limits()
        };
        let (id, label, side) = match panel {
            Panel::Sidebar => ("sidebar-edge", t!("panel.edge.sidebar"), EdgeSide::Right),
            Panel::Chat => ("chat-edge", t!("panel.edge.chat"), EdgeSide::Left),
        };
        let state = EdgeState {
            id,
            label,
            side,
            width: self.panel_width(panel, window),
            limits,
            dragging,
        };
        panel_edge::panel_edge(state, &focus, window, cx)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.leave_panel_edge(window, cx);
                    if event.click_count >= 2 {
                        this.restore_panel_width(panel, window, cx);
                    } else {
                        this.begin_panel_drag(panel, f32::from(event.position.x), window);
                    }
                    cx.notify();
                }),
            )
            .on_action(cx.listener(move |this, _: &MoveEdgeLeft, window, cx| {
                this.step_panel(panel, panel.moving_edge(Direction::Left), window, cx)
            }))
            .on_action(cx.listener(move |this, _: &MoveEdgeRight, window, cx| {
                this.step_panel(panel, panel.moving_edge(Direction::Right), window, cx)
            }))
            .on_action(cx.listener(move |this, _: &RestorePanelWidth, window, cx| {
                this.restore_panel_width(panel, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &LeavePanelEdge, window, cx| {
                    this.leave_panel_edge(window, cx)
                }),
            )
    }
}
