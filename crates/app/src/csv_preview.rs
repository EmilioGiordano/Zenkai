mod columns;
mod controls;
mod details;
mod fit;
mod render;
mod table;
mod words;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::VirtualListScrollHandle;
use gpui_kit::*;
use zenkai_formats::{
    DateOrder, DecimalMark, Delimiter, Guess, ParsedCsv, find_at_risk, guess, parse_csv,
};
use zenkai_i18n::t;

use crate::csv_options::CsvOptions;
use crate::entry::FileSeen;
pub use columns::Format;
use columns::{Column, columns_of, detected_formats, first_selected, text_columns};

pub fn delimiter_label(delimiter: Delimiter) -> &'static str {
    match delimiter {
        Delimiter::Comma => t!("csv.delimiter.comma"),
        Delimiter::Semicolon => t!("csv.delimiter.semicolon"),
        Delimiter::Tab => t!("csv.delimiter.tab"),
        Delimiter::Pipe => t!("csv.delimiter.pipe"),
    }
}

// What reading the file as a given separator tells: its rows, how its numbers and dates read,
// and the columns the import would keep as text by itself. Scans every row, so callers run it
// off the UI thread.
pub struct Reading {
    parsed: ParsedCsv,
    guess: Guess,
    columns: Vec<Column>,
}

impl Reading {
    pub fn of(parsed: ParsedCsv) -> Reading {
        let guess = guess(&parsed);
        let columns = columns_of(&parsed.rows, guess.columns, find_at_risk(&parsed.rows));
        Reading {
            parsed,
            guess,
            columns,
        }
    }
}

// What the import needs of the preview once the user opens the file.
pub struct Import {
    pub path: PathBuf,
    pub bytes: Arc<Vec<u8>>,
    pub parsed: ParsedCsv,
    pub options: CsvOptions,
    pub seen: FileSeen,
}

pub enum PreviewEvent {
    ReadFailed(String),
}

impl EventEmitter<PreviewEvent> for CsvPreview {}

const SECTION_BLOCKS: usize = 5;
const NOTE_STOPS: usize = 3;
const SEPARATOR_STOPS: usize = 0;
const DECIMAL_STOPS: usize = 4;
const DATE_STOPS: usize = 6;
const CANCEL_STOP: usize = 8;
const OPEN_STOP: usize = 9;
const BUTTON_STOPS: usize = 10;

pub struct CsvPreview {
    path: PathBuf,
    bytes: Arc<Vec<u8>>,
    parsed: ParsedCsv,
    guess: Guess,
    seen: FileSeen,
    columns: Vec<Column>,
    // What the user chose for each column; Keep everything as text overrides all of them.
    formats: Vec<Format>,
    keep_all: bool,
    selected: usize,
    // Answers only the latest read of the file; an older one is dropped.
    request: u64,
    alt_held: bool,
    rows_shown: usize,
    section_scrolls: bool,
    sizes: Option<(Pixels, Rc<Vec<Size<Pixels>>>)>,
    focus: FocusHandle,
    strip: FocusHandle,
    radios: FocusHandle,
    keep_all_stop: FocusHandle,
    note_stops: [FocusHandle; NOTE_STOPS],
    button_stops: [FocusHandle; BUTTON_STOPS],
    blocks: [FocusHandle; SECTION_BLOCKS],
    shown_block: Option<usize>,
    columns_scroll: VirtualListScrollHandle,
    section_scroll: ScrollHandle,
}

impl Focusable for CsvPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CsvPreview {
    pub fn new(
        path: PathBuf,
        bytes: Arc<Vec<u8>>,
        reading: Reading,
        seen: FileSeen,
        cx: &mut Context<Self>,
    ) -> CsvPreview {
        let Reading {
            parsed,
            guess,
            columns,
        } = reading;
        let formats = detected_formats(&columns);
        let selected = first_selected(&columns);
        let columns_scroll = VirtualListScrollHandle::new();
        columns_scroll.scroll_to_item(selected, ScrollStrategy::Nearest);
        CsvPreview {
            path,
            bytes,
            parsed,
            guess,
            seen,
            columns,
            formats,
            keep_all: false,
            selected,
            request: 0,
            alt_held: false,
            rows_shown: columns::SAMPLE_ROWS,
            section_scrolls: false,
            sizes: None,
            focus: cx.focus_handle(),
            strip: cx.focus_handle().tab_stop(true),
            radios: cx.focus_handle().tab_stop(true),
            keep_all_stop: cx.focus_handle().tab_stop(true),
            note_stops: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            button_stops: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            blocks: std::array::from_fn(|_| cx.focus_handle()),
            shown_block: None,
            columns_scroll,
            section_scroll: ScrollHandle::new(),
        }
    }

