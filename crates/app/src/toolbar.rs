use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
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

pub fn render(style: &CellStyle, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .h(px(36.0))
        .px_2()
        .gap_0p5()
        .items_center()
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.border)
        .child(tool("new", IconName::FilePlus, "New (Ctrl+N)", NewWorkbook))
        .child(tool("open", IconName::FolderOpen, "Open (Ctrl+O)", Open))
        .child(tool("save", IconName::Save, "Save (Ctrl+S)", Save))
        .child(separator(cx))
        .child(tool("undo", IconName::Undo2, "Undo (Ctrl+Z)", Undo))
        .child(tool("redo", IconName::Redo2, "Redo (Ctrl+Y)", Redo))
        .child(separator(cx))
        .child(tool("bold", IconName::Bold, "Bold (Ctrl+B)", ToggleBold).selected(style.bold))
        .child(
            tool("italic", IconName::Italic, "Italic (Ctrl+I)", ToggleItalic)
                .selected(style.italic),
        )
        .child(
            tool(
                "underline",
                IconName::Underline,
                "Underline (Ctrl+U)",
                ToggleUnderline,
            )
            .selected(style.underline),
        )
        .child(separator(cx))
        .child(
            tool(
                "align-left",
                IconName::TextAlignStart,
                "Align left",
                AlignLeft,
            )
            .selected(style.align == HAlign::Left),
        )
        .child(
            tool(
                "align-center",
                IconName::TextAlignCenter,
                "Center",
                AlignCenter,
            )
            .selected(style.align == HAlign::Center),
        )
        .child(
            tool(
                "align-right",
                IconName::TextAlignEnd,
                "Align right",
                AlignRight,
            )
            .selected(style.align == HAlign::Right),
        )
        .child(separator(cx))
        .child(tool(
            "general",
            IconName::Baseline,
            "General (Ctrl+Shift+~)",
            FormatGeneral,
        ))
        .child(tool(
            "number",
            IconName::Hash,
            "Number (Ctrl+Shift+!)",
            FormatNumber,
        ))
        .child(tool(
            "currency",
            IconName::DollarSign,
            "Currency (Ctrl+Shift+$)",
            FormatCurrency,
        ))
        .child(tool(
            "percent",
            IconName::Percent,
            "Percent (Ctrl+Shift+%)",
            FormatPercent,
        ))
        .child(tool(
            "date",
            IconName::Calendar,
            "Date (Ctrl+Shift+#)",
            FormatDate,
        ))
        .child(separator(cx))
        .child(tool(
            "zoom-out",
            IconName::ZoomOut,
            "Zoom out (Ctrl+-)",
            ZoomOut,
        ))
        .child(tool(
            "zoom-in",
            IconName::ZoomIn,
            "Zoom in (Ctrl+=)",
            ZoomIn,
        ))
        .child(div().flex_1())
        .child(tool(
            "theme",
            IconName::SunMoon,
            "Light or dark theme",
            ToggleTheme,
        ))
}
