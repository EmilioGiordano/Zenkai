use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::popover::Popover;
use gpui_kit::*;
use zenkai_agent::preferences::InterfaceScale;
use zenkai_i18n::t;
use zenkai_types::{CellStyle, HAlign};

use crate::actions::*;
use crate::interface_scale::{self, rem_px};
use crate::keymap;

fn tool(
    cx: &App,
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(keymap::labeled(cx, tooltip, &action))
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn separator(cx: &App) -> Div {
    div()
        .w(px(1.0))
        .h(rem_px(18.0))
        .mx_1()
        .bg(cx.theme().border)
}

fn shortcut_row(label: &'static str, keys: String, muted: Hsla) -> impl IntoElement {
    h_flex()
        .justify_between()
        .gap_4()
        .child(label)
        .child(div().text_color(muted).child(keys))
}

fn keys_of(cx: &App, action: &dyn Action) -> String {
    keymap::shortcut(cx, action).unwrap_or_default()
}

// Shown only while the interface is not at 100%, so the state is visible and one click away
// from the controls that change it.
fn interface_size(cx: &App) -> Option<impl IntoElement> {
    let scale = interface_scale::shown(cx);
    if scale == InterfaceScale::default() {
        return None;
    }
    let percent = scale.percent();
    let muted = cx.theme().muted_foreground;
    let smaller_keys = keys_of(cx, &InterfaceSmaller);
    let larger_keys = keys_of(cx, &InterfaceLarger);
    let reset_keys = keys_of(cx, &InterfaceReset);
    Some(
        Popover::new("interface-size")
            .anchor(Anchor::TopRight)
            .trigger(
                Button::new("interface-size-trigger")
                    .ghost()
                    .compact()
                    .label(t!("interface.pill", percent = percent)),
            )
            .content(move |_, _, _| {
                v_flex()
                    .w(rem_px(280.0))
                    .gap_2()
                    .text_sm()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(t!("interface.title"))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Button::new("interface-smaller")
                                            .compact()
                                            .icon(IconName::Minus)
                                            .tooltip(t!("interface.smaller"))
                                            .on_click(|_, window, cx| {
                                                window.dispatch_action(
                                                    InterfaceSmaller.boxed_clone(),
                                                    cx,
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .w(rem_px(44.0))
                                            .text_center()
                                            .child(format!("{percent}%")),
                                    )
                                    .child(
                                        Button::new("interface-larger")
                                            .compact()
                                            .icon(IconName::Plus)
                                            .tooltip(t!("interface.larger"))
                                            .on_click(|_, window, cx| {
                                                window.dispatch_action(
                                                    InterfaceLarger.boxed_clone(),
                                                    cx,
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .child(shortcut_row(
                        t!("interface.smaller"),
                        smaller_keys.clone(),
                        muted,
                    ))
                    .child(shortcut_row(
                        t!("interface.larger"),
                        larger_keys.clone(),
                        muted,
                    ))
                    .child(shortcut_row(
                        t!("interface.reset"),
                        reset_keys.clone(),
                        muted,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("interface.note")),
                    )
            }),
    )
}

pub struct ColorPickers {
    pub font: Entity<ColorPickerState>,
    pub fill: Entity<ColorPickerState>,
}

pub fn render(style: &CellStyle, colors: &ColorPickers, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .h(rem_px(36.0))
        .px_2()
        .gap_0p5()
        .items_center()
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.border)
        .child(tool(
            cx,
            "new",
            IconName::FilePlus,
            t!("toolbar.new"),
            NewWorkbook,
        ))
        .child(tool(
            cx,
            "open",
            IconName::FolderOpen,
            t!("toolbar.open"),
            Open,
        ))
        .child(tool(cx, "save", IconName::Save, t!("toolbar.save"), Save))
        .child(separator(cx))
        .child(tool(cx, "undo", IconName::Undo2, t!("toolbar.undo"), Undo))
        .child(tool(cx, "redo", IconName::Redo2, t!("toolbar.redo"), Redo))
        .child(separator(cx))
        .child(
            tool(cx, "bold", IconName::Bold, t!("toolbar.bold"), ToggleBold).selected(style.bold),
        )
        .child(
            tool(
                cx,
                "italic",
                IconName::Italic,
                t!("toolbar.italic"),
                ToggleItalic,
            )
            .selected(style.italic),
        )
        .child(
            tool(
                cx,
                "underline",
                IconName::Underline,
                t!("toolbar.underline"),
                ToggleUnderline,
            )
            .selected(style.underline),
        )
        .child(separator(cx))
        .child(
            ColorPicker::new(&colors.font)
                .icon(IconName::Baseline)
                .accessibility_label(t!("toolbar.font_color"))
                .small(),
        )
        .child(
            ColorPicker::new(&colors.fill)
                .icon(IconName::PaintBucket)
                .accessibility_label(t!("toolbar.fill_color"))
                .small(),
        )
        .child(
            Button::new("borders")
                .ghost()
                .compact()
                .icon(IconName::Grid2x2)
                .tooltip(t!("toolbar.borders"))
                .dropdown_menu(|menu, _, _| {
                    menu.menu(t!("toolbar.border_bottom"), Box::new(BorderBottom))
                        .menu(t!("toolbar.borders_all"), Box::new(BordersAll))
                        .menu(t!("toolbar.borders_outside"), Box::new(BordersOutside))
                        .menu(t!("toolbar.borders_none"), Box::new(BordersNone))
                }),
        )
        .child(separator(cx))
        .child(
            tool(
                cx,
                "align-left",
                IconName::TextAlignStart,
                t!("toolbar.align_left"),
                AlignLeft,
            )
            .selected(style.align == HAlign::Left),
        )
        .child(
            tool(
                cx,
                "align-center",
                IconName::TextAlignCenter,
                t!("toolbar.align_center"),
                AlignCenter,
            )
            .selected(style.align == HAlign::Center),
        )
        .child(
            tool(
                cx,
                "align-right",
                IconName::TextAlignEnd,
                t!("toolbar.align_right"),
                AlignRight,
            )
            .selected(style.align == HAlign::Right),
        )
        .child(
            tool(
                cx,
                "wrap",
                IconName::TextWrap,
                t!("toolbar.wrap_text"),
                ToggleWrapText,
            )
            .selected(style.wrap),
        )
        .child(separator(cx))
        .child(tool(
            cx,
            "general",
            IconName::CaseSensitive,
            t!("toolbar.format_general"),
            FormatGeneral,
        ))
        .child(tool(
            cx,
            "more-decimals",
            IconName::DecimalsArrowRight,
            t!("toolbar.increase_decimal"),
            IncreaseDecimal,
        ))
        .child(tool(
            cx,
            "fewer-decimals",
            IconName::DecimalsArrowLeft,
            t!("toolbar.decrease_decimal"),
            DecreaseDecimal,
        ))
        .child(tool(
            cx,
            "number",
            IconName::Hash,
            t!("toolbar.format_number"),
            FormatNumber,
        ))
        .child(tool(
            cx,
            "currency",
            IconName::DollarSign,
            t!("toolbar.format_currency"),
            FormatCurrency,
        ))
        .child(tool(
            cx,
            "percent",
            IconName::Percent,
            t!("toolbar.format_percent"),
            FormatPercent,
        ))
        .child(tool(
            cx,
            "date",
            IconName::Calendar,
            t!("toolbar.format_date"),
            FormatDate,
        ))
        .child(separator(cx))
        .child(tool(
            cx,
            "chart",
            IconName::ChartColumn,
            t!("toolbar.chart"),
            InsertChart,
        ))
        .child(separator(cx))
        .child(tool(
            cx,
            "zoom-out",
            IconName::ZoomOut,
            t!("toolbar.zoom_out"),
            ZoomOut,
        ))
        .child(tool(
            cx,
            "zoom-in",
            IconName::ZoomIn,
            t!("toolbar.zoom_in"),
            ZoomIn,
        ))
        .child(div().flex_1())
        .children(interface_size(cx))
        .child(tool(
            cx,
            "theme",
            IconName::SunMoon,
            t!("toolbar.theme"),
            ToggleTheme,
        ))
}