    fn install(&mut self, reading: Reading) {
        let Reading {
            parsed,
            guess,
            columns,
        } = reading;
        self.columns = columns;
        self.formats = detected_formats(&self.columns);
        self.selected = first_selected(&self.columns);
        self.columns_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        self.sizes = None;
        self.parsed = parsed;
        self.guess = guess;
    }

    // A failed import brings the rows back; every choice is still as it was.
    pub fn restore(&mut self, parsed: ParsedCsv) {
        self.parsed = parsed;
    }

    pub fn take_import(&mut self) -> Import {
        // A read of the file that lands after this would put columns over rows it did not read.
        self.request += 1;
        let rows = std::mem::take(&mut self.parsed.rows);
        let parsed = ParsedCsv {
            rows,
            ..self.parsed.clone()
        };
        let options = CsvOptions {
            text: text_columns(&self.columns, &self.formats, self.keep_all),
            ..CsvOptions::from_read(&parsed, &self.guess)
        };
        Import {
            path: self.path.clone(),
            bytes: self.bytes.clone(),
            parsed,
            options,
            seen: self.seen,
        }
    }

    fn has_columns(&self) -> bool {
        !self.columns.is_empty()
    }

    // What a column is read as, whatever the choice stored for it.
    fn format_of(&self, index: usize) -> Format {
        if self.keep_all {
            Format::Text
        } else {
            self.formats.get(index).copied().unwrap_or(Format::General)
        }
    }

    fn formats_are_open(&self) -> bool {
        self.has_columns() && !self.keep_all
    }

