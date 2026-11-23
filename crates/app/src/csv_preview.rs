use std::path::PathBuf;
use std::sync::Arc;
use zenkai_i18n::t;

use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_formats::{DateOrder, DecimalMark, Delimiter, Guess, ParsedCsv, apply};

use crate::actions::{CancelCsvImport, ConfirmCsvImport};
use crate::entry::FileSeen;
use crate::interface_scale::rem_px;

const PREVIEW_ROWS: usize = 12;
const PREVIEW_COLS: usize = 8;
const DELIMITERS: [Delimiter; 4] = [
    Delimiter::Comma,
    Delimiter::Semicolon,
    Delimiter::Tab,
    Delimiter::Pipe,
];

pub fn delimiter_label(delimiter: Delimiter) -> &'static str {
    match delimiter {
        Delimiter::Comma => t!("csv.delimiter.comma"),
        Delimiter::Semicolon => t!("csv.delimiter.semicolon"),
        Delimiter::Tab => t!("csv.delimiter.tab"),
        Delimiter::Pipe => t!("csv.delimiter.pipe"),
    }
}

pub struct CsvPreview {
    pub path: PathBuf,
    pub bytes: Arc<Vec<u8>>,
    pub parsed: ParsedCsv,
    pub guess: Guess,
    // The file as it was when read, for the document the import makes of it.
    pub seen: FileSeen,
    // What the table shows, built once and again when the reading changes, not every frame.
    pub shown: Vec<Vec<String>>,
}

impl CsvPreview {
    pub fn new(
        path: PathBuf,
        bytes: Arc<Vec<u8>>,
        parsed: ParsedCsv,
        guess: Guess,
        seen: FileSeen,
    ) -> CsvPreview {
        let mut preview = CsvPreview {
            path,
            bytes,
            parsed,
            guess,
            seen,
            shown: Vec::new(),
        };
        preview.rebuild();
        preview
    }

    // The table shows the values as they will be imported.
    pub fn rebuild(&mut self) {
        let mut shown: Vec<Vec<String>> = self
            .parsed
            .rows
            .iter()
            .take(PREVIEW_ROWS)
            .map(|row| row.iter().take(PREVIEW_COLS).cloned().collect())
            .collect();
        apply(self.guess.decimal_mark, self.guess.date_order, &mut shown);
        self.shown = shown;
    }
}

#[derive(Clone, Copy)]
pub enum PreviewEvent {
    Delimiter(Delimiter),
    DecimalMark(DecimalMark),
    DateOrder(DateOrder),
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
    let total_cols = preview.guess.columns;
    let table = v_flex()
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .overflow_hidden()
        .children(preview.shown.iter().enumerate().map(|(index, row)| {
            h_flex()
                .when(index % 2 == 1, |line| line.bg(theme.table_even))
                .children((0..total_cols.min(PREVIEW_COLS)).map(|col| {
                    div()
                        .w(rem_px(110.0))
                        .px_2()
                        .py_0p5()
                        .text_sm()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(row.get(col).cloned().unwrap_or_default())
                }))
        }));
    let choice = |label: &'static str, selected: bool, event: PreviewEvent| {
        let on_event = on_event.clone();
        // The check mark keeps the choice readable without colour.
        let text = if selected {
            format!("✓ {label}")
        } else {
            label.to_string()
        };
        Button::new(label)
            .ghost()
            .compact()
            .label(text)
            .selected(selected)
            .on_click(move |_, window, cx| on_event(event, window, cx))
    };
    let delimiter_buttons = DELIMITERS.map(|delimiter| {
        choice(
            delimiter_label(delimiter),
            preview.parsed.delimiter == delimiter,
            PreviewEvent::Delimiter(delimiter),
        )
    });
    let decimal_buttons =
        [(DecimalMark::Dot, "1.5"), (DecimalMark::Comma, "1,5")].map(|(mark, label)| {
            choice(
                label,
                preview.guess.decimal_mark == mark,
                PreviewEvent::DecimalMark(mark),
            )
        });
    let date_buttons = [
        (DateOrder::DayFirst, t!("csv.date_day_first")),
        (DateOrder::MonthFirst, t!("csv.date_month_first")),
    ]
    .map(|(order, label)| {
        choice(
            label,
            preview.guess.date_order == order,
            PreviewEvent::DateOrder(order),
        )
    });
    v_flex()
        .key_context("CsvPreview")
        .track_focus(focus)
        // Clicks on the preview must not reach the grid underneath.
        .occlude()
        .w(rem_px(920.0))
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
                .child(t!("csv.title", name = name)),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t!("csv.separator")),
                )
                .children(delimiter_buttons)
                .child(div().w(rem_px(12.0)))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t!("csv.decimal")),
                )
                .children(decimal_buttons)
                .child(div().w(rem_px(12.0)))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t!("csv.dates")),
                )
                .children(date_buttons)
                .child(div().flex_1())
                .child(div().text_color(theme.muted_foreground).child(t!(
                    "csv.summary",
                    encoding = preview.parsed.encoding.label(),
                    rows = t!("csv.rows", count = preview.parsed.rows.len()),
                    columns = t!("csv.columns", count = total_cols)
                ))),
        )
        .child(table)
        .child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("csv-cancel")
                        .label(t!("button.cancel"))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(CancelCsvImport), cx)
                        }),
                )
                .child(
                    Button::new("csv-import")
                        .primary()
                        .label(t!("csv.import"))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ConfirmCsvImport), cx)
                        }),
                ),
        )
}
