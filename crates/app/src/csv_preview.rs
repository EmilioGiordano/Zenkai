use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_formats::{Delimiter, ParsedCsv};

use crate::actions::{CancelCsvImport, ConfirmCsvImport};

const PREVIEW_ROWS: usize = 12;
const PREVIEW_COLS: usize = 8;
const DELIMITERS: [Delimiter; 4] = [
    Delimiter::Comma,
    Delimiter::Semicolon,
    Delimiter::Tab,
    Delimiter::Pipe,
];

pub struct CsvPreview {
    pub path: PathBuf,
    pub bytes: Arc<Vec<u8>>,
    pub parsed: ParsedCsv,
}

pub enum PreviewEvent {
    Delimiter(Delimiter),
}

pub fn render(
    preview: &CsvPreview,
    focus: &FocusHandle,
    on_event: impl Fn(PreviewEvent, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let name = preview
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let total_cols = preview.parsed.rows.iter().map(Vec::len).max().unwrap_or(0);
    let table = v_flex()
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .overflow_hidden()
        .children(
            preview
                .parsed
                .rows
                .iter()
                .take(PREVIEW_ROWS)
                .enumerate()
                .map(|(index, row)| {
                    h_flex()
                        .when(index % 2 == 1, |line| line.bg(theme.table_even))
                        .children((0..total_cols.min(PREVIEW_COLS)).map(|col| {
                            div()
                                .w(px(110.0))
                                .px_2()
                                .py_0p5()
                                .text_sm()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(row.get(col).cloned().unwrap_or_default())
                        }))
                }),
        );
    let delimiter_buttons = DELIMITERS.into_iter().map(|delimiter| {
        let on_event = on_event.clone();
        Button::new(delimiter.label())
            .ghost()
            .compact()
            .label(delimiter.label())
            .selected(preview.parsed.delimiter == delimiter)
            .on_click(move |_, window, cx| on_event(PreviewEvent::Delimiter(delimiter), window, cx))
    });
    v_flex()
        .key_context("CsvPreview")
        .track_focus(focus)
        .w(px(920.0))
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
                .child(format!("Import {name}")),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().text_color(theme.muted_foreground).child("Separator"))
                .children(delimiter_buttons)
                .child(div().flex_1())
                .child(div().text_color(theme.muted_foreground).child(format!(
                    "{} · {} rows · {} columns",
                    preview.parsed.encoding.label(),
                    preview.parsed.rows.len(),
                    total_cols
                ))),
        )
        .child(table)
        .child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("csv-cancel")
                        .label("Cancel")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(CancelCsvImport), cx)
                        }),
                )
                .child(
                    Button::new("csv-import")
                        .primary()
                        .label("Import")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ConfirmCsvImport), cx)
                        }),
                ),
        )
}
