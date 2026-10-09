use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use crate::actions::*;
use crate::panel_width::{Direction, Panel, PanelWidths, Resize, Shown};

const EDGE_HIT_AREA: f32 = 6.0;

struct Drag {
    panel: Panel,
    start_x: f32,
    start_width: f32,
}

pub(super) struct PanelsState {
    pub widths: PanelWidths,
    dragging: Option<Drag>,
    sidebar_edge: FocusHandle,
    chat_edge: FocusHandle,
}

impl PanelsState {
    pub fn new(cx: &mut App) -> PanelsState {
        PanelsState {
            widths: PanelWidths::default(),
            dragging: None,
            sidebar_edge: cx.focus_handle(),
            chat_edge: cx.focus_handle(),
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
            chat: self.chat_open(),
        }
    }

    pub(super) fn panel_width(&self, panel: Panel, window: &Window) -> f32 {
        let window_width = f32::from(window.viewport_size().width);
        self.panels
            .widths
            .fitted(window_width, self.shown_panels())
            .width_of(panel)
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
        self.change_panel_widths(
            |widths, window_width, shown| widths.stepped(panel, resize, window_width, shown),
            window,
            cx,
        );
        self.persist_session(cx);
    }

    fn restore_panel_width(&mut self, panel: Panel, window: &Window, cx: &mut Context<Self>) {
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
        let focused = focus.is_focused(window);
        let active = self
            .panels
            .dragging
            .as_ref()
            .is_some_and(|drag| drag.panel == panel);
        let (min, max) = panel.limits();
        let (id, label) = match panel {
            Panel::Sidebar => ("sidebar-edge", t!("panel.edge.sidebar")),
            Panel::Chat => ("chat-edge", t!("panel.edge.chat")),
        };
        let ring = cx.theme().ring;
        let mouse_focus = focus.clone();
        div()
            .id(id)
            .key_context("PanelEdge")
            .track_focus(&focus)
            .tab_stop(true)
            .role(Role::Splitter)
            .aria_label(label)
            .aria_orientation(Orientation::Vertical)
            .aria_numeric_value(f64::from(self.panel_width(panel, window)))
            .aria_min_numeric_value(f64::from(min))
            .aria_max_numeric_value(f64::from(max))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(EDGE_HIT_AREA))
            .cursor_col_resize()
            .map(|edge| match panel {
                Panel::Sidebar => edge.right_0().border_r_1(),
                Panel::Chat => edge.left_0().border_l_1(),
            })
            .border_color(if active || focused {
                ring
            } else {
                gpui_kit::transparent_black()
            })
            .when(focused, |edge| edge.bg(ring.opacity(0.3)))
            .hover(|edge| edge.border_color(ring))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&mouse_focus, cx);
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
