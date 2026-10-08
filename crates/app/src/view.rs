use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

use zenkai_engine::{Engine, EngineError, Opened, Workbook, open_xlsx, save_xlsx_atomic};
use zenkai_grid::{Direction, EditMode, Grid, GridEvent, Layout, SheetView};
use zenkai_types::{CellPos, CellStyle, HAlign, NumberFormat, Range, SheetId, StyleChange};

use crate::actions::*;
use crate::clipboard;
use crate::document::{self, Document};
use crate::files;
use crate::jump::jump_target;
use crate::stats::{self, SelectionStats};
use crate::toolbar;

const SAVING: &str = "Saving…";
const CALCULATING: &str = "Calculating…";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Info,
    Warning,
    Error,
}

struct Notice {
    severity: Severity,
    text: SharedString,
}

pub struct Workspace {
    document: Document,
    grid: Entity<Grid>,
    stats: Option<SelectionStats>,
    active_input: SharedString,
    active_style: CellStyle,
    notice: Option<Notice>,
    busy: Option<SharedString>,
    last_recalc: Option<Duration>,
    diagnostics: bool,
    clipboard_source: Option<(Range, String)>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Workspace {
        let grid = cx.new(Grid::new);
        let subscription = cx.subscribe_in(&grid, window, Self::on_grid_event);
        let workbook = match Workbook::new_empty() {
            Ok(workbook) => workbook,
            Err(error) => exit_without_workbook(error),
        };
        let mut workspace = Workspace {
            document: Document::new(workbook, None, Vec::new()),
            grid,
            stats: None,
            active_input: SharedString::default(),
            active_style: CellStyle::default(),
            notice: None,
            busy: None,
            last_recalc: None,
            diagnostics: false,
            clipboard_source: None,
            _subscriptions: vec![subscription],
        };
        workspace.reset_grid(window, cx);
        if let Some(path) = initial {
            workspace.open_path(path, window, cx);
        }
        workspace
    }

