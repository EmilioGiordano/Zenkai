use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::{CellStyle, HAlign};

use crate::actions::*;

fn tool(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn separator(cx: &App) -> Div {
    div().w(px(1.0)).h(px(18.0)).mx_1().bg(cx.theme().border)
}

pub struct ColorPickers {
    pub font: Entity<ColorPickerState>,
    pub fill: Entity<ColorPickerState>,
}

pub fn render(style: &CellStyle, colors: &ColorPickers, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .h(px(36.0))
        .px_2()
        .gap_0p5()
        .items_center()
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.border)
        .child(tool(
            "new",
            IconName::FilePlus,
            t!("toolbar.new"),
            NewWorkbook,
        ))
        .child(tool("open", IconName::FolderOpen, t!("toolbar.open"), Open))
        .child(tool("save", IconName::Save, t!("toolbar.save"), Save))
        .child(separator(cx))
        .child(tool("undo", IconName::Undo2, t!("toolbar.undo"), Undo))
        .child(tool("redo", IconName::Redo2, t!("toolbar.redo"), Redo))
        .child(separator(cx))
        .child(tool("bold", IconName::Bold, t!("toolbar.bold"), ToggleBold).selected(style.bold))
        .child(
            tool(
                "italic",
                IconName::Italic,
                t!("toolbar.italic"),
                ToggleItalic,
            )
            .selected(style.italic),
        )
        .child(
            tool(
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
                "align-left",
                IconName::TextAlignStart,
                t!("toolbar.align_left"),
                AlignLeft,
            )
            .selected(style.align == HAlign::Left),
        )
        .child(
            tool(
                "align-center",
                IconName::TextAlignCenter,
                t!("toolbar.align_center"),
                AlignCenter,
            )
            .selected(style.align == HAlign::Center),
        )
        .child(
            tool(
                "align-right",
                IconName::TextAlignEnd,
                t!("toolbar.align_right"),
                AlignRight,
            )
            .selected(style.align == HAlign::Right),
        )
        .child(
            tool(
                "wrap",
                IconName::TextWrap,
                t!("toolbar.wrap_text"),
                ToggleWrapText,
            )
            .selected(style.wrap),
        )
        .child(separator(cx))
        .child(tool(
            "general",
            IconName::CaseSensitive,
            t!("toolbar.format_general"),
            FormatGeneral,
        ))
        .child(tool(
            "more-decimals",
            IconName::DecimalsArrowRight,
            t!("toolbar.increase_decimal"),
            IncreaseDecimal,
        ))
        .child(tool(
            "fewer-decimals",
            IconName::DecimalsArrowLeft,
            t!("toolbar.decrease_decimal"),
            DecreaseDecimal,
        ))
        .child(tool(
            "number",
            IconName::Hash,
            t!("toolbar.format_number"),
            FormatNumber,
        ))
        .child(tool(
            "currency",
            IconName::DollarSign,
            t!("toolbar.format_currency"),
            FormatCurrency,
        ))
        .child(tool(
            "percent",
            IconName::Percent,
            t!("toolbar.format_percent"),
            FormatPercent,
        ))
        .child(tool(
            "date",
            IconName::Calendar,
            t!("toolbar.format_date"),
            FormatDate,
        ))
        .child(separator(cx))
        .child(tool(
            "chart",
            IconName::ChartColumn,
            t!("toolbar.chart"),
            InsertChart,
        ))
        .child(separator(cx))
        .child(tool(
            "zoom-out",
            IconName::ZoomOut,
            t!("toolbar.zoom_out"),
            ZoomOut,
        ))
        .child(tool(
            "zoom-in",
            IconName::ZoomIn,
            t!("toolbar.zoom_in"),
            ZoomIn,
        ))
        .child(div().flex_1())
        .child(tool(
            "theme",
            IconName::SunMoon,
            t!("toolbar.theme"),
            ToggleTheme,
        ))
}
