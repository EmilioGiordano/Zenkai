use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const EDGE_HIT_AREA: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EdgeSide {
    Left,
    Right,
}

pub struct EdgeState {
    pub id: &'static str,
    pub label: &'static str,
    pub side: EdgeSide,
    pub width: f32,
    pub limits: (f32, f32),
    pub dragging: bool,
}

pub fn panel_edge(
    state: EdgeState,
    focus: &FocusHandle,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let focused = focus.is_focused(window);
    let ring = cx.theme().ring;
    div()
        .id(state.id)
        .key_context("PanelEdge")
        .track_focus(focus)
        .role(Role::Splitter)
        .aria_label(state.label)
        .aria_orientation(Orientation::Vertical)
        .aria_numeric_value(f64::from(state.width))
        .aria_min_numeric_value(f64::from(state.limits.0))
        .aria_max_numeric_value(f64::from(state.limits.1))
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(EDGE_HIT_AREA))
        .cursor_col_resize()
        .map(|edge| match state.side {
            EdgeSide::Right => edge.right_0().border_r_1(),
            EdgeSide::Left => edge.left_0().border_l_1(),
        })
        .border_color(if state.dragging || focused {
            ring
        } else {
            gpui_kit::transparent_black()
        })
        .when(focused, |edge| edge.bg(ring.opacity(0.3)))
        .hover(|edge| edge.border_color(ring))
}