    fn reset_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.document.sheet;
        let view = self
            .document
            .workbook()
            .map(|wb| {
                let (frozen_rows, frozen_cols) = wb.frozen(sheet);
                SheetView {
                    layout: Layout::from_sizes(&wb.sizes(sheet)),
                    frozen_rows,
                    frozen_cols,
                    merges: wb.merged(sheet),
                }
            })
            .unwrap_or_default();
        self.grid.update(cx, |grid, cx| grid.reset(view, cx));
        window.set_window_title(&self.document.title());
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        self.refresh_cells(cx);
    }

    fn refresh_cells(&mut self, cx: &mut Context<Self>) {
        let ranges = self.grid.read(cx).visible_ranges();
        let mut cells = HashMap::new();
        for range in ranges {
            cells.extend(self.document.cells(range));
        }
        self.grid.update(cx, |grid, cx| grid.set_cells(cells, cx));
        self.refresh_stats(cx);
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        let selection = self.grid.read(cx).selection();
        let sheet = self.document.sheet;
        if let Some(wb) = self.document.workbook() {
            self.stats = stats::compute(wb, sheet, selection.range());
            self.active_input = wb.input(sheet, selection.active).into();
            self.active_style = wb.cell(sheet, selection.active).style;
        }
        cx.notify();
    }

    fn notify(
        &mut self,
        severity: Severity,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(Notice {
            severity,
            text: text.into(),
        });
        cx.notify();
    }

    fn on_grid_event(
        &mut self,
        _: &Entity<Grid>,
        event: &GridEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            GridEvent::SelectionChanged => self.refresh_stats(cx),
            GridEvent::ViewportChanged => self.refresh_cells(cx),
            GridEvent::EditChanged => cx.notify(),
            GridEvent::EditRequested(pos) => {
                let text = self
                    .document
                    .workbook()
                    .map(|wb| wb.input(self.document.sheet, *pos))
                    .unwrap_or_default();
                let pos = *pos;
                self.grid.update(cx, |grid, cx| {
                    grid.begin_edit(pos, text, EditMode::Edit, cx)
                });
            }
            GridEvent::Commit { pos, text } => {
                let (sheet, pos, text) = (self.document.sheet, *pos, text.clone());
                self.edit(window, cx, move |wb| wb.set_input(sheet, pos, &text));
            }
            GridEvent::ClearRequested(range) => {
                let (sheet, range) = (self.document.sheet, *range);
                self.edit(window, cx, move |wb| wb.clear(sheet, range));
            }
            GridEvent::Jump { direction, extend } => self.jump(*direction, *extend, cx),
            GridEvent::EndRequested { extend } => {
                let Some(end) = self
                    .document
                    .workbook()
                    .map(|wb| wb.used_end(self.document.sheet))
                else {
                    return;
                };
                let extend = *extend;
                self.grid.update(cx, |grid, cx| {
                    let active = if extend { grid.selection().active } else { end };
                    grid.select(active, end, cx);
                });
            }
        }
    }

    fn jump(&mut self, direction: Direction, extend: bool, cx: &mut Context<Self>) {
        let Some(workbook) = self.document.workbook() else {
            return;
        };
        let sheet = self.document.sheet;
        let selection = self.grid.read(cx).selection();
        let from = if extend {
            selection.corner
        } else {
            selection.active
        };
        let used_end = workbook.used_end(sheet);
        let target = jump_target(from, direction, used_end, |pos| {
            !workbook.cell(sheet, pos).text.is_empty()
        });
        self.grid.update(cx, |grid, cx| {
            let active = if extend { selection.active } else { target };
            grid.select(active, target, cx);
        });
    }

    fn edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Workbook) -> Result<(), EngineError> + Send + 'static,
    ) {
        self.document.queue(Box::new(edit));
        self.document.dirty = true;
        window.set_window_title(&self.document.title());
        self.flush_edits(cx);
    }

    fn flush_edits(&mut self, cx: &mut Context<Self>) {
        let Some((mut workbook, edits)) = self.document.take_batch() else {
            return;
        };
        self.busy = Some(CALCULATING.into());
        cx.notify();
        let started = Instant::now();
        let generation = self.document.generation();
        cx.spawn(async move |this, cx| {
            let (workbook, errors) = cx
                .background_executor()
                .spawn(async move {
                    let errors = document::run_batch(&mut workbook, edits);
                    (workbook, errors)
                })
                .await;
            let update = this.update(cx, |this, cx| {
                if !this.document.restore(workbook, generation) {
                    this.clear_busy(CALCULATING, cx);
                    return;
                }
                this.last_recalc = Some(started.elapsed());
                this.clear_busy(CALCULATING, cx);
                if let Some(error) = errors.first() {
                    this.notify(Severity::Error, error.to_string(), cx);
                }
                this.refresh_cells(cx);
                this.flush_edits(cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during recalc");
            }
        })
        .detach();
    }

    fn clear_busy(&mut self, label: &str, cx: &mut Context<Self>) {
        if self
            .busy
            .as_ref()
            .is_some_and(|busy| busy.as_ref() == label)
        {
            self.busy = None;
            cx.notify();
        }
    }

    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = Some(format!("Opening {}…", path.display()).into());
        cx.notify();
        let started = Instant::now();
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result: Result<Opened, EngineError> = cx
                .background_executor()
                .spawn(async move { open_xlsx(&task_path) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(opened) => {
                        let unsupported = opened.unsupported.clone();
                        this.document =
                            Document::new(opened.workbook, Some(path), opened.unsupported);
                        this.reset_grid(window, cx);
                        if unsupported.is_empty() {
                            this.notify(
                                Severity::Info,
                                format!("Opened in {} ms", started.elapsed().as_millis()),
                                cx,
                            );
                        } else {
                            let list: Vec<&str> = unsupported.iter().map(|u| u.label()).collect();
                            this.notify(
                                Severity::Warning,
                                format!(
                                    "This file has content Zenkai does not keep yet ({}). Saving will ask for a new name.",
                                    list.join(", ")
                                ),
                                cx,
                            );
                        }
                    }
                    Err(error) => this.notify(Severity::Error, error.to_string(), cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during open");
            }
        })
        .detach();
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "open dialog failed");
                    None
                }
                Err(error) => {
                    tracing::warn!(%error, "open dialog dropped");
                    None
                }
            };
            if let Some(path) = chosen
                && let Err(error) =
                    this.update_in(cx, |this, window, cx| this.open_path(path, window, cx))
            {
                tracing::debug!(%error, "workspace closed during open dialog");
            }
        })
        .detach();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let must_rename = !self.document.unsupported.is_empty() || self.document.is_macro_enabled();
        match self.document.path.clone() {
            Some(path) if !must_rename => self.save_to(path, window, cx),
            Some(_) => {
                let detail = if self.document.is_macro_enabled() {
                    let mut text =
                        "Macros are not kept. The original .xlsm file will not be overwritten."
                            .to_string();
                    if !self.document.unsupported.is_empty() {
                        text.push_str(&format!(
                            " Also lost: {}.",
                            self.document.unsupported_labels()
                        ));
                    }
                    text
                } else {
                    format!(
                        "Saving will lose: {}. Save a copy with a new name to keep the original intact.",
                        self.document.unsupported_labels()
                    )
                };
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "This workbook has content Zenkai cannot save yet",
                    Some(&detail),
                    &["Save As…", "Cancel"],
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await == Ok(0)
                        && let Err(error) =
                            this.update_in(cx, |this, window, cx| this.save_as(&SaveAs, window, cx))
                    {
                        tracing::debug!(%error, "workspace closed during save prompt");
                    }
                })
                .detach();
            }
            None => self.save_as(&SaveAs, window, cx),
        }
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self
            .document
            .path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let suggested = self
            .document
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(
                || "Book1".to_string(),
                |s| format!("{} (Zenkai)", s.to_string_lossy()),
            );
        let path = cx.prompt_for_new_path(&directory, Some(&format!("{suggested}.xlsx")));
        cx.spawn_in(window, async move |this, cx| {
            let chosen = match path.await {
                Ok(Ok(Some(path))) => Some(path),
                Ok(Ok(None)) => None,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "save dialog failed");
                    None
                }
                Err(error) => {
                    tracing::warn!(%error, "save dialog dropped");
                    None
                }
            };
            if let Some(path) = chosen
                && let Err(error) =
                    this.update_in(cx, |this, window, cx| this.confirm_target(path, window, cx))
            {
                tracing::debug!(%error, "workspace closed during save dialog");
            }
        })
        .detach();
    }

    fn confirm_target(&mut self, chosen: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = files::xlsx_target(&chosen);
        let renamed = path != chosen;
        let replaces_source = self
            .document
            .path
            .as_ref()
            .is_some_and(|source| files::same_file(source, &path));
        let (title, detail) = if replaces_source && self.document.is_macro_enabled() {
            self.notify(
                Severity::Error,
                "Macro-enabled workbooks are never overwritten. Choose a new name.",
                cx,
            );
            return;
        } else if replaces_source && !self.document.unsupported.is_empty() {
            (
                "Replace the original file?",
                format!(
                    "Replacing the original loses: {}. This cannot be undone.",
                    self.document.unsupported_labels()
                ),
            )
        } else if renamed && path.exists() {
            (
                "Replace the existing file?",
                format!(
                    "Zenkai saves as .xlsx, so the workbook goes to {}, which already exists.",
                    path.display()
                ),
            )
        } else {
            self.save_to(path, window, cx);
            return;
        };
        let answer = window.prompt(
            PromptLevel::Critical,
            title,
            Some(&detail),
            &["Cancel", "Replace"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1)
                && let Err(error) =
                    this.update_in(cx, |this, window, cx| this.save_to(path, window, cx))
            {
                tracing::debug!(%error, "workspace closed during replace prompt");
            }
        })
        .detach();
    }

    fn save_to(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbook) = self.document.take() else {
            self.notify(
                Severity::Warning,
                "Still calculating, try again in a moment.",
                cx,
            );
            return;
        };
        self.busy = Some(SAVING.into());
        cx.notify();
        let generation = self.document.generation();
        cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let (workbook, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = save_xlsx_atomic(&workbook, &target);
                    (workbook, result)
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                if !this.document.restore(workbook, generation) {
                    this.clear_busy(SAVING, cx);
                    if let Err(error) = result {
                        this.notify(
                            Severity::Error,
                            format!("Saving {} failed: {error}", path.display()),
                            cx,
                        );
                    }
                    return;
                }
                this.clear_busy(SAVING, cx);
                match result {
                    Ok(()) => {
                        if this.document.path.as_ref() != Some(&path) {
                            this.document.unsupported.clear();
                        }
                        this.document.path = Some(path);
                        this.document.dirty = this.document.has_pending();
                        window.set_window_title(&this.document.title());
                        this.notify(Severity::Info, "Saved", cx);
                    }
                    Err(error) => this.notify(
                        Severity::Error,
                        format!("{error}. Use Save As (F12) to pick another location."),
                        cx,
                    ),
                }
                this.flush_edits(cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during save");
            }
        })
        .detach();
    }

    fn new_workbook(&mut self, _: &NewWorkbook, window: &mut Window, cx: &mut Context<Self>) {
        match Workbook::new_empty() {
            Ok(workbook) => {
                self.document = Document::new(workbook, None, Vec::new());
                self.reset_grid(window, cx);
            }
            Err(error) => self.notify(Severity::Error, error.to_string(), cx),
        }
    }

    fn selection(&self, cx: &App) -> Range {
        self.grid.read(cx).selection().range()
    }

    fn style(&mut self, change: StyleChange, window: &mut Window, cx: &mut Context<Self>) {
        let (sheet, range) = (self.document.sheet, self.selection(cx));
        self.edit(window, cx, move |wb| wb.apply_style(sheet, range, change));
    }

    fn toggle_flag(
        &mut self,
        read: fn(&CellStyle) -> bool,
        make: fn(bool) -> StyleChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.grid.read(cx).selection().active;
        let current = self
            .document
            .workbook()
            .is_some_and(|wb| read(&wb.cell(self.document.sheet, active).style));
        self.style(make(!current), window, cx);
    }

    fn copy(&mut self, cut: bool, cx: &mut Context<Self>) {
        let range = self.selection(cx);
        let Some(workbook) = self.document.workbook() else {
            return;
        };
        match clipboard::copy_tsv(workbook, self.document.sheet, range) {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.clipboard_source = cut.then_some((range, text));
                self.grid
                    .update(cx, |grid, cx| grid.set_marquee(Some(range), cx));
            }
            None => self.notify(Severity::Warning, "The selection is too large to copy.", cx),
        }
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let rows = clipboard::parse_tsv(&text);
        let sheet = self.document.sheet;
        let origin = self.grid.read(cx).selection().active;
        let cut_source = self
            .clipboard_source
            .take()
            .filter(|(_, cut_text)| *cut_text == text)
            .map(|(range, _)| range);
        let height = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        self.edit(window, cx, move |wb| {
            if let Some(source) = cut_source {
                wb.clear(sheet, source)?;
            }
            for (r, row) in (0i64..).zip(rows) {
                for (c, value) in (0i64..).zip(row) {
                    let pos = CellPos::new(origin.row.offset(r), origin.col.offset(c));
                    wb.set_input(sheet, pos, &value)?;
                }
            }
            Ok(())
        });
        let end = CellPos::new(
            origin.row.offset(i64::from(height.saturating_sub(1))),
            origin.col.offset(i64::from(width.saturating_sub(1))),
        );
        self.grid.update(cx, |grid, cx| {
            grid.set_marquee(None, cx);
            grid.select(origin, end, cx);
        });
    }

    fn switch_sheet(&mut self, sheet: SheetId, window: &mut Window, cx: &mut Context<Self>) {
        if sheet.0 as usize >= self.document.sheets.len() || sheet == self.document.sheet {
            return;
        }
        self.document.sheet = sheet;
        self.reset_grid(window, cx);
    }

    fn render_formula_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let grid = self.grid.read(cx);
        let selection = grid.selection();
        let name = selection.active.to_string();
        let content: SharedString = match grid.editor() {
            Some(editor) => editor.text.clone().into(),
            None => self.active_input.clone(),
        };
        let grid_entity = self.grid.clone();
        h_flex()
            .h(px(32.0))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .w(px(96.0))
                    .h(px(24.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_sm()
                    .text_sm()
                    .child(name),
            )
            .child(
                div()
                    .text_sm()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .px_1()
                    .child("fx"),
            )
            .child(
                div()
                    .id("formula-content")
                    .flex_1()
                    .h(px(24.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_sm()
                    .text_sm()
                    .overflow_hidden()
                    .cursor_text()
                    .on_click(move |_, window, cx| {
                        grid_entity.update(cx, |grid, cx| {
                            window.focus(&grid.focus_handle(cx), cx);
                            if grid.editor().is_none() {
                                cx.emit(GridEvent::EditRequested(grid.selection().active));
                            }
                        });
                    })
                    .child(content),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entity = cx.entity().downgrade();
        let tabs = self
            .document
            .sheets
            .iter()
            .map(|sheet| Tab::new().label(sheet.name.clone()));
        h_flex()
            .h(px(32.0))
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(
                TabBar::new("sheets")
                    .children(tabs)
                    .selected_index(self.document.sheet.0 as usize)
                    .on_click(move |index, window, cx| {
                        let sheet = SheetId(u32::try_from(*index).unwrap_or(0));
                        if let Err(error) =
                            entity.update(cx, |this, cx| this.switch_sheet(sheet, window, cx))
                        {
                            tracing::debug!(%error, "workspace dropped");
                        }
                    }),
            )
            .child(
                div()
                    .id("add-sheet")
                    .px_3()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| this.add_sheet(window, cx))),
            )
    }

    fn add_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(window, cx, |wb| wb.add_sheet().map(|_| ()));
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut left = h_flex().gap_3().items_center();
        if let Some(busy) = &self.busy {
            left = left.child(div().text_color(theme.primary).child(busy.clone()));
        } else if let Some(notice) = &self.notice {
            let (icon, color) = match notice.severity {
                Severity::Info => ("ⓘ", theme.muted_foreground),
                Severity::Warning => ("⚠", theme.warning),
                Severity::Error => ("✕", theme.danger),
            };
            left = left.child(
                h_flex()
                    .gap_1()
                    .text_color(color)
                    .child(icon)
                    .child(notice.text.clone()),
            );
        } else {
            left = left.child(div().text_color(theme.muted_foreground).child("Ready"));
        }
        let mut right = h_flex()
            .gap_4()
            .items_center()
            .text_color(theme.muted_foreground);
        match self.stats {
            Some(stats) if stats.count > 1 => {
                if let Some(avg) = stats.average() {
                    right = right
                        .child(format!("Average: {}", format_number(avg)))
                        .child(format!("Sum: {}", format_number(stats.sum)));
                }
                right = right.child(format!("Count: {}", stats.count));
            }
            None => right = right.child("Selection too large to summarize"),
            _ => {}
        }
        let zoom = self.grid.read(cx).zoom();
        right = right.child(format!("{:.0}%", zoom * 100.0));
        if self.diagnostics {
            let recalc = self
                .last_recalc
                .map_or("-".to_string(), |d| format!("{} ms", d.as_millis()));
            right = right.child(format!("Last recalc: {recalc}"));
        }
        h_flex()
            .h(px(26.0))
            .px_3()
            .justify_between()
            .items_center()
            .text_xs()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .child(left)
            .child(right)
    }
}

