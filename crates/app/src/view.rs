use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use zenkai_engine::{Copied, Engine, EngineError, Opened, Workbook, open_xlsx, save_xlsx_atomic};
use zenkai_formats::{Delimiter, parse_csv};
use zenkai_grid::{
    CycleReference, DeleteForward, Direction, EditMode, Grid, GridEvent, Layout, SheetView,
};
use zenkai_types::{
    BorderPreset, CellPos, CellStyle, ColIdx, HAlign, NumberFormat, Range, Rgb, SheetId,
    StyleChange,
};

use crate::actions::*;
use crate::chart::{self, ChartKind};
use crate::chart_panel::{self, ChartPanel};
use crate::clipboard;
use crate::csv_preview::{self, CsvPreview};
use crate::decimals;
use crate::document::{self, Document};
use crate::files;
use crate::find::{self, FindBar, FindResults};
use crate::format_dialog::{self, FormatDialog};
use crate::jump::jump_target;
use crate::palette;
use crate::recent;
use crate::recovery;
use crate::region;
use crate::stats::{self, SelectionStats};
use crate::theme;
use crate::toolbar;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::command::{Command, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu};

struct FormulaBarEdit {
    input: Entity<InputState>,
    pos: CellPos,
    // The edit only lands where it started: same document, same sheet.
    sheet: SheetId,
    generation: u64,
    original: String,
    _events: Subscription,
}

#[derive(Clone)]
struct InternalClip {
    copied: Copied,
    cut: bool,
    range: Range,
    sheet: SheetId,
}

#[derive(Clone, Copy)]
enum StructureEdit {
    InsertRows,
    DeleteRows,
    InsertColumns,
    DeleteColumns,
    FreezePanes,
}

const MAX_AUTOSUM_SCAN: usize = 10_000;
const MAX_AUTOFIT_CELLS: usize = 100_000;
const AUTOFIT_CHAR_WIDTH: f32 = 7.5;

