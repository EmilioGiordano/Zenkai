use gpui_kit::assets::IconName;
use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::preferences::ColorMode;
use zenkai_grid::HighContrast;

use super::rows::RowInfo;

const NARROW_CONTROL: f32 = 170.0;

pub fn group_title(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .pl_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn card(rows: Vec<AnyElement>, cx: &App) -> Div {
    let contrast = cx.try_global::<HighContrast>().is_some_and(|high| high.0);
    v_flex()
        .rounded_xl()
        .bg(cx.theme().title_bar)
        .when(contrast, |this| {
            this.border_1().border_color(cx.theme().border)
        })
        .children(rows)
}

pub struct RowParts {
    pub modified: bool,
    pub first: bool,
    pub control: Option<AnyElement>,
    pub below: Option<AnyElement>,
    pub overlay: Option<AnyElement>,
    pub code: Option<String>,
}

pub fn row(
    id: usize,
    info: &RowInfo,
    parts: RowParts,
    on_reset: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let title = info.title;
    let heading = h_flex()
        .gap_2()
        .items_center()
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .when(parts.modified, |this| {
            this.child(
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(theme.warning)
                    .flex_shrink_0(),
            )
            .child(
                Button::new(("reset", id))
                    .ghost()
                    .compact()
                    .label("Reset")
                    .accessibility_label(format!("Reset {title} to its default"))
                    .on_click(move |_, window, cx| on_reset(window, cx)),
            )
        });
    let text = v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(heading)
        .when(!info.description.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(info.description),
            )
        })
        .children(parts.code.map(|code| {
            div()
                .mt_1()
                .font_family("monospace")
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(code)
        }));
    v_flex()
        .relative()
        .w_full()
        .gap_3()
        .px_4()
        .py_3p5()
        .when(!parts.first, |this| {
            this.border_t_1().border_color(theme.background)
        })
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(text)
                .children(parts.control),
        )
        .children(parts.below)
        .children(parts.overlay.map(|overlay| {
            deferred(
                div()
                    .absolute()
                    .top(px(52.0))
                    .right(px(16.0))
                    .w(px(380.0))
                    .occlude()
                    .shadow_lg()
                    .child(overlay),
            )
            .with_priority(1)
        }))
        .into_any_element()
}

pub fn switch(
    id: impl Into<ElementId>,
    label: &'static str,
    on: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Switch::new(id)
        .checked(on)
        .accessibility_label(label)
        .on_click(move |checked, window, cx| on_change(*checked, window, cx))
        .into_any_element()
}

pub fn select(
    id: impl Into<ElementId>,
    label: &'static str,
    value: impl Into<SharedString>,
    open: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id)
        .w(px(NARROW_CONTROL))
        .selected(open)
        .accessibility_label(format!("{label}: choose"))
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(div().flex_1().text_left().child(value.into()))
                .child(Icon::new(IconName::ChevronDown).size_3()),
        )
        .into_any_element()
}

pub fn stepper(
    id: &'static str,
    label: &'static str,
    value: impl Into<SharedString>,
    can_decrease: bool,
    can_increase: bool,
    on_step: impl Fn(bool, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let down = on_step.clone();
    h_flex()
        .items_center()
        .gap_1()
        .child(
            Button::new((id, 0usize))
                .ghost()
                .compact()
                .label("−")
                .accessibility_label(format!("Decrease {label}"))
                .disabled(!can_decrease)
                .on_click(move |_, window, cx| down(false, window, cx)),
        )
        .child(
            div()
                .min_w(px(52.0))
                .text_center()
                .font_family("monospace")
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(value.into()),
        )
        .child(
            Button::new((id, 1usize))
                .ghost()
                .compact()
                .label("+")
                .accessibility_label(format!("Increase {label}"))
                .disabled(!can_increase)
                .on_click(move |_, window, cx| on_step(true, window, cx)),
        )
        .into_any_element()
}

pub fn segmented<T: Copy + 'static>(
    id: &'static str,
    options: Vec<(T, &'static str, bool)>,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Div {
    h_flex()
        .self_start()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .children(
            options
                .into_iter()
                .enumerate()
                .map(|(index, (value, label, selected))| {
                    let on_pick = on_pick.clone();
                    Button::new((id, index))
                        .ghost()
                        .selected(selected)
                        .label(if selected {
                            format!("✓ {label}")
                        } else {
                            label.to_string()
                        })
                        .on_click(move |_, window, cx| on_pick(value, window, cx))
                }),
        )
}

pub fn explanation(text: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .items_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(IconName::Info).size_3p5())
        .child(text)
}

struct Half {
    page: u32,
    side: u32,
    line: u32,
}

const LIGHT_HALF: Half = Half {
    page: 0xff_ff_ff,
    side: 0xec_ec_ee,
    line: 0x55_58_5e,
};
const DARK_HALF: Half = Half {
    page: 0x18_19_1b,
    side: 0x26_27_2a,
    line: 0xa3_a5_ab,
};

fn half(shade: &Half) -> Div {
    h_flex()
        .flex_1()
        .h_full()
        .bg(rgb(shade.page))
        .child(div().w(relative(0.26)).h_full().bg(rgb(shade.side)))
        .child(
            v_flex()
                .flex_1()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(10.0))
                .child(
                    div()
                        .h(px(5.0))
                        .w(relative(0.7))
                        .rounded(px(3.0))
                        .bg(rgb(shade.line)),
                )
                .child(
                    div()
                        .h(px(5.0))
                        .w(relative(0.45))
                        .rounded(px(3.0))
                        .bg(rgb(shade.line))
                        .opacity(0.6),
                )
                .child(
                    div()
                        .h(px(10.0))
                        .w(relative(0.36))
                        .rounded(px(3.0))
                        .border_2()
                        .border_color(rgb(0x4c_af_7a)),
                ),
        )
}

pub fn mode_cards(
    selected: ColorMode,
    on_pick: impl Fn(ColorMode, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    h_flex().gap_3p5().children(ColorMode::ALL.map(|mode| {
        let on_pick = on_pick.clone();
        let chosen = selected == mode;
        let thumbnail = h_flex()
            .w(px(124.0))
            .h(px(78.0))
            .rounded_lg()
            .overflow_hidden()
            .border_2()
            .border_color(if chosen { theme.primary } else { theme.border })
            .map(|this| match mode {
                ColorMode::Light => this.child(half(&LIGHT_HALF)),
                ColorMode::Dark => this.child(half(&DARK_HALF)),
                ColorMode::System => this.child(half(&LIGHT_HALF)).child(half(&DARK_HALF)),
            });
        Button::new(("color-mode", mode as usize))
            .ghost()
            .selected(chosen)
            .h_auto()
            .p_1()
            .accessibility_label(mode.label())
            .on_click(move |_, window, cx| on_pick(mode, window, cx))
            .child(
                v_flex().items_center().gap_2().child(thumbnail).child(
                    div()
                        .font_weight(if chosen {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .child(if chosen {
                            format!("✓ {}", mode.label())
                        } else {
                            mode.label().to_string()
                        }),
                ),
            )
    }))
}
