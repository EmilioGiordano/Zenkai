use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use crate::space_appearance::{
    ApplyTo, Intensity, Look, MAX_INTENSITY, Opacity, Resolved, Rgba, SpaceStyle,
};
use crate::space_parts::{Contrast, Fill, parts};
use crate::space_settings;
use crate::spaces::SpaceColor;

const SAMPLE: u32 = 0x5b8def;

pub fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn paint_or(fill: Option<Fill>, fallback: Hsla) -> Hsla {
    fill.map_or(fallback, space_settings::paint)
}

// The sidebar's own rules draw the preview, so a card shows what the style will look like.
fn preview(look: Look, contrast: Contrast, cx: &App) -> Div {
    let theme = cx.theme();
    let shape = parts(
        &Resolved {
            look,
            tint: Some(Rgba::new(SAMPLE, Opacity::OPAQUE)),
        },
        contrast,
    );
    let (idle, ink, surface) = (theme.sidebar_accent, theme.muted_foreground, theme.sidebar);
    let clear = gpui_kit::transparent_black();
    let row = move || {
        div()
            .h(px(12.0))
            .rounded(px(4.0))
            .bg(paint_or(shape.row_fill, idle))
    };
    v_flex()
        .h(px(60.0))
        .p(px(5.0))
        .gap(px(4.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(paint_or(shape.outline, clear))
        .bg(paint_or(shape.container_fill, surface))
        .child(
            h_flex()
                .h(px(14.0))
                .px(px(5.0))
                .gap(px(5.0))
                .items_center()
                .rounded(px(4.0))
                .bg(paint_or(shape.header_fill, clear))
                .children(shape.dot.map(|dot| {
                    div()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(space_settings::paint(dot))
                }))
                .child(div().h(px(4.0)).w(px(28.0)).rounded(px(2.0)).bg(ink)),
        )
        .child(row())
        .child(row())
}

pub fn style_cards(
    prefix: &'static str,
    look: Look,
    on_select: impl Fn(SpaceStyle, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Div {
    let contrast = space_settings::contrast(cx);
    h_flex().gap_2().children(SpaceStyle::ALL.map(|style| {
        let selected = look.style == style;
        let on_select = on_select.clone();
        let name = if selected {
            format!("✓ {}", style.label())
        } else {
            style.label().to_string()
        };
        Button::new((prefix, style as usize))
            .ghost()
            .selected(selected)
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .h(px(150.0))
            .p_2()
            .on_click(move |_, window, cx| on_select(style, window, cx))
            .child(
                v_flex()
                    .size_full()
                    .gap_1p5()
                    .items_start()
                    .justify_start()
                    .child(preview(Look { style, ..look }, contrast, cx).w_full())
                    .child(div().font_weight(FontWeight::MEDIUM).child(name))
                    .child(
                        caption(style.description(), cx)
                            .w_full()
                            .whitespace_normal()
                            .text_left(),
                    ),
            )
    }))
}

pub fn swatch_color(color: SpaceColor, cx: &App) -> Hsla {
    color
        .rgb()
        .map_or(cx.theme().muted_foreground, |value| rgb(value).into())
}

pub fn swatch_dot(color: SpaceColor, selected: bool, cx: &App) -> Div {
    let ink = swatch_color(color, cx);
    let dot = div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .rounded_full()
        .border_1()
        .border_color(ink);
    let dot = match color.rgb() {
        Some(_) => dot.bg(ink),
        None => dot,
    };
    let mark = if color.rgb().is_some() {
        cx.theme().background
    } else {
        cx.theme().foreground
    };
    if selected {
        dot.child(Icon::new(IconName::Check).size_3().text_color(mark))
    } else {
        dot
    }
}

pub fn swatch(
    id: impl Into<ElementId>,
    color: SpaceColor,
    selected: bool,
    on_select: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .selected(selected)
        .tooltip(color.label())
        .accessibility_label(color.label())
        .on_click(move |_, window, cx| on_select(window, cx))
        .child(swatch_dot(color, selected, cx))
}

pub fn intensity_row(
    slider: &Entity<SliderState>,
    intensity: Intensity,
    on_step: impl Fn(bool, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Div {
    let down = on_step.clone();
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .justify_between()
                .child(caption("Intensity", cx))
                .child(div().text_xs().child(format!("{}%", intensity.percent()))),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("intensity-down")
                        .compact()
                        .label("−")
                        .accessibility_label("Less intensity")
                        .disabled(intensity.percent() == 0)
                        .on_click(move |_, window, cx| down(false, window, cx)),
                )
                .child(div().flex_1().child(Slider::new(slider)))
                .child(
                    Button::new("intensity-up")
                        .compact()
                        .label("+")
                        .accessibility_label("More intensity")
                        .disabled(intensity.percent() == MAX_INTENSITY)
                        .on_click(move |_, window, cx| on_step(true, window, cx)),
                ),
        )
}

pub fn apply_to_checkboxes(
    prefix: &'static str,
    apply_to: ApplyTo,
    on_change: impl Fn(ApplyTo, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Div {
    let header_change = on_change.clone();
    v_flex()
        .gap_2()
        .child(caption("Apply to", cx))
        .child(
            Checkbox::new((prefix, 0usize))
                .checked(apply_to.header())
                .label("Space header")
                .on_click(move |checked, window, cx| {
                    header_change(ApplyTo::of(*checked, apply_to.workbooks()), window, cx)
                }),
        )
        .child(
            Checkbox::new((prefix, 1usize))
                .checked(apply_to.workbooks())
                .label("Space workbooks")
                .on_click(move |checked, window, cx| {
                    on_change(ApplyTo::of(apply_to.header(), *checked), window, cx)
                }),
        )
}