fn exit_without_workbook(error: EngineError) -> Workbook {
    tracing::error!(%error, "could not create an empty workbook");
    std::process::exit(1)
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        let text = format!("{n:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .key_context("Workspace")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::new_workbook))
            .on_action(
                cx.listener(|this, _: &Undo, window, cx| this.edit(window, cx, |wb| wb.undo())),
            )
            .on_action(
                cx.listener(|this, _: &Redo, window, cx| this.edit(window, cx, |wb| wb.redo())),
            )
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(false, cx)))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.copy(true, cx)))
            .on_action(cx.listener(|this, _: &Paste, window, cx| this.paste(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleBold, window, cx| {
                this.toggle_flag(|s| s.bold, StyleChange::Bold, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleItalic, window, cx| {
                this.toggle_flag(|s| s.italic, StyleChange::Italic, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleUnderline, window, cx| {
                this.toggle_flag(|s| s.underline, StyleChange::Underline, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AlignLeft, window, cx| {
                this.style(StyleChange::Align(HAlign::Left), window, cx)
            }))
            .on_action(cx.listener(|this, _: &AlignCenter, window, cx| {
                this.style(StyleChange::Align(HAlign::Center), window, cx)
            }))
            .on_action(cx.listener(|this, _: &AlignRight, window, cx| {
                this.style(StyleChange::Align(HAlign::Right), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatGeneral, window, cx| {
                this.style(StyleChange::NumberFormat(NumberFormat::General), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatNumber, window, cx| {
                this.style(StyleChange::NumberFormat(NumberFormat::Number), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatCurrency, window, cx| {
                this.style(
                    StyleChange::NumberFormat(NumberFormat::Currency),
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &FormatPercent, window, cx| {
                this.style(StyleChange::NumberFormat(NumberFormat::Percent), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatDate, window, cx| {
                this.style(StyleChange::NumberFormat(NumberFormat::Date), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.grid.update(cx, |g, cx| g.set_zoom(g.zoom() + 0.1, cx));
                this.refresh_cells(cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.grid.update(cx, |g, cx| g.set_zoom(g.zoom() - 0.1, cx));
                this.refresh_cells(cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| {
                this.grid.update(cx, |g, cx| g.set_zoom(1.0, cx));
                this.refresh_cells(cx);
            }))
            .on_action(cx.listener(|this, _: &NextSheet, window, cx| {
                let next = SheetId(this.document.sheet.0 + 1);
                this.switch_sheet(next, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousSheet, window, cx| {
                let previous = SheetId(this.document.sheet.0.saturating_sub(1));
                this.switch_sheet(previous, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewSheet, window, cx| this.add_sheet(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleDiagnostics, _, cx| {
                this.diagnostics = !this.diagnostics;
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| {
                let mode = if cx.theme().mode.is_dark() {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                Theme::change(mode, Some(window), cx);
            }))
            .child(toolbar::render(&self.active_style, cx))
            .child(self.render_formula_bar(cx))
            .child(div().flex_1().min_h_0().child(self.grid.clone()))
            .child(self.render_tabs(cx))
            .child(self.render_status(cx))
    }
}
