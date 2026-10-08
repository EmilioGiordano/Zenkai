use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use zenkai_types::Range;

use crate::actions::{ApplyNumberFormat, CloseFormatDialog};

// Excel's Format Cells > Number categories, with the code each one starts from.
pub const CATEGORIES: [(&str, &str); 10] = [
    ("General", "general"),
    ("Number", "#,##0.00"),
    ("Currency", "$#,##0.00"),
    (
        "Accounting",
        "_($* #,##0.00_);_($* (#,##0.00);_($* \"-\"??_);_(@_)",
    ),
    ("Date", "dd/mm/yyyy"),
    ("Time", "h:mm:ss"),
    ("Percentage", "0.00%"),
    ("Fraction", "# ?/?"),
    ("Scientific", "0.00E+00"),
    ("Text", "@"),
];

pub struct FormatDialog {
    pub code: Entity<InputState>,
    pub sample: f64,
    pub range: Range,
    pub focus: FocusHandle,
    // Redraws the sample as the code is typed; dropped with the dialog.
    pub _refresh: Subscription,
}

pub fn render(
    dialog: &FormatDialog,
    preview: String,
    on_category: impl Fn(&'static str, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let current = dialog.code.read(cx).value().to_string();
    let categories = CATEGORIES.map(|(label, code)| {
        let on_category = on_category.clone();
        let selected = current == code;
        Button::new(label)
            .ghost()
            .label(if selected {
                format!("✓ {label}")
            } else {
                label.to_string()
            })
            .selected(selected)
            .on_click(move |_, window, cx| on_category(code, window, cx))
    });
    v_flex()
        .key_context("FormatDialog")
        .track_focus(&dialog.focus)
        // Clicks on the dialog must not reach the grid underneath.
        .occlude()
        .w(px(560.0))
        .p_4()
        .gap_3()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
        .shadow_lg()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Format Cells: Number"),
        )
        .child(
            h_flex()
                .gap_4()
                .items_start()
                .child(v_flex().w(px(150.0)).gap_0p5().children(categories))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Sample"),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .border_1()
                                .border_color(theme.border)
                                .rounded_md()
                                .child(preview),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Type"),
                        )
                        .child(Input::new(&dialog.code)),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("format-cancel")
                        .label("Cancel")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(CloseFormatDialog), cx)
                        }),
                )
                .child(
                    Button::new("format-ok")
                        .primary()
                        .label("OK")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ApplyNumberFormat), cx)
                        }),
                ),
        )
}