    pub fn choose_separator(&mut self, delimiter: Delimiter, cx: &mut Context<Self>) {
        if delimiter == self.parsed.delimiter {
            return;
        }
        self.request += 1;
        let request = self.request;
        let bytes = self.bytes.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { parse_csv(&bytes, Some(delimiter)).map(Reading::of) })
                .await;
            let update = this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                match read {
                    Ok(reading) => this.install(reading),
                    Err(error) => cx.emit(PreviewEvent::ReadFailed(error.to_string())),
                }
                cx.notify();
            });
            if let Err(error) = update {
                tracing::debug!(%error, "the preview closed while the file was read again");
            }
        })
        .detach();
    }

    pub fn choose_decimal_mark(&mut self, mark: DecimalMark, cx: &mut Context<Self>) {
        self.guess.decimal_mark = mark;
        cx.notify();
    }

    pub fn choose_date_order(&mut self, order: DateOrder, cx: &mut Context<Self>) {
        self.guess.date_order = order;
        cx.notify();
    }

    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(last) = self.columns.len().checked_sub(1) else {
            return;
        };
        self.selected = index.min(last);
        self.columns_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_offset(&mut self, offset: isize, cx: &mut Context<Self>) {
        self.select(self.selected.saturating_add_signed(offset), cx);
    }

    fn select_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        let page = self.page_size().cast_signed();
        self.select_offset(if forward { page } else { -page }, cx);
    }

    // How many columns fit the strip from the selected one on, at least one.
    fn page_size(&self) -> usize {
        let viewport = f32::from(self.columns_scroll.bounds().size.width);
        let Some((_, sizes)) = &self.sizes else {
            return 1;
        };
        let mut used = 0.0;
        let fitting = sizes
            .iter()
            .skip(self.selected)
            .take_while(|size| {
                used += f32::from(size.width);
                used <= viewport
            })
            .count();
        fitting.max(1)
    }

    pub fn set_format(&mut self, format: Format, cx: &mut Context<Self>) {
        if !self.formats_are_open() {
            return;
        }
        if let Some(chosen) = self.formats.get_mut(self.selected) {
            *chosen = format;
            cx.notify();
        }
    }

    pub fn switch_format(&mut self, cx: &mut Context<Self>) {
        if let Some(current) = self.formats.get(self.selected).copied() {
            self.set_format(current.other(), cx);
        }
    }

    pub fn toggle_keep_all(&mut self, cx: &mut Context<Self>) {
        if !self.has_columns() {
            return;
        }
        self.keep_all = !self.keep_all;
        cx.notify();
    }

    // The next column the import kept as text by itself, after the selected one, from the
    // first again when there is none further on.
    fn select_next_flagged(&mut self, cx: &mut Context<Self>) {
        let count = self.columns.len();
        let next = (1..=count)
            .map(|step| (self.selected + step) % count)
            .find(|index| self.columns[*index].risk.is_some());
        if let Some(next) = next {
            self.select(next, cx);
        }
    }

    fn focus_strip(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.strip, cx);
    }

    fn scroll_section(&mut self, down: bool, cx: &mut Context<Self>) {
        if !self.section_scrolls {
            return;
        }
        let offset = self.section_scroll.offset();
        let furthest = -f32::from(self.section_scroll.max_offset().y);
        let step = if down {
            -fit::ROW_HEIGHT
        } else {
            fit::ROW_HEIGHT
        };
        let target = (f32::from(offset.y) + step).clamp(furthest, 0.0);
        self.section_scroll.set_offset(point(offset.x, px(target)));
        cx.notify();
    }

    fn note_stops_shown(&self) -> usize {
        match words::note(&self.columns, &self.formats, self.keep_all) {
            words::Note::Flagged { entries, more } => entries.len() + usize::from(more > 0),
            words::Note::Sentence(_) => 0,
        }
    }

    // A choice can take away the stop that has focus (Keep everything as text closes the radios,
    // a new reading of the file the entries of the note). Focus left on a stop that is no longer
    // there takes every key of the dialog with it, so the dialog takes it back.
    fn keep_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mut gone: Vec<&FocusHandle> =
            self.note_stops[self.note_stops_shown()..].iter().collect();
        if !self.has_columns() {
            gone.extend([&self.keep_all_stop, &self.strip]);
        }
        if !self.formats_are_open() {
            gone.push(&self.radios);
        }
        if self.keep_all {
            gone.extend(&self.button_stops[DECIMAL_STOPS..CANCEL_STOP]);
        }
        if gone.iter().any(|stop| stop.is_focused(window)) {
            window.focus(&self.focus, cx);
        }
    }

    // How many stops Tab visits in the dialog now, so that Shift+Tab can wrap round to the last.
    fn tab_stops(&self) -> usize {
        let notes = self.note_stops_shown();
        let separators = 4;
        let reading_choices = if self.keep_all { 0 } else { 4 };
        let checkbox = usize::from(self.has_columns());
        let strip = usize::from(self.has_columns());
        let radios = usize::from(self.formats_are_open());
        let buttons = 2;
        separators + reading_choices + checkbox + notes + strip + radios + buttons
    }

    // Tab wraps round inside the dialog; focus never lands on the window behind it.
    fn cycle_focus(&self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if forward {
            window.focus_next(cx);
            if !self.focus.contains_focused(window, cx) {
                window.focus(&self.focus, cx);
                window.focus_next(cx);
            }
        } else {
            window.focus_prev(cx);
            if !self.focus.contains_focused(window, cx) {
                window.focus(&self.focus, cx);
                for _ in 0..self.tab_stops() {
                    window.focus_next(cx);
                }
            }
        }
    }

    // The sizes the strip lays its columns out at, kept while the interface scale is the same.
    fn column_sizes(&mut self, rem_size: Pixels) -> Rc<Vec<Size<Pixels>>> {
        if let Some((for_rem, sizes)) = &self.sizes
            && *for_rem == rem_size
        {
            return sizes.clone();
        }
        let height = columns_height(self.rows_shown).to_pixels(rem_size);
        let sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
            self.columns
                .iter()
                .map(|column| {
                    size(
                        crate::interface_scale::rem_px(column.width).to_pixels(rem_size),
                        height,
                    )
                })
                .collect(),
        );
        self.sizes = Some((rem_size, sizes.clone()));
        sizes
    }

    pub fn rows_changed(&mut self, rows: usize) {
        if self.rows_shown != rows {
            self.rows_shown = rows;
            self.sizes = None;
        }
    }
}

fn columns_height(rows: usize) -> Rems {
    crate::interface_scale::rem_px(table::HEADER_HEIGHT + rows as f32 * fit::ROW_HEIGHT)
}