const UI_SCALE_STEP: f32 = 0.125;
const MAX_REPLACE_CELLS: usize = 100_000;
const FONT_SIZES: [u16; 16] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 22, 24, 26, 28, 36, 48, 72];
const IMPORTING: &str = "Importing…";
const SAVING: &str = "Saving…";
const CALCULATING: &str = "Calculating…";
const SEARCHING: &str = "Searching…";

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
    clipboard_source: Option<InternalClip>,
    chart: Option<ChartPanel>,
    find: Option<FindBar>,
    palette: Option<Entity<CommandState>>,
    csv_preview: Option<CsvPreview>,
    // Bumped by every CSV read, re-parse, import and cancel; a background result is
    // applied only if no newer request started meanwhile.
    csv_request: u64,
    // Interface scale, independent of the grid zoom: everything sized in rems.
    ui_scale: f32,
    show_formulas: bool,
    formula_bar: Option<FormulaBarEdit>,
    // The last formatting change, which F4 repeats on the selection as in Excel.
    last_style: Option<StyleChange>,
    recent: Vec<PathBuf>,
    recent_saves: Arc<AtomicU64>,
    format_dialog: Option<FormatDialog>,
    colors: toolbar::ColorPickers,
    focus: FocusHandle,
    session_lock: Option<recovery::SessionLock>,
    rename: Option<(Entity<InputState>, Subscription)>,
    go_to: Option<(Entity<InputState>, Subscription)>,
    pending_sheet: Option<SheetId>,
    last_tab_click: Option<(Instant, SheetId)>,
    memory_mb: u64,
    diagnostics_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Workspace {
        let grid = cx.new(Grid::new);
        let subscription = cx.subscribe_in(&grid, window, Self::on_grid_event);
        let colors = toolbar::ColorPickers {
            font: cx.new(|cx| ColorPickerState::new(window, cx)),
            fill: cx.new(|cx| ColorPickerState::new(window, cx)),
        };
        let font_color = cx.subscribe_in(
            &colors.font,
            window,
            |this, _, ColorPickerEvent::Change(color), window, cx| {
                this.style(StyleChange::FontColor(color.map(rgb_of)), window, cx);
            },
        );
        let fill_color = cx.subscribe_in(
            &colors.fill,
            window,
            |this, _, ColorPickerEvent::Change(color), window, cx| {
                this.style(StyleChange::Fill(color.map(rgb_of)), window, cx);
            },
        );
        theme::follow_system(window, cx);
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_system(window, cx);
        });
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
            chart: None,
            find: None,
            palette: None,
            csv_preview: None,
            csv_request: 0,
            ui_scale: 1.0,
            show_formulas: false,
            formula_bar: None,
            last_style: None,
            recent: Vec::new(),
            recent_saves: Arc::new(AtomicU64::new(0)),
            format_dialog: None,
            colors,
            focus: cx.focus_handle(),
            session_lock: None,
            rename: None,
            go_to: None,
            pending_sheet: None,
            last_tab_click: None,
            memory_mb: 0,
            diagnostics_task: None,
            _subscriptions: vec![subscription, appearance, font_color, fill_color],
        };
        workspace.reset_grid(window, cx);
        if let Some(path) = initial {
            workspace.open_path(path, window, cx);
        }
        workspace.start_autosave(window, cx);
        workspace.load_recent(cx);
        workspace
    }

    fn reset_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_sheet_view(cx);
        self.refresh_stats(cx);
        window.set_window_title(&self.document.title());
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn load_sheet_view(&mut self, cx: &mut Context<Self>) {
        self.forget_find_results();
        let view = self.sheet_view();
        self.grid.update(cx, |grid, cx| grid.reset(view, cx));
        self.refresh_cells(cx);
    }

    fn sheet_view(&self) -> SheetView {
        let sheet = self.document.sheet;
        self.document
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
            .unwrap_or_default()
    }

    fn refresh_cells(&mut self, cx: &mut Context<Self>) {
        let ranges = self.grid.read(cx).visible_ranges();
        let mut cells = HashMap::new();
        for range in ranges {
            cells.extend(self.document.cells(range, self.show_formulas));
        }
        self.grid.update(cx, |grid, cx| grid.set_cells(cells, cx));
    }

    fn refresh_chart(&mut self) {
        if let (Some(panel), Some(wb)) = (&mut self.chart, self.document.workbook()) {
            panel.refresh(wb);
        }
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self
            .palette
            .as_ref()
            .is_some_and(|state| state.focus_handle(cx).contains_focused(window, cx));
        if focused {
            self.close_palette(window, cx);
            return;
        }
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |state, cx| state.focus(window, cx));
        self.palette = Some(state);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn render_palette(&self) -> Option<impl IntoElement> {
        let state = self.palette.as_ref()?;
        Some(
            div()
                .absolute()
                .top(px(72.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div().w(px(560.0)).shadow_lg().child(
                        palette::groups(&self.recent)
                            .into_iter()
                            .fold(Command::new(state), Command::group)
                            .placeholder("Type a command")
                            .bordered(true)
                            .max_h(px(420.0))
                            .on_confirm(|_, window, cx| {
                                window.dispatch_action(Box::new(ClosePalette), cx)
                            })
                            .on_cancel(|window, cx| {
                                window.dispatch_action(Box::new(ClosePalette), cx)
                            }),
                    ),
                ),
        )
    }

    fn forget_find_results(&mut self) {
        if let Some(bar) = &mut self.find {
            bar.results = FindResults::default();
        }
    }

    fn sample_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let memory =
                    memory_stats::memory_stats().map_or(0, |m| m.physical_mem / 1024 / 1024);
                let update = this.update(cx, |this, cx| {
                    this.memory_mb = u64::try_from(memory).unwrap_or(u64::MAX);
                    cx.notify();
                });
                if update.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        }));
    }

    fn start_autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = recovery::directory() else {
            tracing::warn!("no data directory for recovery files; autosave is off");
            return;
        };
        let own = recovery::own_file(&directory);
        match recovery::lock_session(&directory) {
            Ok(lock) => self.session_lock = Some(lock),
            Err(error) => {
                tracing::warn!(%error, "could not lock the recovery session; autosave is off");
                self.notify(
                    Severity::Warning,
                    format!("Autosave is off, the recovery folder is not usable: {error}"),
                    cx,
                );
                return;
            }
        }
        let on_quit_file = own.clone();
        let quit = cx.on_app_quit(move |_, _| {
            recovery::remove_with_lock(&on_quit_file);
            async {}
        });
        self._subscriptions.push(quit);
        let leftovers = recovery::leftovers(&directory);
        let own_for_recovery = own.clone();
        cx.spawn_in(window, async move |this, cx| {
            if !leftovers.is_empty()
                && let Err(error) = this.update_in(cx, |this, window, cx| {
                    this.offer_recovery(leftovers, own_for_recovery, window, cx)
                })
            {
                tracing::debug!(%error, "workspace closed before recovery");
            }
            loop {
                cx.background_executor()
                    .timer(recovery::AUTOSAVE_EVERY)
                    .await;
                let own = own.clone();
                if this.update(cx, |this, cx| this.autosave(own, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn autosave(&mut self, own: PathBuf, cx: &mut Context<Self>) {
        if !self.document.dirty {
            recovery::remove(&own);
            return;
        }
        if self.document.has_pending() {
            return;
        }
        let Some(workbook) = self.document.take() else {
            return;
        };
        let generation = self.document.generation();
        cx.spawn(async move |this, cx| {
            let target = own.clone();
            let (workbook, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = recovery::write(&workbook, &target);
                    (workbook, result)
                })
                .await;
            let update = this.update(cx, |this, cx| {
                if !this.document.restore(workbook, generation) {
                    return;
                }
                if let Err(error) = result {
                    tracing::warn!(%error, "autosave failed");
                    this.notify(
                        Severity::Warning,
                        format!("Autosave failed, recovery is not protecting this work: {error}"),
                        cx,
                    );
                }
                this.flush_edits(cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during autosave");
            }
        })
        .detach();
    }

    fn offer_recovery(
        &mut self,
        leftovers: Vec<PathBuf>,
        own: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = format!(
            "Zenkai closed unexpectedly and kept {} unsaved workbook(s). Open the most recent one?",
            leftovers.len()
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "Recover unsaved work?",
            Some(&detail),
            &["Open recovered", "Discard"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else {
                return;
            };
            if choice == 1 {
                leftovers
                    .iter()
                    .for_each(|path| recovery::remove_with_lock(path));
                return;
            }
            let Some(newest) = leftovers
                .iter()
                .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
            else {
                return;
            };
            let path = newest.clone();
            let update = this.update_in(cx, |this, window, cx| {
                this.confirm_discard(window, cx, move |this, window, cx| {
                    this.load_recovered(path, own, window, cx)
                })
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during recovery");
            }
        })
        .detach();
    }

    fn load_recovered(
        &mut self,
        path: PathBuf,
        own: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move { open_xlsx(&path).map(|opened| (opened, path)) })
                .await;
            let update = this.update_in(cx, |this, window, cx| match opened {
                Ok((opened, path)) => {
                    this.document = Document::new(opened.workbook, None, opened.unsupported);
                    this.document.dirty = true;
                    this.reset_grid(window, cx);
                    // Kept as this session's own recovery copy until the work is saved.
                    if let Err(error) = std::fs::rename(&path, &own) {
                        tracing::warn!(?path, %error, "could not adopt the recovery file");
                    }
                    recovery::remove(&path.with_extension("lock"));
                    this.notify(Severity::Warning, "Recovered work. Save it to keep it.", cx);
                }
                Err(error) => {
                    this.notify(Severity::Error, format!("Could not recover: {error}"), cx)
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during recovery");
            }
        })
        .detach();
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        let input = match &self.find {
            Some(bar) => bar.input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in sheet"));
                let subscription = cx.subscribe_in(&input, window, Self::on_find_event);
                self._subscriptions.push(subscription);
                self.find = Some(FindBar {
                    input: input.clone(),
                    replace: None,
                    results: FindResults::default(),
                });
                input
            }
        };
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn open_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(window, cx);
        let Some(bar) = &mut self.find else {
            return;
        };
        if bar.replace.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Replace with"));
            let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.replace_all(window, cx);
                }
            });
            self._subscriptions.push(subscription);
            bar.replace = Some(input);
        }
        cx.notify();
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = &self.find else {
            return;
        };
        let query = bar.input.read(cx).value().to_string();
        let replacement = bar
            .replace
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        if query.is_empty() {
            return;
        }
        let sheet = self.document.sheet;
        // The scan reads every filled cell, so it runs with the edit, off the UI thread.
        self.edit(window, cx, move |wb| {
            let changes = find::replacements(wb, sheet, &query, &replacement);
            if changes.is_empty() {
                return Err(EngineError::Rejected(format!(
                    "no cell contains \"{query}\""
                )));
            }
            // Every replaced cell is one undo entry, so huge replacements are refused
            // whole rather than cut short.
            if changes.len() > MAX_REPLACE_CELLS {
                return Err(EngineError::Rejected(format!(
                    "{} cells match; Replace All handles up to {MAX_REPLACE_CELLS} at once",
                    changes.len()
                )));
            }
            wb.set_scattered_inputs(sheet, &changes)
        });
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn on_find_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InputEvent::PressEnter { shift, .. } = event else {
            return;
        };
        let query = input.read(cx).value().to_string();
        let Some(bar) = &mut self.find else {
            return;
        };
        if bar.results.query == query {
            if let Some(pos) = bar.results.step(*shift) {
                self.grid.update(cx, |grid, cx| grid.select(pos, pos, cx));
            }
            cx.notify();
            return;
        }
        self.run_find(query, cx);
    }

    fn run_find(&mut self, query: String, cx: &mut Context<Self>) {
        if self.document.has_pending() {
            self.notify(
                Severity::Warning,
                "Still calculating, try again in a moment.",
                cx,
            );
            return;
        }
        let Some(workbook) = self.document.take() else {
            self.notify(
                Severity::Warning,
                "Still calculating, try again in a moment.",
                cx,
            );
            return;
        };
        let (sheet, generation) = (self.document.sheet, self.document.generation());
        self.busy = Some(SEARCHING.into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let task_query = query.clone();
            let (workbook, matches) = cx
                .background_executor()
                .spawn(async move {
                    let matches = find::search(&workbook, sheet, &task_query);
                    (workbook, matches)
                })
                .await;
            let update = this.update(cx, |this, cx| {
                this.clear_busy(SEARCHING, cx);
                if !this.document.restore(workbook, generation) {
                    return;
                }
                if let Some(bar) = &mut this.find {
                    bar.results = FindResults {
                        query,
                        matches,
                        current: 0,
                    };
                    if let Some(pos) = bar.results.matches.first().copied() {
                        this.grid.update(cx, |grid, cx| grid.select(pos, pos, cx));
                    }
                }
                this.flush_edits(cx);
                cx.notify();
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during search");
            }
        })
        .detach();
    }

    fn render_find(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bar = self.find.as_ref()?;
        let theme = cx.theme();
        Some(
            h_flex()
                .key_context("FindBar")
                .h(px(36.0))
                .px_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(div().w(px(260.0)).child(Input::new(&bar.input)))
                .children(bar.replace.as_ref().map(|replace| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(220.0)).child(Input::new(replace)))
                        .child(
                            Button::new("replace-all")
                                .small()
                                .label("Replace all")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.replace_all(window, cx)),
                                ),
                        )
                }))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(bar.results.status()),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("close-find")
                        .px_2()
                        .cursor_pointer()
                        .text_color(theme.muted_foreground)
                        .child("✕")
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                ),
        )
    }

    fn insert_chart(&mut self, cx: &mut Context<Self>) {
        let Some(workbook) = self.document.workbook() else {
            return;
        };
        let source = self.grid.read(cx).selection().range();
        self.chart = Some(ChartPanel::new(workbook, self.document.sheet, source));
        cx.notify();
    }

    fn set_chart_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.chart {
            panel.kind = kind;
            cx.notify();
        }
    }

    fn copy_chart_mermaid(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &self.chart {
            let text = chart::to_mermaid(&panel.data, panel.kind);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.notify(Severity::Info, "Chart copied as Mermaid", cx);
        }
    }

    fn export_chart_svg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.chart else {
            return;
        };
        let svg = chart::to_svg(&panel.data, panel.kind);
        let directory = self
            .document
            .path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let path = cx.prompt_for_new_path(&directory, Some("chart.svg"));
        cx.spawn_in(window, async move |this, cx| {
            let chosen = match path.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "export dialog failed");
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "export dialog dropped");
                    return;
                }
            };
            let target = chosen.clone();
            let written = cx
                .background_executor()
                .spawn(
                    async move { zenkai_engine::write_atomic(&target, svg.as_bytes(), |_| Ok(())) },
                )
                .await;
            let update = this.update(cx, |this, cx| match written {
                Ok(()) => this.notify(
                    Severity::Info,
                    format!("Chart saved to {}", chosen.display()),
                    cx,
                ),
                Err(error) => this.notify(
                    Severity::Error,
                    format!("Could not save the chart: {error}"),
                    cx,
                ),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during chart export");
            }
        })
        .detach();
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        let selection = self.grid.read(cx).selection();
        let sheet = self.document.sheet;
        if let Some(wb) = self.document.workbook() {
            self.stats = stats::compute(wb, sheet, selection.range());
            self.active_input = wb.input(sheet, selection.active).into();
            self.active_style = wb.cell(sheet, selection.active).style;
            let formula = self.active_input.clone();
            let formula = if formula.starts_with('=') {
                formula
            } else {
                SharedString::default()
            };
            self.grid
                .update(cx, |grid, _| grid.set_active_formula(formula));
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
                self.commit_text(self.document.sheet, *pos, text.clone(), window, cx)
            }
            GridEvent::CommitToSelection { pos, text, range } => {
                let (sheet, pos, text, range) = (self.document.sheet, *pos, text.clone(), *range);
                self.edit(window, cx, move |wb| wb.fill_with(sheet, pos, &text, range));
            }
            GridEvent::ClearRequested(range) => {
                let (sheet, range) = (self.document.sheet, *range);
                self.edit(window, cx, move |wb| wb.clear(sheet, range));
            }
            GridEvent::Jump { direction, extend } => self.jump(*direction, *extend, cx),
            GridEvent::ColumnResized { col, width } => {
                let (sheet, col, width) = (self.document.sheet, *col, *width);
                self.edit(window, cx, move |wb| wb.set_column_width(sheet, col, width));
            }
            GridEvent::RowResized { row, height } => {
                let (sheet, row, height) = (self.document.sheet, *row, *height);
                self.edit(window, cx, move |wb| wb.set_row_height(sheet, row, height));
            }
            GridEvent::FillRequested { source, target } => {
                let (sheet, source, target) = (self.document.sheet, *source, *target);
                self.edit(window, cx, move |wb| wb.extend(sheet, source, target));
            }
            GridEvent::AutoFitRequested(col) => self.auto_fit(*col, window, cx),
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
        // Any edit ends copy mode, as in Excel, so a later paste never reads cells or
        // sheets that changed since the copy.
        if self.clipboard_source.take().is_some() {
            self.grid.update(cx, |grid, cx| grid.set_marquee(None, cx));
        }
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
                let sheets_before = this.document.sheets.clone();
                let sheet_before = this.document.sheet;
                let pending_sheet = this.pending_sheet.take();
                if !this.document.restore(workbook, generation) {
                    this.clear_busy(CALCULATING, cx);
                    return;
                }
                if let Some(target) = pending_sheet
                    && errors.is_empty()
                {
                    this.document.sheet = target;
                }
                let sheet_changed = this.document.sheet != sheet_before
                    || this.document.sheets.len() != sheets_before.len();
                if sheet_changed {
                    this.load_sheet_view(cx);
                } else {
                    // Edits can insert rows, resize, merge or freeze; refresh the layout
                    // without moving the selection.
                    let view = this.sheet_view();
                    this.grid.update(cx, |grid, cx| grid.update_view(view, cx));
                }
                this.last_recalc = Some(started.elapsed());
                this.clear_busy(CALCULATING, cx);
                if let Some(error) = errors.first() {
                    this.notify(Severity::Error, error.to_string(), cx);
                }
                this.refresh_cells(cx);
                this.refresh_stats(cx);
                this.refresh_chart();
                this.forget_find_results();
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

    fn import_csv(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = Some(format!("Reading {}…", path.display()).into());
        cx.notify();
        let label = self.busy.clone().unwrap_or_default();
        let request = self.next_csv_request();
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    files::read_csv(&task_path).map(|(bytes, parsed)| {
                        let guess = csv_preview::guess(&parsed);
                        (bytes, parsed, guess)
                    })
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(&label, cx);
                if this.csv_request != request {
                    return;
                }
                match result {
                    Ok((bytes, parsed, guess)) => {
                        this.csv_preview = Some(CsvPreview {
                            path,
                            bytes: Arc::new(bytes),
                            parsed,
                            guess,
                        });
                        window.focus(&this.focus, cx);
                        cx.notify();
                    }
                    Err(error) => this.notify(Severity::Error, error, cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while reading a CSV");
            }
        })
        .detach();
    }

    fn reparse_csv(&mut self, delimiter: Delimiter, cx: &mut Context<Self>) {
        let Some(preview) = &self.csv_preview else {
            return;
        };
        let bytes = preview.bytes.clone();
        let request = self.next_csv_request();
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    parse_csv(&bytes, Some(delimiter)).map(|parsed| {
                        let guess = csv_preview::guess(&parsed);
                        (parsed, guess)
                    })
                })
                .await;
            let update = this.update(cx, |this, cx| match parsed {
                _ if this.csv_request != request => {}
                Ok((parsed, guess)) => {
                    if let Some(preview) = &mut this.csv_preview {
                        preview.guess = guess;
                        preview.parsed = parsed;
                    }
                    cx.notify();
                }
                Err(error) => this.notify(Severity::Error, error.to_string(), cx),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while parsing a CSV");
            }
        })
        .detach();
    }

    fn confirm_csv_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.csv_preview.take() else {
            return;
        };
        let request = self.next_csv_request();
        let summary = format!(
            "Imported {} ({}, {}). Save to keep it as .xlsx.",
            preview
                .path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            preview.parsed.delimiter.label(),
            preview.parsed.encoding.label()
        );
        self.busy = Some(IMPORTING.into());
        cx.notify();
        let CsvPreview {
            path,
            bytes,
            parsed,
            guess,
        } = preview;
        let delimiter = parsed.delimiter;
        let mut rows = parsed.rows;
        cx.spawn_in(window, async move |this, cx| {
            let task_bytes = bytes.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    csv_preview::apply(guess, &mut rows);
                    // On failure, parse again so the preview comes back as it was.
                    files::workbook_from_rows(rows)
                        .map_err(|error| (error, parse_csv(&task_bytes, Some(delimiter)).ok()))
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(IMPORTING, cx);
                match result {
                    Ok(workbook) => {
                        this.document = Document::new(workbook, None, Vec::new());
                        this.reset_grid(window, cx);
                        this.notify(Severity::Info, summary, cx);
                    }
                    Err((error, parsed)) => {
                        if let Some(parsed) = parsed
                            && this.csv_request == request
                        {
                            this.csv_preview = Some(CsvPreview {
                                path,
                                bytes,
                                parsed,
                                guess,
                            });
                            window.focus(&this.focus, cx);
                        }
                        this.notify(Severity::Error, error, cx);
                    }
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during import");
            }
        })
        .detach();
    }

    fn set_ui_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        self.ui_scale = scale.clamp(0.75, 2.0);
        self.notify(
            Severity::Info,
            format!("Interface size {:.0}%", self.ui_scale * 100.0),
            cx,
        );
        cx.notify();
    }

    fn next_csv_request(&mut self) -> u64 {
        self.csv_request += 1;
        self.csv_request
    }

    fn render_csv_preview(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let preview = self.csv_preview.as_ref()?;
        let entity = cx.entity().downgrade();
        let on_event = move |event: csv_preview::PreviewEvent, _: &mut Window, cx: &mut App| {
            let result = entity.update(cx, |this, cx| match event {
                csv_preview::PreviewEvent::Delimiter(delimiter) => this.reparse_csv(delimiter, cx),
                csv_preview::PreviewEvent::DecimalComma(comma) => {
                    if let Some(preview) = &mut this.csv_preview {
                        preview.guess.decimal_comma = comma;
                        cx.notify();
                    }
                }
                csv_preview::PreviewEvent::DateOrder(order) => {
                    if let Some(preview) = &mut this.csv_preview {
                        preview.guess.date_order = order;
                        cx.notify();
                    }
                }
            });
            if let Err(error) = result {
                tracing::debug!(%error, "workspace dropped");
            }
        };
        Some(
            div()
                .absolute()
                .top(px(96.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(csv_preview::render(preview, &self.focus, on_event, cx)),
        )
    }

    fn export_csv(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbook) = self.document.workbook() else {
            self.notify(
                Severity::Warning,
                "Still calculating, try again in a moment.",
                cx,
            );
            return;
        };
        let rows = files::sheet_rows(workbook, self.document.sheet);
        let sheets = self.document.sheets.len();
        cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move { files::write_csv_file(&target, &rows) })
                .await;
            let update = this.update(cx, |this, cx| match written {
                Ok(()) if sheets > 1 => this.notify(
                    Severity::Warning,
                    format!("Saved the active sheet only to {}", path.display()),
                    cx,
                ),
                Ok(()) => this.notify(Severity::Info, format!("Saved {}", path.display()), cx),
                Err(error) => this.notify(Severity::Error, error, cx),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during export");
            }
        })
        .detach();
    }

    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if files::is_delimited_text(&path) {
            self.import_csv(path, window, cx);
            return;
        }
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
                        this.remember_recent(&path, cx);
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
                    Err(EngineError::InvalidFile(reason)) => {
                        this.open_values(path, reason, window, cx)
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

    fn confirm_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !self.document.dirty {
            then(self, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            Some("The current workbook has changes that are not saved."),
            &["Cancel", "Discard"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1)
                && let Err(error) = this.update_in(cx, |this, window, cx| then(this, window, cx))
            {
                tracing::debug!(%error, "workspace closed during discard prompt");
            }
        })
        .detach();
    }

    fn open_values(
        &mut self,
        path: PathBuf,
        reason: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = Some(format!("Reading values from {}…", path.display()).into());
        let label = self.busy.clone().unwrap_or_default();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { files::open_values(&task_path) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(&label, cx);
                match result {
                    Ok(workbook) => {
                        this.document = Document::new(workbook, Some(path), Vec::new());
                        this.document.read_only = true;
                        this.reset_grid(window, cx);
                        this.notify(
                            Severity::Warning,
                            "Opened read-only: values only, without formulas or formatting. Save As keeps a copy.",
                            cx,
                        );
                    }
                    Err(fallback) => this.notify(
                        Severity::Error,
                        format!("The file could not be opened: {reason}. Reading its values also failed: {fallback}"),
                        cx,
                    ),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during read-only open");
            }
        })
        .detach();
    }

    // The list is read and written off the UI thread; a failure only loses the list.
    fn load_recent(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let recent = cx
                .background_executor()
                .spawn(async { recent::load() })
                .await;
            if let Err(error) = this.update(cx, |this, cx| {
                // Files opened while the list was loading stay in front.
                let opened_meanwhile = !this.recent.is_empty();
                this.recent = this
                    .recent
                    .iter()
                    .rev()
                    .fold(recent, |list, path| recent::with(&list, path));
                if opened_meanwhile {
                    this.save_recent(cx);
                }
                cx.notify();
            }) {
                tracing::debug!(%error, "workspace closed while loading recent files");
            }
        })
        .detach();
    }

    fn remember_recent(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.recent = recent::with(&self.recent, path);
        self.save_recent(cx);
    }

    // Saves can finish out of order; each carries a generation and only the newest
    // one is written.
    fn save_recent(&mut self, cx: &mut Context<Self>) {
        let generation = self.recent_saves.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = self.recent_saves.clone();
        let list = self.recent.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = recent::save(&list, generation, &latest) {
                    tracing::warn!(%error, "could not save the recent files list");
                }
            })
            .detach();
    }

    fn open_recent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.recent.get(index).cloned() else {
            return;
        };
        self.confirm_discard(window, cx, move |this, window, cx| {
            this.open_path(path, window, cx)
        });
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, Self::pick_and_open);
    }

    fn pick_and_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        // Text typed in the formula bar is entered before saving, as in Excel.
        self.close_formula_bar(true, window, cx);
        let must_rename = !self.document.unsupported.is_empty()
            || self.document.is_macro_enabled()
            || self.document.read_only;
        match self.document.path.clone() {
            Some(path) if !must_rename => self.save_to(path, window, cx),
            Some(_) => {
                let detail = if self.document.read_only {
                    "This file was opened read-only with its values only. Save a copy with a new name; the original is never overwritten.".to_string()
                } else if self.document.is_macro_enabled() {
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
        self.close_formula_bar(true, window, cx);
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
        if files::is_delimited_text(&chosen) {
            self.export_csv(chosen, window, cx);
            return;
        }
        let path = files::xlsx_target(&chosen);
        let renamed = path != chosen;
        let replaces_source = self
            .document
            .path
            .as_ref()
            .is_some_and(|source| files::same_file(source, &path));
        let (title, detail) =
            if replaces_source && (self.document.is_macro_enabled() || self.document.read_only) {
                self.notify(
                    Severity::Error,
                    "This original is never overwritten. Choose a new name.",
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
                    Err(error) => {
                        this.notify(
                            Severity::Error,
                            format!("{error}. Pick another location to keep your changes."),
                            cx,
                        );
                        this.flush_edits(cx);
                        // Locked by another program or read-only: offer Save As right away,
                        // unless this already was a new location picked in Save As.
                        if this.document.path.as_ref() == Some(&path) {
                            this.save_as(&SaveAs, window, cx);
                        }
                        return;
                    }
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
        self.confirm_discard(window, cx, Self::replace_with_empty);
    }

    fn replace_with_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.last_style = Some(change);
        let (sheet, range) = (self.document.sheet, self.selection(cx));
        self.edit(window, cx, move |wb| {
            wb.apply_style(sheet, range, change)?;
            if matches!(change, StyleChange::NumberFormat(_)) {
                widen_for_numbers(wb, sheet, range)?;
            }
            Ok(())
        });
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

    // Ctrl+Shift+> and < move through Excel's font size list from the active cell's size.
    fn step_font_size(&mut self, grow: bool, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.grid.read(cx).selection().active;
        let current = self
            .document
            .workbook()
            .and_then(|wb| wb.cell(self.document.sheet, active).style.font_size)
            .unwrap_or(11.0);
        let next = if grow {
            FONT_SIZES.iter().find(|size| f32::from(**size) > current)
        } else {
            FONT_SIZES
                .iter()
                .rev()
                .find(|size| f32::from(**size) < current)
        };
        if let Some(size) = next {
            self.style(StyleChange::FontSize(*size), window, cx);
        }
    }

    fn copy(&mut self, cut: bool, cx: &mut Context<Self>) {
        let range = self.selection(cx);
        let sheet = self.document.sheet;
        let Some(workbook) = self.document.workbook_mut() else {
            self.notify(
                Severity::Warning,
                "Still calculating, try again in a moment.",
                cx,
            );
            return;
        };
        let end = workbook.used_end(sheet);
        let clipped = range.clip_to(end).unwrap_or(Range::single(range.start));
        if clipped.cell_count() > clipboard::MAX_COPY_CELLS {
            self.notify(Severity::Warning, "The selection is too large to copy.", cx);
            return;
        }
        match workbook.copy(sheet, clipped) {
            Ok(copied) => {
                cx.write_to_clipboard(ClipboardItem::new_string(copied.text.clone()));
                let sheet = self.document.sheet;
                self.clipboard_source = Some(InternalClip {
                    copied,
                    cut,
                    range,
                    sheet,
                });
                self.grid
                    .update(cx, |grid, cx| grid.set_marquee(Some(range), cx));
            }
            Err(error) => self.notify(Severity::Error, error.to_string(), cx),
        }
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let sheet = self.document.sheet;
        let origin = self.grid.read(cx).selection().active;
        let internal = self
            .clipboard_source
            .take_if(|clip| clip.copied.text == text);
        let (height, width) = match &internal {
            Some(clip) => (clip.range.rows(), clip.range.cols()),
            None => {
                let rows = clipboard::parse_tsv(&text);
                let height = u32::try_from(rows.len()).unwrap_or(u32::MAX);
                let width =
                    u16::try_from(rows.iter().map(Vec::len).max().unwrap_or(0)).unwrap_or(u16::MAX);
                self.edit(window, cx, move |wb| wb.set_inputs(sheet, origin, &rows));
                (height, width)
            }
        };
        if let Some(clip) = internal {
            let keep = (!clip.cut).then(|| clip.clone());
            self.edit(window, cx, move |wb| {
                wb.paste(sheet, origin, &clip.copied, clip.cut)
            });
            // Pasting a copy keeps copy mode, so the same cells can be pasted again.
            self.clipboard_source = keep;
        }
        let end = CellPos::new(
            origin.row.offset(i64::from(height.saturating_sub(1))),
            origin.col.offset(i64::from(width.saturating_sub(1))),
        );
        self.grid.update(cx, |grid, cx| {
            grid.set_marquee(None, cx);
            grid.select(origin, end, cx);
        });
    }

    // Excel's Paste Values: the values shown in the copied cells, without formulas or
    // formats; works for copies from Zenkai and from other programs alike.
    fn paste_values(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let sheet = self.document.sheet;
        let origin = self.grid.read(cx).selection().active;
        // A copy made here pastes the cells' own values (full precision, dates as serial
        // numbers, as Excel does); text from other programs is pasted as shown.
        let source = self
            .clipboard_source
            .as_ref()
            .filter(|clip| clip.copied.text == text)
            .map(|clip| (clip.sheet, clip.range));
        let (height, width) = match source {
            Some((_, range)) => (i64::from(range.rows()), i64::from(range.cols())),
            None => {
                let rows = clipboard::values_only(&text);
                (
                    i64::try_from(rows.len()).unwrap_or(i64::MAX),
                    i64::try_from(rows.iter().map(Vec::len).max().unwrap_or(0)).unwrap_or(i64::MAX),
                )
            }
        };
        // A cut ends with its first paste, as in Excel; a copy stays ready.
        let keep = self.clipboard_source.clone().filter(|clip| !clip.cut);
        self.edit(window, cx, move |wb| {
            let rows = match source {
                Some((from, range)) => clipboard::cell_values(wb, from, range),
                None => clipboard::values_only(&text),
            };
            wb.set_inputs(sheet, origin, &rows)
        });
        self.clipboard_source = keep;
        let end = CellPos::new(
            origin.row.offset(height.saturating_sub(1)),
            origin.col.offset(width.saturating_sub(1)),
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
        h_flex()
            .h(px(32.0))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(match &self.go_to {
                Some((input, _)) => div()
                    .key_context("NameBox")
                    .w(px(120.0))
                    .child(Input::new(input))
                    .into_any_element(),
                None => div()
                    .id("name-box")
                    .role(Role::Button)
                    .aria_label(format!("Name box, {name}, press to go to a cell"))
                    .w(px(96.0))
                    .h(px(24.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_sm()
                    .text_sm()
                    .cursor_text()
                    .on_click(cx.listener(|this, _, window, cx| this.open_go_to(window, cx)))
                    .child(name)
                    .into_any_element(),
            })
            .child(
                div()
                    .text_sm()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .px_1()
                    .child("fx"),
            )
            .children(self.formula_bar.as_ref().map(|bar| {
                div()
                    .id("formula-bar-edit")
                    .key_context("FormulaBar")
                    .role(Role::Group)
                    .aria_label("Formula bar")
                    .flex_1()
                    .child(Input::new(&bar.input))
            }))
            .when(self.formula_bar.is_none(), |row| {
                row.child(
                    div()
                        .id("formula-content")
                        .role(Role::TextInput)
                        .aria_label("Formula bar")
                        .aria_value(content.clone())
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
                        .on_click(cx.listener(|this, _, window, cx| {
                            // While the cell is being edited the bar shows that text; a click
                            // then keeps editing in the cell.
                            if this.grid.read(cx).editor().is_some() {
                                let focus = this.grid.focus_handle(cx);
                                window.focus(&focus, cx);
                                return;
                            }
                            this.open_formula_bar(window, cx);
                        }))
                        .child(content),
                )
            })
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entity = cx.entity().downgrade();
        let tabs = self.document.sheets.iter().map(|sheet| {
            let entity = entity.clone();
            let id = sheet.id;
            // Excel activates a tab on right click, so its menu acts on that sheet.
            Tab::new().label(sheet.name.clone()).on_mouse_down(
                MouseButton::Right,
                move |_, window, cx| {
                    if let Err(error) =
                        entity.update(cx, |this, cx| this.switch_sheet(id, window, cx))
                    {
                        tracing::debug!(%error, "workspace dropped");
                    }
                },
            )
        });
        h_flex()
            .h(px(32.0))
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(
                div()
                    .id("sheet-tabs")
                    .child(
                        TabBar::new("sheets")
                            .children(tabs)
                            .selected_index(self.document.sheet.0 as usize)
                            .on_click(move |index, window, cx| {
                                let sheet = SheetId(u32::try_from(*index).unwrap_or(0));
                                if let Err(error) = entity.update(cx, |this, cx| {
                                    // A second click on the active tab within the double-click time renames it.
                                    let now = Instant::now();
                                    let double = this.last_tab_click.is_some_and(|(at, tab)| {
                                        tab == sheet
                                            && now.duration_since(at) < Duration::from_millis(450)
                                    });
                                    this.last_tab_click = Some((now, sheet));
                                    if double && sheet == this.document.sheet {
                                        this.open_rename(window, cx);
                                    } else {
                                        this.switch_sheet(sheet, window, cx);
                                    }
                                }) {
                                    tracing::debug!(%error, "workspace dropped");
                                }
                            }),
                    )
                    .context_menu({
                        let grid_focus = self.grid.focus_handle(cx);
                        move |menu, _, _| sheet_menu(menu, grid_focus.clone())
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

    fn structure_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: StructureEdit,
    ) {
        let sheet = self.document.sheet;
        let selection = self.selection(cx);
        let active = self.grid.read(cx).selection().active;
        let rows = selection.rows();
        let cols = selection.cols();
        let whole_sheet_rows = rows == zenkai_types::MAX_ROWS;
        let whole_sheet_cols = cols == zenkai_types::MAX_COLS;
        let refused = match action {
            StructureEdit::InsertRows | StructureEdit::DeleteRows => whole_sheet_rows,
            StructureEdit::InsertColumns | StructureEdit::DeleteColumns => whole_sheet_cols,
            StructureEdit::FreezePanes => false,
        };
        if refused {
            self.notify(
                Severity::Warning,
                "Select whole rows or columns, not the entire sheet, to insert or delete.",
                cx,
            );
            return;
        }
        match action {
            StructureEdit::InsertRows => self.edit(window, cx, move |wb| {
                wb.insert_rows(sheet, selection.start.row, rows)
            }),
            StructureEdit::DeleteRows => self.edit(window, cx, move |wb| {
                wb.delete_rows(sheet, selection.start.row, rows)
            }),
            StructureEdit::InsertColumns => self.edit(window, cx, move |wb| {
                wb.insert_columns(sheet, selection.start.col, cols)
            }),
            StructureEdit::DeleteColumns => self.edit(window, cx, move |wb| {
                wb.delete_columns(sheet, selection.start.col, cols)
            }),
            StructureEdit::FreezePanes => {
                let frozen = self
                    .document
                    .workbook()
                    .map(|wb| wb.frozen(sheet))
                    .unwrap_or_default();
                let (rows, cols) = if frozen == (0, 0) {
                    (active.row.get(), active.col.get())
                } else {
                    (0, 0)
                };
                let (visible_rows, visible_cols) = self.grid.read(cx).visible_counts();
                if rows >= visible_rows || cols >= visible_cols {
                    self.notify(
                        Severity::Warning,
                        "Scroll to the top-left and pick a visible cell: the rows above it and the columns to its left are frozen.",
                        cx,
                    );
                    return;
                }
                self.edit(window, cx, move |wb| wb.set_frozen(sheet, rows, cols));
            }
        }
    }

    // With a single cell selected, Excel sorts its current region.
    fn sort(&mut self, descending: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.document.sheet;
        // Taken before the region is selected, which moves the active cell to its corner.
        let key = self.grid.read(cx).selection().active.col;
        let mut range = self.selection(cx);
        if range.cell_count() == 1 {
            range = self.select_current_region(cx);
        }
        if range.rows() < 2 {
            self.notify(
                Severity::Info,
                "Nothing to sort: select rows of data first.",
                cx,
            );
            return;
        }
        self.edit(window, cx, move |wb| wb.sort(sheet, range, key, descending));
    }

    // The new format comes from the active cell, as in Excel, and applies to the selection.
    fn step_decimals(&mut self, more: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Not expressible as a repeatable StyleChange; F4 must not repeat an older one.
        self.last_style = None;
        let (sheet, range) = (self.document.sheet, self.selection(cx));
        let active = self.grid.read(cx).selection().active;
        let Some(workbook) = self.document.workbook() else {
            return;
        };
        let view = workbook.cell(sheet, active);
        let Some(code) = decimals::step_decimals(
            &view.style.num_fmt,
            (view.kind == zenkai_types::ValueKind::Number).then_some(view.text.as_str()),
            more,
        ) else {
            return;
        };
        self.edit(window, cx, move |wb| {
            wb.set_number_format(sheet, range, &code)?;
            widen_for_numbers(wb, sheet, range)
        });
    }

    fn toggle_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sheet, range) = (self.document.sheet, self.selection(cx));
        let active = self.grid.read(cx).selection().active;
        let wrap = !self
            .document
            .workbook()
            .is_some_and(|wb| wb.cell(sheet, active).style.wrap);
        self.edit(window, cx, move |wb| {
            wb.apply_style(sheet, range, StyleChange::Wrap(wrap))?;
            if !wrap || range.cell_count() > MAX_AUTOFIT_CELLS as u64 {
                return Ok(());
            }
            grow_wrapped_rows(wb, sheet, range)
        });
    }

    // The rows (or columns) the selection touches, as Excel's Hide and Unhide.
    fn hide(&mut self, rows: bool, hidden: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (sheet, range) = (self.document.sheet, self.selection(cx));
        let whole_sheet = if rows {
            range.rows() == zenkai_types::MAX_ROWS
        } else {
            range.cols() == zenkai_types::MAX_COLS
        };
        if hidden && whole_sheet {
            self.notify(
                Severity::Warning,
                "Hiding every row or column is not supported.",
                cx,
            );
            return;
        }
        self.edit(window, cx, move |wb| {
            if rows {
                wb.set_rows_hidden(sheet, range, hidden)
            } else {
                wb.set_columns_hidden(sheet, range, hidden)
            }
        });
    }

    // Excel's Ctrl+1, Number tab: categories, a custom code and a sample of the active cell.
    fn open_format_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.grid.read(cx).selection().active;
        let view = self
            .document
            .workbook()
            .map(|wb| wb.cell(self.document.sheet, active))
            .unwrap_or_default();
        let code = view.style.num_fmt.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(code));
        let refresh = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.apply_format_dialog(window, cx),
            _ => cx.notify(),
        });
        let focus = cx.focus_handle();
        let input_focus = input.focus_handle(cx);
        window.focus(&input_focus, cx);
        self.format_dialog = Some(FormatDialog {
            code: input,
            sample: view.number.unwrap_or(1234.5678),
            range: self.selection(cx),
            focus,
            _refresh: refresh,
        });
        cx.notify();
    }

    fn apply_format_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.last_style = None;
        let Some(dialog) = &self.format_dialog else {
            return;
        };
        let code = dialog.code.read(cx).value().trim().to_string();
        let range = dialog.range;
        // A fixed in-range sample: a date code is valid even when the active cell holds
        // a number no date can show, as Excel accepts it and shows ####.
        if let Err(error) = zenkai_engine::format_preview(1234.5678, &code) {
            self.notify(
                Severity::Warning,
                format!("Invalid number format: {error}"),
                cx,
            );
            return;
        }
        let sheet = self.document.sheet;
        self.edit(window, cx, move |wb| {
            wb.set_number_format(sheet, range, &code)?;
            widen_for_numbers(wb, sheet, range)
        });
        self.close_format_dialog(window, cx);
    }

    fn close_format_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.format_dialog = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn render_format_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.format_dialog.as_ref()?;
        let code = dialog.code.read(cx).value().to_string();
        let preview = zenkai_engine::format_preview(dialog.sample, &code)
            .unwrap_or_else(|_| "Invalid format".to_string());
        let input = dialog.code.clone();
        let on_category = move |code: &'static str, window: &mut Window, cx: &mut App| {
            input.update(cx, |state, cx| state.set_value(code, window, cx));
        };
        Some(
            div()
                .absolute()
                .top(px(96.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(format_dialog::render(dialog, preview, on_category, cx)),
        )
    }

    // What Enter does with typed text, from the cell or the formula bar.
    fn commit_text(
        &mut self,
        sheet: SheetId,
        pos: CellPos,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(window, cx, move |wb| {
            // Excel turns on wrap text for a cell typed with a line break.
            let wraps = text.contains('\n') && !text.starts_with('=');
            wb.set_input(sheet, pos, &text)?;
            if wraps {
                wb.apply_style(sheet, Range::single(pos), StyleChange::Wrap(true))?;
                grow_wrapped_rows(wb, sheet, Range::single(pos))?;
            }
            widen_for_numbers(wb, sheet, Range::single(pos))
        });
    }

    // Editing in the formula bar: Enter or leaving the bar enters the text in the
    // cell, Esc leaves it unchanged (Excel's behaviour without point mode).
    fn open_formula_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.formula_bar.is_some() || self.document.read_only {
            return;
        }
        let pos = self.grid.read(cx).selection().active;
        // While a recalculation has the workbook the cell's text is unknown; opening
        // with an empty bar would then clear the cell on Enter.
        let Some(original) = self
            .document
            .workbook()
            .map(|wb| wb.input(self.document.sheet, pos))
        else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(original.clone()));
        let events = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                this.close_formula_bar(true, window, cx)
            }
            _ => {}
        });
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        self.formula_bar = Some(FormulaBarEdit {
            input,
            pos,
            sheet: self.document.sheet,
            generation: self.document.generation(),
            original,
            _events: events,
        });
        cx.notify();
    }

    fn close_formula_bar(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.formula_bar.take() else {
            return;
        };
        // Switching sheets still enters the text in the sheet it was typed for, as in
        // Excel; a different document never receives it.
        if commit && bar.generation == self.document.generation() {
            let text = bar.input.read(cx).value().to_string();
            if text != bar.original {
                self.commit_text(bar.sheet, bar.pos, text, window, cx);
            }
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn select_current_region(&mut self, cx: &mut Context<Self>) -> Range {
        let active = self.grid.read(cx).selection().active;
        let sheet = self.document.sheet;
        let Some(workbook) = self.document.workbook() else {
            return Range::single(active);
        };
        let region = region::current_region(active, |pos| !workbook.input(sheet, pos).is_empty());
        self.grid
            .update(cx, |grid, cx| grid.select(region.start, region.end, cx));
        region
    }

    fn freeze(&mut self, rows: u32, cols: u16, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.document.sheet;
        self.edit(window, cx, move |wb| wb.set_frozen(sheet, rows, cols));
    }

    fn fill(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (sheet, range) = (self.document.sheet, self.selection(cx));
        self.edit(window, cx, move |wb| wb.fill(sheet, range, down));
    }

    // Alt+= proposes =SUM over the numbers right above (or to the left), in edit mode.
    fn auto_sum(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbook) = self.document.workbook() else {
            return;
        };
        let sheet = self.document.sheet;
        let active = self.grid.read(cx).selection().active;
        let is_number = |pos: CellPos| workbook.number(sheet, pos).is_some();
        let run = |direction: Direction| {
            let mut first = None;
            let mut pos = active;
            for _ in 0..MAX_AUTOSUM_SCAN {
                let next = zenkai_grid::step(pos, direction, 1);
                if next == pos || !is_number(next) {
                    break;
                }
                pos = next;
                first = Some(pos);
            }
            first.map(|start| Range::new(start, zenkai_grid::step(active, direction, 1)))
        };
        let range = run(Direction::Up).or_else(|| run(Direction::Left));
        let formula = range.map_or_else(|| "=SUM()".to_string(), |r| format!("=SUM({r})"));
        self.grid.update(cx, |grid, cx| {
            grid.begin_edit(active, formula, EditMode::Enter, cx)
        });
        window.refresh();
    }

    fn insert_now(&mut self, time: bool, window: &mut Window, cx: &mut Context<Self>) {
        let now = chrono::Local::now().naive_local();
        let (sheet, active) = (self.document.sheet, self.grid.read(cx).selection().active);
        if !time {
            // An ISO date is recognised and formatted by the engine in one undo step.
            let text = now.format("%Y-%m-%d").to_string();
            self.edit(window, cx, move |wb| {
                wb.set_input(sheet, active, &text)?;
                widen_for_numbers(wb, sheet, Range::single(active))
            });
            return;
        }
        let midnight = now.date().and_hms_opt(0, 0, 0).unwrap_or(now);
        // Excel stores a time of day as the fraction of a day.
        let fraction = (now - midnight).num_milliseconds() as f64 / 86_400_000.0;
        let text = fraction.to_string();
        self.edit(window, cx, move |wb| {
            wb.set_input(sheet, active, &text)?;
            wb.apply_style(
                sheet,
                Range::single(active),
                StyleChange::NumberFormat(NumberFormat::Time),
            )
        });
    }

    // Double-click on a header edge: as wide as the longest displayed text in the column.
    fn auto_fit(&mut self, col: ColIdx, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.document.sheet;
        self.edit(window, cx, move |wb| {
            let longest = wb
                .filled_cells(sheet)
                .into_iter()
                .filter(|pos| pos.col == col)
                .take(MAX_AUTOFIT_CELLS)
                .map(|pos| wb.cell(sheet, pos).text.chars().count())
                .max()
                .unwrap_or(0);
            let width = (longest as f32 * AUTOFIT_CHAR_WIDTH + 12.0).clamp(24.0, 600.0);
            wb.set_column_width(sheet, col, width)
        });
    }

    fn duplicate_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.document.sheet;
        self.pending_sheet = Some(SheetId(sheet.0 + 1));
        self.edit(window, cx, move |wb| wb.duplicate_sheet(sheet));
    }

    fn add_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_sheet = Some(SheetId(
            u32::try_from(self.document.sheets.len()).unwrap_or(0),
        ));
        self.edit(window, cx, |wb| wb.add_sheet().map(|_| ()));
    }

    fn open_go_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("A1 or A1:C10"));
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        let target = input.read(cx).value().to_string();
                        this.close_go_to(window, cx);
                        match Range::parse_a1(&target) {
                            Some(range) => {
                                this.grid.update(cx, |grid, cx| grid.show_range(range, cx))
                            }
                            None => this.notify(
                                Severity::Warning,
                                format!("\"{target}\" is not a cell or range reference"),
                                cx,
                            ),
                        }
                    }
                    InputEvent::Blur => this.close_go_to(window, cx),
                    _ => {}
                },
            );
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        self.go_to = Some((input, subscription));
        cx.notify();
    }

    fn close_go_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.go_to.take().is_none() {
            return;
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        let current = self
            .document
            .sheets
            .get(self.document.sheet.0 as usize)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Sheet name");
            state.set_value(current, window, cx);
            state
        });
        let subscription = cx.subscribe_in(&input, window, |this, input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let name = input.read(cx).value().trim().to_string();
                this.close_rename(window, cx);
                if !name.is_empty() {
                    let sheet = this.document.sheet;
                    this.edit(window, cx, move |wb| wb.rename_sheet(sheet, &name));
                }
            }
        });
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        self.rename = Some((input, subscription));
        cx.notify();
    }

    fn close_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn render_rename(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (input, _) = self.rename.as_ref()?;
        let theme = cx.theme();
        Some(
            h_flex()
                .key_context("RenameBar")
                .h(px(36.0))
                .px_2()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(div().text_sm().child("Rename sheet"))
                .child(div().w(px(260.0)).child(Input::new(input)))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Enter to apply, Esc to cancel"),
                ),
        )
    }

    fn delete_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.sheets.len() < 2 {
            self.notify(
                Severity::Warning,
                "A workbook must contain at least one sheet.",
                cx,
            );
            return;
        }
        let sheet = self.document.sheet;
        let name = self
            .document
            .sheets
            .get(sheet.0 as usize)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete sheet \"{name}\"?"),
            Some("Its data is removed. You can undo with Ctrl+Z."),
            &["Cancel", "Delete"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1)
                && let Err(error) = this.update_in(cx, |this, window, cx| {
                    // Excel shows the sheet that takes the deleted one's place.
                    let last_after =
                        u32::try_from(this.document.sheets.len().saturating_sub(2)).unwrap_or(0);
                    this.pending_sheet = Some(SheetId(sheet.0.min(last_after)));
                    this.edit(window, cx, move |wb| wb.delete_sheet(sheet));
                })
            {
                tracing::debug!(%error, "workspace closed during delete prompt");
            }
        })
        .detach();
    }

    fn move_sheet(&mut self, left: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self.document.sheet;
        let last = u32::try_from(self.document.sheets.len().saturating_sub(1)).unwrap_or(0);
        let target = if left {
            sheet.0.checked_sub(1)
        } else {
            (sheet.0 < last).then_some(sheet.0 + 1)
        };
        let Some(target) = target else {
            return;
        };
        self.pending_sheet = Some(SheetId(target));
        self.edit(window, cx, move |wb| wb.move_sheet(sheet, target));
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
        // Modes that change what the sheet shows or allows are named, never only implied.
        if self.show_formulas {
            right = right.child("Showing formulas");
        }
        if self.document.read_only {
            right = right.child("Read-only");
        }
        let zoom = self.grid.read(cx).zoom();
        right = right.child(format!("{:.0}%", zoom * 100.0));
        if self.diagnostics {
            let recalc = self
                .last_recalc
                .map_or("-".to_string(), |d| format!("{} ms", d.as_millis()));
            let frame = self.grid.read(cx).last_paint();
            right = right
                .child(format!("Memory: {} MB", self.memory_mb))
                .child(format!(
                    "Grid paint: {:.1} ms",
                    frame.as_secs_f64() * 1000.0
                ))
                .child(format!("Last recalc: {recalc}"));
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(cx.theme().font_size * self.ui_scale);
        let theme = cx.theme();
        v_flex()
            .relative()
            .key_context("Workspace")
            // A file dropped on the window opens like File > Open, after the unsaved-changes
            // question.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first().cloned() {
                    this.confirm_discard(window, cx, move |this, window, cx| {
                        this.open_path(path, window, cx)
                    });
                }
            }))
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
            .on_action(cx.listener(|this, _: &ToggleStrikethrough, window, cx| {
                this.toggle_flag(|s| s.strike, StyleChange::Strike, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &GrowFont, window, cx| this.step_font_size(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShrinkFont, window, cx| {
                this.step_font_size(false, window, cx)
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
            .on_action(cx.listener(|this, _: &BordersAll, window, cx| {
                this.style(StyleChange::Borders(BorderPreset::All), window, cx)
            }))
            .on_action(cx.listener(|this, _: &BordersOutside, window, cx| {
                this.style(StyleChange::Borders(BorderPreset::Outside), window, cx)
            }))
            .on_action(cx.listener(|this, _: &BorderBottom, window, cx| {
                this.style(StyleChange::Borders(BorderPreset::Bottom), window, cx)
            }))
            .on_action(cx.listener(|this, _: &BordersNone, window, cx| {
                this.style(StyleChange::Borders(BorderPreset::None), window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectCurrentRegion, _, cx| {
                this.select_current_region(cx);
            }))
            .on_action(
                cx.listener(|this, _: &FreezeTopRow, window, cx| this.freeze(1, 0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FreezeFirstColumn, window, cx| {
                    this.freeze(0, 1, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SortAscending, window, cx| this.sort(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SortDescending, window, cx| this.sort(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &IncreaseDecimal, window, cx| {
                this.step_decimals(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseDecimal, window, cx| {
                this.step_decimals(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleWrapText, window, cx| this.toggle_wrap(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &HideRows, window, cx| this.hide(true, true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &UnhideRows, window, cx| this.hide(true, false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &HideColumns, window, cx| this.hide(false, true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &UnhideColumns, window, cx| {
                this.hide(false, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleFormulas, _, cx| {
                this.show_formulas = !this.show_formulas;
                let shown = if this.show_formulas {
                    "formulas"
                } else {
                    "values"
                };
                this.notify(
                    Severity::Info,
                    format!("Showing {shown} (Ctrl+` to switch)"),
                    cx,
                );
                this.refresh_cells(cx);
            }))
            .on_action(
                cx.listener(|this, _: &PasteValues, window, cx| this.paste_values(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FormatCells, window, cx| {
                    this.open_format_dialog(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ApplyNumberFormat, window, cx| {
                this.apply_format_dialog(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseFormatDialog, window, cx| {
                this.close_format_dialog(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenRecent1, window, cx| this.open_recent(0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenRecent2, window, cx| this.open_recent(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenRecent3, window, cx| this.open_recent(2, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenRecent4, window, cx| this.open_recent(3, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenRecent5, window, cx| this.open_recent(4, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CycleReference, window, cx| {
                if let Some(change) = this.last_style {
                    this.style(change, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &DuplicateSheet, window, cx| {
                    this.duplicate_sheet(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ClearFormats, window, cx| {
                let (sheet, range) = (this.document.sheet, this.selection(cx));
                this.edit(window, cx, move |wb| wb.clear_formats(sheet, range));
            }))
            .on_action(cx.listener(|this, _: &ClearAll, window, cx| {
                let (sheet, range) = (this.document.sheet, this.selection(cx));
                this.edit(window, cx, move |wb| wb.clear_all(sheet, range));
            }))
            .on_action(cx.listener(|this, _: &CancelFormulaBar, window, cx| {
                this.close_formula_bar(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NoFill, window, cx| {
                this.style(StyleChange::Fill(None), window, cx)
            }))
            .on_action(cx.listener(|this, _: &AutomaticFontColor, window, cx| {
                this.style(StyleChange::FontColor(None), window, cx)
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
            .on_action(cx.listener(|this, _: &InterfaceLarger, _, cx| {
                this.set_ui_scale(this.ui_scale + UI_SCALE_STEP, cx);
            }))
            .on_action(cx.listener(|this, _: &InterfaceSmaller, _, cx| {
                this.set_ui_scale(this.ui_scale - UI_SCALE_STEP, cx);
            }))
            .on_action(cx.listener(|this, _: &InterfaceReset, _, cx| this.set_ui_scale(1.0, cx)))
            .on_action(cx.listener(|this, _: &ToggleReduceMotion, _, cx| {
                let reduce = !cx.reduce_motion();
                cx.set_reduce_motion(reduce);
                let state = if reduce { "on" } else { "off" };
                this.notify(Severity::Info, format!("Reduce motion {state}"), cx);
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
            .on_action(cx.listener(|this, _: &InsertChart, _, cx| this.insert_chart(cx)))
            .on_action(cx.listener(|this, _: &Replace, window, cx| this.open_replace(window, cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(
                cx.listener(|this, _: &RenameSheet, window, cx| this.open_rename(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ConfirmCsvImport, window, cx| {
                this.confirm_csv_import(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CancelCsvImport, window, cx| {
                this.csv_preview = None;
                this.next_csv_request();
                let focus = this.grid.focus_handle(cx);
                window.focus(&focus, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoTo, window, cx| this.open_go_to(window, cx)))
            .on_action(cx.listener(|this, _: &FillDown, window, cx| this.fill(true, window, cx)))
            .on_action(cx.listener(|this, _: &FillRight, window, cx| this.fill(false, window, cx)))
            .on_action(cx.listener(|this, _: &AutoSum, window, cx| this.auto_sum(window, cx)))
            .on_action(
                cx.listener(|this, _: &InsertDate, window, cx| this.insert_now(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &InsertTime, window, cx| this.insert_now(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &InsertRows, window, cx| {
                this.structure_edit(window, cx, StructureEdit::InsertRows)
            }))
            .on_action(cx.listener(|this, _: &DeleteRows, window, cx| {
                this.structure_edit(window, cx, StructureEdit::DeleteRows)
            }))
            .on_action(cx.listener(|this, _: &InsertColumns, window, cx| {
                this.structure_edit(window, cx, StructureEdit::InsertColumns)
            }))
            .on_action(cx.listener(|this, _: &DeleteColumns, window, cx| {
                this.structure_edit(window, cx, StructureEdit::DeleteColumns)
            }))
            .on_action(cx.listener(|this, _: &FreezePanes, window, cx| {
                this.structure_edit(window, cx, StructureEdit::FreezePanes)
            }))
            .on_action(cx.listener(|this, _: &CloseGoTo, window, cx| this.close_go_to(window, cx)))
            .on_action(
                cx.listener(|this, _: &CloseRename, window, cx| this.close_rename(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DeleteSheet, window, cx| this.delete_sheet(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveSheetLeft, window, cx| {
                    this.move_sheet(true, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &MoveSheetRight, window, cx| {
                this.move_sheet(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &TogglePalette, window, cx| this.toggle_palette(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ClosePalette, window, cx| this.close_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseFind, window, cx| this.close_find(window, cx)))
            .on_action(cx.listener(|this, _: &ChartColumn, _, cx| {
                this.set_chart_kind(ChartKind::Column, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ChartLine, _, cx| this.set_chart_kind(ChartKind::Line, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ChartPie, _, cx| this.set_chart_kind(ChartKind::Pie, cx)),
            )
            .on_action(cx.listener(|this, _: &CopyChartMermaid, _, cx| this.copy_chart_mermaid(cx)))
            .on_action(
                cx.listener(|this, _: &ExportChartSvg, window, cx| {
                    this.export_chart_svg(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CloseChart, _, cx| {
                this.chart = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleDiagnostics, _, cx| {
                this.diagnostics = !this.diagnostics;
                if this.diagnostics {
                    this.sample_diagnostics(cx);
                } else {
                    this.diagnostics_task = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| theme::cycle(window, cx)))
            .child(toolbar::render(&self.active_style, &self.colors, cx))
            .child(self.render_formula_bar(cx))
            .children(self.render_find(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("grid-area")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.grid.clone())
                            .context_menu({
                                let grid_focus = self.grid.focus_handle(cx);
                                move |menu, _, _| cell_menu(menu, grid_focus.clone())
                            }),
                    )
                    .children(
                        self.chart
                            .as_ref()
                            .map(|panel| chart_panel::render(panel, cx)),
                    ),
            )
            .children(self.render_rename(cx))
            .child(self.render_tabs(cx))
            .child(self.render_status(cx))
            .children(self.render_palette())
            .children(self.render_csv_preview(cx))
            .children(self.render_format_dialog(cx))
    }
}

// Excel's cell context menu, trimmed to what Zenkai supports. Actions go to the grid so
// they act on the selection the right click just set.
fn cell_menu(menu: PopupMenu, grid_focus: FocusHandle) -> PopupMenu {
    menu.action_context(grid_focus)
        .menu("Cut", Box::new(Cut))
        .menu("Copy", Box::new(Copy))
        .menu("Paste", Box::new(Paste))
        .menu("Paste values", Box::new(PasteValues))
        .separator()
        .menu("Insert rows above", Box::new(InsertRows))
        .menu("Insert columns to the left", Box::new(InsertColumns))
        .menu("Delete rows", Box::new(DeleteRows))
        .menu("Delete columns", Box::new(DeleteColumns))
        .separator()
        .menu("Clear contents", Box::new(DeleteForward))
        .menu("Clear formats", Box::new(ClearFormats))
        .menu("Clear all", Box::new(ClearAll))
        .separator()
        .menu("Sort A to Z", Box::new(SortAscending))
        .menu("Sort Z to A", Box::new(SortDescending))
        .separator()
        .menu("Insert chart", Box::new(InsertChart))
}

// Excel's sheet tab menu; it acts on the active sheet.
fn sheet_menu(menu: PopupMenu, grid_focus: FocusHandle) -> PopupMenu {
    menu.action_context(grid_focus)
        .menu("Insert sheet", Box::new(NewSheet))
        .menu("Rename", Box::new(RenameSheet))
        .menu("Duplicate", Box::new(DuplicateSheet))
        .menu("Delete", Box::new(DeleteSheet))
        .separator()
        .menu("Move left", Box::new(MoveSheetLeft))
        .menu("Move right", Box::new(MoveSheetRight))
}

fn rgb_of(color: Hsla) -> Rgb {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    Rgb(channel(rgba.r) << 16 | channel(rgba.g) << 8 | channel(rgba.b))
}

// Excel widens a column that still has the default width when a formatted number or date
// typed into it would only show as ####. General numbers are shortened when painted
// instead, and text keeps overflowing.
// Each default-width column of `range` grows to its widest formatted number, once.
fn widen_for_numbers(wb: &mut Workbook, sheet: SheetId, range: Range) -> Result<(), EngineError> {
    let sizes = wb.sizes(sheet);
    let mut needed: std::collections::BTreeMap<ColIdx, f32> = Default::default();
    // Small ranges are read cell by cell; large ones only where the sheet has content.
    let cells: Vec<CellPos> = if range.cell_count() <= MAX_AUTOFIT_CELLS as u64 {
        range.positions().collect()
    } else {
        wb.filled_cells(sheet)
            .into_iter()
            .filter(|pos| range.contains(*pos))
            .take(MAX_AUTOFIT_CELLS)
            .collect()
    };
    for pos in cells {
        let custom = sizes
            .columns
            .iter()
            .any(|span| span.first <= pos.col && pos.col <= span.last);
        let view = wb.cell(sheet, pos);
        if custom || view.number.is_none() || view.style.num_fmt == "general" {
            continue;
        }
        let width = view.text.chars().count() as f32 * AUTOFIT_CHAR_WIDTH + 12.0;
        let entry = needed.entry(pos.col).or_insert(0.0);
        *entry = entry.max(width);
    }
    for (col, width) in needed {
        if width > zenkai_types::DEFAULT_COL_WIDTH {
            wb.set_column_width(sheet, col, width)?;
        }
    }
    Ok(())
}

// Excel grows default-height rows to show every wrapped line; the line count is
// estimated from the text length and column width, like the column autofit.
fn grow_wrapped_rows(wb: &mut Workbook, sheet: SheetId, range: Range) -> Result<(), EngineError> {
    let sizes = wb.sizes(sheet);
    let width = |col: ColIdx| {
        sizes
            .columns
            .iter()
            .find(|span| span.first <= col && col <= span.last)
            .map_or(zenkai_types::DEFAULT_COL_WIDTH, |span| span.width)
    };
    // The tallest wrapped cell sets each row, once.
    let mut lines_per_row: std::collections::BTreeMap<zenkai_types::RowIdx, f32> =
        Default::default();
    for pos in wb
        .filled_cells(sheet)
        .into_iter()
        .filter(|pos| range.contains(*pos))
    {
        if sizes.rows.iter().any(|(row, _)| *row == pos.row) {
            continue;
        }
        let view = wb.cell(sheet, pos);
        if view.kind != zenkai_types::ValueKind::Text {
            continue;
        }
        let per_line = ((width(pos.col) - 12.0) / AUTOFIT_CHAR_WIDTH).max(1.0);
        // Breaking at word boundaries wastes part of each line.
        let lines: f32 = view
            .text
            .split('\n')
            .map(|part| {
                (part.chars().count() as f32 * 1.3 / per_line)
                    .ceil()
                    .max(1.0)
            })
            .sum();
        let entry = lines_per_row.entry(pos.row).or_insert(1.0);
        *entry = entry.max(lines.min(20.0));
    }
    for (row, lines) in lines_per_row {
        if lines > 1.0 {
            wb.set_row_height(sheet, row, lines * 19.0 + 4.0)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::widen_for_numbers;
    use zenkai_engine::{Engine, Workbook};
    use zenkai_types::{CellPos, Range, SheetId};

    #[test]
    fn formatting_a_number_that_no_longer_fits_widens_its_column() {
        let mut wb = Workbook::new_empty().unwrap();
        let a1 = CellPos::default();
        wb.set_input(SheetId(0), a1, "1234.5").unwrap();
        let range = Range::single(a1);
        wb.set_number_format(SheetId(0), range, "$#,##0.00")
            .unwrap();
        eprintln!(
            "PROBE text={:?} fmt={:?} sizes={:?}",
            wb.cell(SheetId(0), a1).text,
            wb.cell(SheetId(0), a1).style.num_fmt,
            wb.sizes(SheetId(0)).columns
        );
        widen_for_numbers(&mut wb, SheetId(0), range).unwrap();
        let widened = wb
            .sizes(SheetId(0))
            .columns
            .iter()
            .any(|span| span.first == a1.col && span.width > zenkai_types::DEFAULT_COL_WIDTH);
        assert!(widened, "{:?}", wb.sizes(SheetId(0)).columns);
    }
}
