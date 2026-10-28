use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use zenkai_agent::settings::SettingsError;
use zenkai_engine::{Copied, Engine, EngineError, Unsupported, Workbook, save_xlsx};
use zenkai_formats::{Delimiter, parse_csv};
use zenkai_grid::{
    CycleReference, DeleteForward, Direction, EditMode, Grid, GridEvent, Layout, SheetView,
};
use zenkai_i18n::t;
use zenkai_types::{
    BorderPreset, CellPos, CellStyle, ColIdx, Contents, HAlign, NumberFormat, Range, Rgb, SheetId,
    StyleChange, WorkbookId,
};

mod agent_access;
mod agent_calls;
mod agent_chip;
mod agent_files;
mod agent_review;
mod chat_dock;
mod close_dialog;
mod closing;
mod messages;
mod notice_card;
mod notices;
mod pdfs;
mod save_flow;
mod settings_gate;
mod space_panel;
mod text_actions;
mod text_save;
mod text_watch;
mod texts;

use crate::prompt_gate::{PROMPT_OPEN_CONTEXT, PromptGate};
use agent_calls::{AgentLink, Decision};
use chat_dock::ChatDock;
use zenkai_agent::protected_view::file_origin;
use zenkai_agent::settings::{HeldChange, PermissionMode};

use crate::actions::*;
use crate::agent_settings::{self, AgentConfig, HeldDecision};
use crate::auto_sum;
use crate::chart::{self, ChartKind};
use crate::chart_panel::{self, ChartPanel};
use crate::chat_layout::{ChatLayout, ChatLayoutState};
use crate::clipboard;
use crate::close_prompt::{CloseGoal, Departure};
use crate::csv_preview::{self, CsvPreview};
use crate::decimals;
use crate::document::{self, ChangedFile, Document, DocumentSave, FileJob};
use crate::documents::{Documents, Step};
use crate::failure::{Failure, offer_another_location, save_failure};
use crate::files;
use crate::find::{self, FindBar, FindResults};
use crate::format_dialog::{self, FormatDialog};
use crate::go_to::{self, GoToError};
use crate::interface_scale::{self, rem_px};
use crate::jump::jump_target;
use crate::keymap;
use crate::memory;
use crate::open_loads::OpenLoads;
use crate::palette;
use crate::panel_width::{Panel, Resize};
use crate::pending_save::{AfterJob, JobEnd};
use crate::previews::TypedPreviews;
use crate::recent;
use crate::recovery;
use crate::refresh::{CellsRefresh, MissedRefreshes};
use crate::region;
use crate::session::{SaveOrder, Session};
use crate::space_appearance::SpaceAppearance;
use crate::spaces::Neighbour;
use crate::stale_view::StaleRequest;
use crate::start_view;
use crate::stats::{self, SelectionStats, StatsJob};
use crate::theme;
use crate::toolbar;
use zenkai_agent::status_summary::{StatusSummary, SummaryItem};

mod budget;
mod generate;
mod lifecycle;
mod main_window;
mod panels;
mod prompt;
mod search;
mod sidebar;
mod theme_picker;
mod workbooks;
use crate::job_pill::{JobPill, LongJob, SHOW_AFTER};
use gpui_kit::component::Icon;
use gpui_kit::component::Sizable;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::command::{Command, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu};
use gpui_kit::component::spinner::Spinner;
use lifecycle::Lifecycle;
use notice_card::{STATUS_BAR_HEIGHT, severity_icon, severity_title};
use notices::{Lifetime, Notices, Severity};
use panels::PanelsState;
use search::SearchOverlay;
use sidebar::SidebarState;
use theme_picker::ThemePicker;

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

// Excel asks with a dialog for a plain cell range; without one, only whole rows or columns act.
fn selection_edit(insert: bool, selection: Range) -> Option<StructureEdit> {
    let whole_columns = selection.rows() == zenkai_types::MAX_ROWS;
    let whole_rows = selection.cols() == zenkai_types::MAX_COLS;
    match (whole_columns, whole_rows, insert) {
        (true, false, true) => Some(StructureEdit::InsertColumns),
        (true, false, false) => Some(StructureEdit::DeleteColumns),
        (false, true, true) => Some(StructureEdit::InsertRows),
        (false, true, false) => Some(StructureEdit::DeleteRows),
        _ => None,
    }
}

const SAVE_AGAIN_POLL: Duration = Duration::from_millis(50);
const SAVE_AGAIN_LIMIT: Duration = Duration::from_secs(300);
// Windows makes the first answer the default button, so Cancel goes first.
const CHANGED_FILE_REPLACE: usize = 1;
const CHANGED_FILE_SAVE_AS: usize = 2;
const MAX_AUTOSUM_SCAN: usize = 10_000;
const MAX_AUTOFIT_CELLS: usize = 100_000;
const AUTOFIT_CHAR_WIDTH: f32 = 7.5;

const MAX_REPLACE_CELLS: usize = 100_000;
const FONT_SIZES: [u16; 16] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 22, 24, 26, 28, 36, 48, 72];

pub struct Workspace {
    documents: Documents,
    grid: Entity<Grid>,
    stats: Option<SelectionStats>,
    stats_request: u64,
    stats_job: StatsJob,
    active_input: SharedString,
    active_style: CellStyle,
    notices: Notices,
    notice_focus: FocusHandle,
    notice_hovered: bool,
    notice_focused: Cell<bool>,
    messages: Option<messages::MessagesPanel>,
    toast: Option<agent_access::Toast>,
    toast_generation: u64,
    busy: Option<SharedString>,
    save_as_flows: save_flow::SaveAsFlows,
    job_pill: JobPill,
    open_loads: OpenLoads,
    last_recalc: Option<Duration>,
    diagnostics: bool,
    clipboard_source: Option<InternalClip>,
    chart: Option<ChartPanel>,
    palette: Option<Entity<CommandState>>,
    search: Option<SearchOverlay>,
    theme_picker: Option<ThemePicker>,
    csv_preview: Option<CsvPreview>,
    // Bumped by every CSV read, re-parse, import and cancel; a background result is
    // applied only if no newer request started meanwhile.
    csv_request: u64,
    show_formulas: bool,
    formula_bar: Option<FormulaBarEdit>,
    // The last formatting change, which F4 repeats on the selection as in Excel.
    last_style: Option<StyleChange>,
    recent: Vec<PathBuf>,
    recent_saves: Arc<AtomicU64>,
    format_dialog: Option<FormatDialog>,
    generate: Option<generate::GenerateSession>,
    colors: toolbar::ColorPickers,
    focus: FocusHandle,
    session_lock: Option<recovery::SessionLock>,
    recovery_dir: Option<PathBuf>,
    rename: Option<(Entity<InputState>, Subscription)>,
    go_to: Option<(Entity<InputState>, Subscription)>,
    previews: TypedPreviews,
    last_tab_click: Option<(Instant, SheetId)>,
    memory_mb: u64,
    memory_sampler: Option<Task<()>>,
    memory_budget_mb: u64,
    last_unload: Option<Instant>,
    // The workbook being written to disk to leave memory; one at a time, as each needs a copy
    // of itself in memory while it is encoded.
    hibernating: Option<WorkbookId>,
    // Since when the window has been hidden in the tray.
    hidden_since: Option<Instant>,
    sidebar: SidebarState,
    panels: PanelsState,
    text_panes: texts::TextPanes,
    pdf_panes: pdfs::PdfPanes,
    pdf_places: crate::pdf_places::PdfPlaces,
    pdf_client: crate::pdf::PdfClient,
    text_watch: text_watch::TextWatch,
    lifecycle: Lifecycle,
    #[cfg(windows)]
    quit_after_hide: bool,
    // Saves ahead of closing a tab or the app; `None` when no close waits on a save.
    departure: Option<Departure>,
    close_dialog: Option<close_dialog::CloseDialog>,
    prompts: PromptGate,
    saved_session: Option<Session>,
    unwritten_session: Option<(PathBuf, Session, SaveOrder)>,
    session_writing: bool,
    cell_refresh: CellRefresh,
    missed: MissedRefreshes,
    space_panel: Option<space_panel::SpacePanel>,
    agent: AgentLink,
    chat: ChatDock,
    // The settings problem already shown, so a reload with the same error stays quiet.
    shown_settings_problem: Option<SettingsError>,
    // A settings.json change that gives agents more power, as last shown for confirmation.
    shown_held: Option<HeldChange>,
    held_focus: FocusHandle,
    held_return_focus: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellRefresh {
    Idle,
    Scheduled,
}

// A save over the document's own file, which stops before dropping anything not accepted.
enum Overwrite {
    InPlace,
    Original,
    Confirmed(Vec<Unsupported>),
}

impl Overwrite {
    fn accepted(&self) -> &[Unsupported] {
        match self {
            Overwrite::InPlace | Overwrite::Original => &[],
            Overwrite::Confirmed(lost) => lost,
        }
    }
}

enum Saved {
    Written {
        kept: Vec<Unsupported>,
    },
    Refused {
        dropped: Vec<Unsupported>,
        overwrite: Overwrite,
    },
    BaseChanged {
        overwrite: Overwrite,
    },
}

#[derive(Debug, PartialEq)]
enum TargetAction {
    RefuseOriginal,
    ConfirmReplacingOriginal,
    SaveOverOriginal,
    ConfirmReplacingExisting,
    SaveNew,
}

fn target_action(
    probe: &files::TargetProbe,
    renamed: bool,
    never_overwritten: bool,
    may_lose_unseen_content: bool,
) -> TargetAction {
    if probe.same_as_source {
        if never_overwritten {
            TargetAction::RefuseOriginal
        } else if may_lose_unseen_content {
            TargetAction::ConfirmReplacingOriginal
        } else {
            TargetAction::SaveOverOriginal
        }
    } else if renamed && probe.exists {
        TargetAction::ConfirmReplacingExisting
    } else {
        TargetAction::SaveNew
    }
}

#[derive(Debug, PartialEq)]
enum DelimitedAction {
    Export,
    ConfirmReplacing,
}

// The native dialog already warns about an existing file, and a delimited name is never
// rewritten by Zenkai; only the open source file is confirmed again.
fn delimited_action(probe: &files::TargetProbe) -> DelimitedAction {
    if probe.same_as_source {
        DelimitedAction::ConfirmReplacing
    } else {
        DelimitedAction::Export
    }
}

enum Replacing {
    Existing,
    ExistingExport,
    Original(Vec<Unsupported>),
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
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                agent_settings::reload(cx);
            }
        });
        let settings = cx.observe_global_in::<AgentConfig>(window, Self::on_settings_changed);
        let space_look = cx.observe_global::<SpaceAppearance>(|this, cx| {
            this.persist_session(cx);
            cx.notify();
        });
        let quit = cx.on_app_quit(|this, cx| {
            this.stop_bridge();
            let tree = this.shutdown_chat(cx);
            let executor = cx.background_executor().clone();
            async move {
                if let Some(tree) = tree {
                    executor.spawn(async move { tree.close() }).await;
                }
            }
        });
        let mut workspace = Workspace {
            documents: Documents::new(empty_workbook()),
            grid,
            stats: None,
            stats_request: 0,
            stats_job: StatsJob::default(),
            active_input: SharedString::default(),
            active_style: CellStyle::default(),
            notices: Notices::default(),
            notice_focus: cx.focus_handle(),
            notice_hovered: false,
            notice_focused: Cell::new(false),
            messages: None,
            toast: None,
            toast_generation: 0,
            busy: None,
            save_as_flows: save_flow::SaveAsFlows::default(),
            job_pill: JobPill::default(),
            open_loads: OpenLoads::default(),
            last_recalc: None,
            diagnostics: false,
            clipboard_source: None,
            chart: None,
            palette: None,
            search: None,
            theme_picker: None,
            csv_preview: None,
            csv_request: 0,
            show_formulas: false,
            formula_bar: None,
            last_style: None,
            recent: Vec::new(),
            recent_saves: Arc::new(AtomicU64::new(0)),
            format_dialog: None,
            generate: None,
            colors,
            focus: cx.focus_handle(),
            session_lock: None,
            recovery_dir: None,
            rename: None,
            go_to: None,
            previews: TypedPreviews::default(),
            last_tab_click: None,
            memory_mb: 0,
            memory_sampler: None,
            memory_budget_mb: memory::budget_mb(),
            last_unload: None,
            hibernating: None,
            hidden_since: None,
            sidebar: SidebarState::new(cx),
            panels: PanelsState::new(cx),
            text_panes: texts::TextPanes::new(),
            pdf_panes: pdfs::PdfPanes::new(),
            pdf_places: crate::pdf_places::PdfPlaces::default(),
            pdf_client: crate::pdf::PdfClient::next_to_exe(),
            text_watch: text_watch::TextWatch::default(),
            lifecycle: Lifecycle::Running,
            #[cfg(windows)]
            quit_after_hide: false,
            departure: None,
            close_dialog: None,
            prompts: PromptGate::default(),
            saved_session: None,
            unwritten_session: None,
            session_writing: false,
            cell_refresh: CellRefresh::Idle,
            missed: MissedRefreshes::default(),
            space_panel: None,
            agent: Self::start_tool_service(window, cx),
            chat: ChatDock::default(),
            shown_settings_problem: None,
            shown_held: None,
            held_focus: cx.focus_handle(),
            held_return_focus: None,
            _subscriptions: vec![
                subscription,
                appearance,
                activation,
                settings,
                space_look,
                quit,
                font_color,
                fill_color,
            ],
        };
        workspace.reset_grid(window, cx);
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let goal = main_window::close_goal(cx);
            let asked = this.update(cx, |this, cx| this.request_close(goal, window, cx));
            asked.is_err()
        });
        #[cfg(windows)]
        workspace._subscriptions.push(
            cx.observe_window_visibility(window, |_, visibility, _, cx| {
                crate::tray::note_visibility(visibility, cx)
            }),
        );
        workspace.start_memory_sampler(window, cx);
        workspace.start_session(initial, window, cx);
        workspace.load_recent(cx);
        workspace
    }

    fn reset_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_sheet_view(cx);
        self.refresh_stats(cx);
        window.set_window_title(&self.window_title());
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn active_sheet(&self) -> Option<SheetId> {
        self.documents.active().map(|document| document.sheet)
    }

    fn window_title(&self) -> String {
        self.documents.shown_title()
    }

    fn load_sheet_view(&mut self, cx: &mut Context<Self>) {
        self.forget_find_results();
        let view = self.sheet_view();
        self.grid.update(cx, |grid, cx| grid.reset(view, cx));
        self.sync_pending_highlight(cx);
        self.sync_review_marks(cx);
        self.refresh_cells(cx);
    }

    fn sheet_view(&self) -> SheetView {
        let Some(document) = self.documents.active() else {
            return SheetView::default();
        };
        let sheet = document.sheet;
        document
            .workbook()
            .map(|wb| {
                let (frozen_rows, frozen_cols) = wb.frozen(sheet);
                SheetView {
                    used_end: wb.used_end(sheet),
                    layout: Layout::from_sizes(&wb.sizes(sheet)),
                    frozen_rows,
                    frozen_cols,
                    merges: wb.merged(sheet),
                }
            })
            .unwrap_or_default()
    }

    // Mouse moves, wheel ticks and key repeats can outnumber frames; one read per frame,
    // right before it is drawn, is all the screen can show.
    fn schedule_cell_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cell_refresh == CellRefresh::Scheduled {
            return;
        }
        self.cell_refresh = CellRefresh::Scheduled;
        let this = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            this.update(cx, |this, cx| {
                this.cell_refresh = CellRefresh::Idle;
                this.refresh_cells(cx);
            })
            .ok();
        });
    }

    fn refresh_cells(&mut self, cx: &mut Context<Self>) {
        self.fetch_cells(CellsRefresh::Merge, cx);
    }

    fn fetch_cells(&mut self, refresh: CellsRefresh, cx: &mut Context<Self>) {
        // While the workbook is busy the grid keeps the last values it knows; the refresh
        // is retried the moment the workbook is free, not only when a batch ends.
        let ranges = self.grid.read(cx).cached_ranges();
        let Some(document) = self.documents.active() else {
            return;
        };
        let (id, generation) = (document.id, document.generation());
        let refresh = self.missed.upgraded(generation, refresh);
        let Some(mut cells) = document.cells(&ranges, self.show_formulas) else {
            self.missed.miss_cells(generation, refresh);
            self.show_stale_cells(&ranges, cx);
            self.wait_for_workbook(id, cx);
            return;
        };
        self.previews
            .apply(generation, document.sheet, &ranges, &mut cells);
        self.missed.cells_done(generation);
        self.grid.update(cx, |grid, cx| match refresh {
            CellsRefresh::Merge => grid.merge_cells(cells, &ranges, cx),
            CellsRefresh::Replace => grid.replace_cells(cells, cx),
        });
    }

    // While a batch recalculates, rows the grid never loaded show their values from before
    // it. They merge into the cache only inside the part the copy covers, so nothing outside
    // it is wiped; the refresh after the batch replaces them with the new values.
    fn show_stale_cells(&mut self, ranges: &[Range], cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let Some((mut cells, covered)) = document.stale_cells(ranges, self.show_formulas) else {
            return;
        };
        self.previews
            .apply(document.generation(), document.sheet, &covered, &mut cells);
        self.grid
            .update(cx, |grid, cx| grid.merge_cells(cells, &covered, cx));
    }

    // A background task waits for the lock, so the UI thread never does, and then hands the
    // missed refreshes back to the foreground.
    fn wait_for_workbook(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(document) = self.documents.get(id) else {
            return;
        };
        let generation = document.generation();
        if !self.missed.start_waiting(generation) {
            return;
        }
        let shared = document.shared_workbook();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { drop(document::read_shared(&shared)) })
                .await;
            let update = this.update(cx, |this, cx| {
                this.missed.stop_waiting(generation);
                this.retry_missed(id, cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while waiting for a workbook");
            }
        })
        .detach();
    }

    fn retry_missed(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let shown = self.documents.active_id() == Some(id);
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        let generation = document.generation();
        if document.sheets_stale() {
            let sheet_before = document.sheet;
            document.sync_sheets();
            if document.sheets_stale() {
                self.wait_for_workbook(id, cx);
            } else if shown && self.active_sheet() != Some(sheet_before) {
                self.load_sheet_view(cx);
            }
            cx.notify();
        }
        if !shown {
            return;
        }
        let missed = self.missed.take(generation);
        if let Some(refresh) = missed.cells {
            self.fetch_cells(refresh, cx);
        }
        if missed.stats {
            self.refresh_stats(cx);
        }
    }

    fn refresh_chart(&mut self) {
        if let (Some(panel), Some(wb)) = (
            &mut self.chart,
            self.documents.active().and_then(Document::workbook),
        ) {
            panel.refresh(&wb);
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
        self.search = None;
        self.revert_theme_preview(cx);
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
        Some(command_overlay(
            palette::groups(&self.recent)
                .into_iter()
                .fold(Command::new(state), Command::group)
                .placeholder(t!("palette.placeholder"))
                .on_confirm(|_, window, cx| window.dispatch_action(Box::new(ClosePalette), cx))
                .on_cancel(|window, cx| window.dispatch_action(Box::new(ClosePalette), cx)),
            "Palette",
            || Box::new(ClosePalette),
        ))
    }

    fn forget_find_results(&mut self) {
        if let Some(bar) = self
            .documents
            .active_mut()
            .and_then(|document| document.find.as_mut())
        {
            bar.results = FindResults::default();
        }
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.revert_theme_preview(cx);
        if self.find_in_text(false, window, cx) {
            return;
        }
        if self.forward_to_pdf(window, cx, |view, window, cx| view.open_find(window, cx)) {
            return;
        }
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        let input = match &document.find {
            Some(bar) => bar.input.clone(),
            None => {
                let input =
                    cx.new(|cx| InputState::new(window, cx).placeholder(t!("find.placeholder")));
                let subscription = cx.subscribe_in(&input, window, Self::on_find_event);
                self._subscriptions.push(subscription);
                document.find = Some(FindBar {
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
        if self.find_in_text(true, window, cx) {
            return;
        }
        self.open_find(window, cx);
        let Some(bar) = self
            .documents
            .active_mut()
            .and_then(|document| document.find.as_mut())
        else {
            return;
        };
        if bar.replace.is_none() {
            let input = cx
                .new(|cx| InputState::new(window, cx).placeholder(t!("find.replace_placeholder")));
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
        let Some(document) = self.documents.active() else {
            return;
        };
        let Some(bar) = &document.find else {
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
        let sheet = document.sheet;
        // The scan reads every filled cell, so it runs with the edit, off the UI thread.
        self.edit(window, cx, move |wb| {
            let changes = find::replacements(wb, sheet, &query, &replacement);
            if changes.is_empty() {
                return Err(EngineError::Rejected(t!("find.no_match", query = query)));
            }
            // Every replaced cell is one undo entry, so huge replacements are refused
            // whole rather than cut short.
            if changes.len() > MAX_REPLACE_CELLS {
                return Err(EngineError::Rejected(t!(
                    "find.too_many",
                    count = changes.len(),
                    limit = zenkai_i18n::number(MAX_REPLACE_CELLS as u64)
                )));
            }
            wb.set_scattered_inputs(sheet, &changes)
        });
    }

    fn close_chart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chart = None;
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(document) = self.documents.active_mut() {
            document.find = None;
        }
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
        let Some(bar) = self
            .documents
            .active_mut()
            .and_then(|document| document.find.as_mut())
        else {
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
        let Some(document) = self.documents.active() else {
            return;
        };
        if document.has_pending() {
            self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
            return;
        }
        let Some(shared) = document.begin_read() else {
            self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
            return;
        };
        let (id, sheet, generation) = (document.id, document.sheet, document.generation());
        self.busy = Some(t!("busy.searching").into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let task_query = query.clone();
            let matches =
                cx.background_executor()
                    .spawn(async move {
                        find::search(&document::read_shared(&shared), sheet, &task_query)
                    })
                    .await;
            let update = this.update(cx, |this, cx| {
                this.clear_busy(t!("busy.searching"), cx);
                let shown = this.documents.active_id() == Some(id);
                let Some(document) = this.documents.get_mut(id) else {
                    return;
                };
                if !document.is_current(generation) {
                    return;
                }
                if let Some(bar) = &mut document.find {
                    bar.results = FindResults {
                        query,
                        matches,
                        current: 0,
                    };
                    let first = bar.results.matches.first().copied();
                    if let Some(pos) = first.filter(|_| shown) {
                        this.grid.update(cx, |grid, cx| grid.select(pos, pos, cx));
                    }
                }
                cx.notify();
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during search");
            }
        })
        .detach();
    }

    fn render_find(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bar = self.documents.active()?.find.as_ref()?;
        let theme = cx.theme();
        Some(
            h_flex()
                .key_context("FindBar")
                .h(rem_px(36.0))
                .px_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(div().w(rem_px(260.0)).child(Input::new(&bar.input)))
                .children(bar.replace.as_ref().map(|replace| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(rem_px(220.0)).child(Input::new(replace)))
                        .child(
                            Button::new("replace-all")
                                .small()
                                .label(t!("find.replace_all"))
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
        let Some(document) = self.documents.active() else {
            return;
        };
        let Some(workbook) = document.workbook() else {
            return;
        };
        let source = self.grid.read(cx).selection().range();
        self.chart = Some(ChartPanel::new(&workbook, document.sheet, source));
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
            self.notify(Severity::Info, t!("chart.copied_mermaid"), cx);
        }
    }

    fn export_chart_svg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = &self.chart else {
            return;
        };
        let svg = chart::to_svg(&panel.data, panel.kind);
        let directory = self
            .documents
            .active()
            .and_then(|document| document.path.as_ref())
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
                    t!("chart.saved", path = chosen.display()),
                    cx,
                ),
                Err(error) => {
                    this.notify(Severity::Error, t!("chart.save_failed", error = error), cx)
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during chart export");
            }
        })
        .detach();
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        self.stats_request += 1;
        let selection = self.grid.read(cx).selection();
        let Some(document) = self.documents.active() else {
            self.stats = None;
            cx.notify();
            return;
        };
        let sheet = document.sheet;
        let range = selection.range();
        let inline = range.cell_count() <= stats::INLINE_CELLS;
        if !inline {
            self.stats = None;
        }
        let (id, generation) = (document.id, document.generation());
        if let Some(wb) = document.workbook() {
            self.missed.stats_done(generation);
            if inline {
                self.stats = stats::compute(&wb, sheet, range);
            }
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
        } else {
            self.missed.miss_stats(generation);
            self.wait_for_workbook(id, cx);
        }
        if !inline && self.stats_job.request() {
            self.start_stats_job(cx);
        }
        cx.notify();
    }

    // A result is dropped when the selection or the document changed while it ran; a
    // queued request then reruns on the selection as it is at that point.
    fn start_stats_job(&mut self, cx: &mut Context<Self>) {
        let range = self.grid.read(cx).selection().range();
        let Some(document) = self.documents.active() else {
            self.stats_job = StatsJob::Idle;
            return;
        };
        let sheet = document.sheet;
        let shared = document.begin_read();
        let Some(shared) = shared.filter(|_| range.cell_count() > stats::INLINE_CELLS) else {
            self.stats_job = StatsJob::Idle;
            return;
        };
        let (request, generation) = (self.stats_request, document.generation());
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_executor()
                .spawn(async move { stats::compute(&document::read_shared(&shared), sheet, range) })
                .await;
            let update = this.update(cx, |this, cx| {
                let current = this
                    .documents
                    .active()
                    .is_some_and(|document| document.is_current(generation));
                if this.stats_request == request && current {
                    this.stats = stats;
                    cx.notify();
                }
                if this.stats_job.finish() {
                    this.start_stats_job(cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during selection statistics");
            }
        })
        .detach();
    }

    fn notify(
        &mut self,
        severity: Severity,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.post(severity, text.into(), None, Lifetime::UntilReplaced);
        self.expire_warning_later(severity, id, cx);
        cx.notify();
    }

    fn notify_failure(&mut self, failure: Failure, cx: &mut Context<Self>) {
        self.post(
            Severity::Error,
            failure.text.into(),
            Some(failure.detail.into()),
            Lifetime::UntilReplaced,
        );
        cx.notify();
    }

    fn notify_until_open(
        &mut self,
        severity: Severity,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.post(severity, text.into(), None, Lifetime::UntilOpen);
        self.expire_warning_later(severity, id, cx);
        cx.notify();
    }

    fn post(
        &mut self,
        severity: Severity,
        text: SharedString,
        detail: Option<SharedString>,
        lifetime: Lifetime,
    ) -> u64 {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        self.notices
            .post(severity, text, detail, lifetime, time.into())
    }

    fn on_grid_event(
        &mut self,
        _: &Entity<Grid>,
        event: &GridEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            GridEvent::SelectionChanged => {
                self.refresh_stats(cx);
                self.follow_review(cx);
            }
            GridEvent::ViewportChanged => self.schedule_cell_refresh(window, cx),
            GridEvent::EditChanged => {
                let read_only = self.documents.active().is_some_and(|d| d.read_only);
                if read_only && self.grid.read(cx).editor().is_some() {
                    self.grid.update(cx, |grid, cx| grid.take_edit(cx));
                    self.notify(Severity::Warning, t!("notice.read_only_refused"), cx);
                }
                cx.notify();
            }
            GridEvent::EditRequested(_) if self.documents.active().is_some_and(|d| d.read_only) => {
                self.notify(Severity::Warning, t!("notice.read_only_refused"), cx);
            }
            GridEvent::EditRequested(pos) => {
                // Unknown while a recalculation holds the workbook: editing an empty
                // text would clear the cell on Enter.
                let Some(text) = self
                    .documents
                    .active()
                    .and_then(|document| Some((document.workbook()?, document.sheet)))
                    .map(|(wb, sheet)| wb.input(sheet, *pos))
                else {
                    self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
                    return;
                };
                let pos = *pos;
                self.grid.update(cx, |grid, cx| {
                    grid.begin_edit(pos, text, EditMode::Edit, cx)
                });
            }
            GridEvent::Commit { pos, text } => {
                if let Some(sheet) = self.active_sheet() {
                    self.forget_review_range(sheet, Range::single(*pos), cx);
                    self.commit_text(sheet, *pos, text.clone(), window, cx);
                }
            }
            GridEvent::CommitToSelection { pos, text, range } => {
                let Some(sheet) = self.active_sheet() else {
                    return;
                };
                let (pos, text, range) = (*pos, text.clone(), *range);
                self.forget_review_range(sheet, range, cx);
                self.show_typed(range, &text, cx);
                self.edit(window, cx, move |wb| wb.fill_with(sheet, pos, &text, range));
            }
            GridEvent::ClearRequested(range) => {
                let Some(sheet) = self.active_sheet() else {
                    return;
                };
                let range = *range;
                self.forget_review_range(sheet, range, cx);
                self.edit(window, cx, move |wb| wb.clear(sheet, range));
            }
            GridEvent::Jump { direction, extend } => self.jump(*direction, *extend, cx),
            GridEvent::ColumnResized { col, width } => {
                let Some(sheet) = self.active_sheet() else {
                    return;
                };
                let (col, width) = (*col, *width);
                self.edit(window, cx, move |wb| wb.set_column_width(sheet, col, width));
            }
            GridEvent::RowResized { row, height } => {
                let Some(sheet) = self.active_sheet() else {
                    return;
                };
                let (row, height) = (*row, *height);
                self.edit(window, cx, move |wb| wb.set_row_height(sheet, row, height));
            }
            GridEvent::FillRequested { source, target } => {
                let Some(sheet) = self.active_sheet() else {
                    return;
                };
                let (source, target) = (*source, *target);
                self.edit(window, cx, move |wb| wb.extend(sheet, source, target));
            }
            GridEvent::AutoFitRequested(col) => self.auto_fit(*col, window, cx),
            GridEvent::EndRequested { extend } => {
                let Some(end) = self
                    .documents
                    .active()
                    .and_then(|document| Some((document.workbook()?, document.sheet)))
                    .map(|(wb, sheet)| wb.used_end(sheet))
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
        let Some(document) = self.documents.active() else {
            return;
        };
        let Some(workbook) = document.workbook() else {
            return;
        };
        let sheet = document.sheet;
        let selection = self.grid.read(cx).selection();
        let from = if extend {
            selection.corner
        } else {
            selection.active
        };
        let used_end = workbook.used_end(sheet);
        let Some(contents) = document::contents_of(&workbook, sheet) else {
            return;
        };
        let target = jump_target(from, direction, used_end, |pos| contents(pos).is_filled());
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
    ) -> bool {
        if self.prompts.blocks_input() {
            return false;
        }
        // Any edit ends copy mode, as in Excel, so a later paste never reads cells or
        // sheets that changed since the copy.
        if self.clipboard_source.take().is_some() {
            self.grid.update(cx, |grid, cx| grid.set_marquee(None, cx));
        }
        let Some(id) = self.documents.active_id() else {
            return false;
        };
        self.edit_document(id, window, cx, edit)
    }

    fn edit_document(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Workbook) -> Result<(), EngineError> + Send + 'static,
    ) -> bool {
        let shown = self.documents.active_id() == Some(id);
        let Some(document) = self.documents.get_mut(id) else {
            return false;
        };
        if document.queue_user_edit(Box::new(edit)).is_err() {
            self.notify(Severity::Warning, t!("notice.read_only_refused"), cx);
            return false;
        }
        if shown {
            window.set_window_title(&self.documents.shown_title());
        }
        self.flush_edits(id, cx);
        true
    }

    fn show_typed(&mut self, range: Range, text: &str, cx: &mut Context<Self>) {
        if let Some(document) = self.documents.active() {
            self.previews.add(
                document.generation(),
                document.sheet,
                range,
                text.to_string().into(),
            );
        }
        self.grid
            .update(cx, |grid, cx| grid.show_typed(range, text, cx));
    }

    fn flush_edits(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        let Some((shared, edits)) = document.take_batch() else {
            return;
        };
        let generation = document.generation();
        let stale = StaleRequest {
            slot: document.stale_slot(),
            sheet: document.sheet,
            formulas: self.show_formulas,
            viewport: self.grid.read(cx).cached_ranges(),
        };
        self.previews.batch_started(generation);
        self.busy = Some(t!("busy.calculating").into());
        self.start_long_job(LongJob::Calculating, cx);
        let started = Instant::now();
        cx.spawn(async move |this, cx| {
            let errors = cx
                .background_executor()
                .spawn(async move { document::run_batch_serving(&shared, edits, Some(stale)) })
                .await;
            let update = this.update(cx, |this, cx| {
                this.previews.batch_finished(generation);
                let shown = this.documents.active_id() == Some(id);
                let Some(document) = this.documents.get_mut(id) else {
                    this.clear_calculating(cx);
                    return;
                };
                let sheets_before = document.sheets.len();
                let sheet_before = document.sheet;
                let pending_sheet = document.pending_sheet.take();
                if !document.finish_batch(generation) {
                    this.clear_calculating(cx);
                    return;
                }
                document.revalidate_review();
                if let Some(target) = pending_sheet
                    && errors.is_empty()
                {
                    document.sheet = target;
                }
                let sheet_changed =
                    document.sheet != sheet_before || document.sheets.len() != sheets_before;
                if document.sheets_stale() {
                    this.wait_for_workbook(id, cx);
                }
                this.last_recalc = Some(started.elapsed());
                if let Some(error) = errors.first() {
                    this.notify(Severity::Error, error.to_string(), cx);
                }
                if shown {
                    if sheet_changed {
                        this.load_sheet_view(cx);
                    } else {
                        // Edits can insert rows, resize, merge or freeze; refresh the
                        // layout without moving the selection.
                        let view = this.sheet_view();
                        this.grid.update(cx, |grid, cx| grid.update_view(view, cx));
                    }
                    this.sync_review_marks(cx);
                    this.refresh_cells(cx);
                    this.refresh_stats(cx);
                    this.refresh_chart();
                    this.forget_find_results();
                }
                this.clear_calculating(cx);
                this.flush_edits(id, cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during recalc");
            }
        })
        .detach();
    }

    // Another document may still be recalculating; the label stays until all are done.
    fn clear_calculating(&mut self, cx: &mut Context<Self>) {
        self.job_pill.end(LongJob::Calculating);
        if !self.documents.iter().any(Document::has_pending) {
            self.clear_busy(t!("busy.calculating"), cx);
        }
    }

    pub(crate) fn start_long_job(&mut self, job: LongJob, cx: &mut Context<Self>) {
        self.job_pill.start(job, Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOW_AFTER).await;
            if let Err(error) = this.update(cx, |_, cx| cx.notify()) {
                tracing::debug!(%error, "workspace closed before the progress pill");
            }
        })
        .detach();
    }

    pub(crate) fn end_long_job(&mut self, job: LongJob, cx: &mut Context<Self>) {
        self.job_pill.end(job);
        cx.notify();
    }

    fn busy_label(&self) -> Option<SharedString> {
        match self.open_loads.newest_label() {
            Some(label) => Some(label.to_owned().into()),
            None => self.busy.clone(),
        }
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
        let read = self
            .open_loads
            .start(t!("busy.reading", name = file_label(&path)));
        cx.notify();
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
                this.open_loads.finish(read);
                cx.notify();
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
        let summary = t!(
            "csv.imported",
            name = preview
                .path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            delimiter = crate::csv_preview::delimiter_label(preview.parsed.delimiter),
            encoding = preview.parsed.encoding.label()
        );
        self.busy = Some(t!("busy.importing").into());
        cx.notify();
        let CsvPreview {
            path,
            bytes,
            parsed,
            guess,
        } = preview;
        let delimiter = parsed.delimiter;
        let mut rows = parsed.rows;
        let origin_path = path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task_bytes = bytes.clone();
            let (result, origin) = cx
                .background_executor()
                .spawn(async move {
                    let origin = file_origin(&origin_path);
                    csv_preview::apply(guess, &mut rows);
                    // On failure, parse again so the preview comes back as it was.
                    let result = files::workbook_from_rows(rows)
                        .map_err(|error| (error, parse_csv(&task_bytes, Some(delimiter)).ok()));
                    (result, origin)
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(t!("busy.importing"), cx);
                match result {
                    Ok(_) if this.csv_request != request => {}
                    Ok(workbook) => {
                        let id = this.open_document(workbook, None, Vec::new(), window, cx);
                        if let Some(document) = this.documents.get_mut(id) {
                            document.origin = origin;
                        }
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

    fn announce_interface_size(&mut self, cx: &mut Context<Self>) {
        self.notify(
            Severity::Info,
            t!(
                "notice.interface_size",
                percent = interface_scale::shown(cx).percent()
            ),
            cx,
        );
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
                .top(rem_px(96.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(csv_preview::render(preview, &self.focus, on_event, cx)),
        )
    }

    fn export_csv(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        let (sheet, sheets) = (document.sheet, document.sheets.len());
        let Some(shared) = document.begin_file_job(FileJob::Saving) else {
            let reason = file_job_refusal(document.file_job());
            self.notify(Severity::Warning, reason, cx);
            self.save_ended(id, false, window, cx);
            return;
        };
        let generation = document.generation();
        self.busy = Some(t!("busy.saving").into());
        self.start_long_job(LongJob::Saving, cx);
        cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move { files::export_sheet_csv(&shared, sheet, &target) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(t!("busy.saving"), cx);
                this.end_long_job(LongJob::Saving, cx);
                let mut follow_up = AfterJob::Nothing;
                if let Some(document) = this.documents.get_mut(id) {
                    document.end_file_job(generation);
                    // A save pressed meanwhile still owes the workbook file: a CSV is not it.
                    follow_up = document.settle_save(generation, JobEnd::Autosaved);
                }
                this.csv_export_ended(id, &path, sheets, written, window, cx);
                if follow_up == AfterJob::SaveAgain {
                    this.save_again(id, window, cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during export");
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
        self.open_path(path, window, cx);
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_and_open(window, cx);
    }

    fn pick_and_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("dialog.open_button").into()),
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
        if let Some(id) = self.documents.shown_text().map(|text| text.id) {
            self.save_text(id, window, cx);
            return;
        }
        let Some(id) = self.documents.active_id() else {
            return;
        };
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        if document.is_following_up() {
            return;
        }
        if document.file_job() != FileJob::Idle {
            document.request_save();
        } else if document.has_pending() {
            self.save_again(id, window, cx);
        } else {
            self.save_document(id, window, cx);
        }
    }

    // After an edit batch or another file job the save cannot start yet; it waits for them,
    // and gives up after a while so a stuck job never leaves a wait running forever.
    fn save_again(&mut self, id: WorkbookId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        if !document.begin_follow_up() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let started = Instant::now();
            loop {
                let waiting = this.update_in(cx, |this, window, cx| {
                    let Some(document) = this.documents.get_mut(id) else {
                        return false;
                    };
                    if this.lifecycle == Lifecycle::Closing {
                        document.clear_pending_save();
                        return false;
                    }
                    if document.has_pending() || document.file_job() != FileJob::Idle {
                        if started.elapsed() < SAVE_AGAIN_LIMIT {
                            return true;
                        }
                        let reason = file_job_refusal(document.file_job());
                        document.clear_pending_save();
                        this.notify(Severity::Warning, reason, cx);
                        return false;
                    }
                    document.clear_pending_save();
                    this.save_document(id, window, cx);
                    false
                });
                match waiting {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        tracing::debug!(%error, "workspace closed while a save waited");
                        break;
                    }
                }
                cx.background_executor().timer(SAVE_AGAIN_POLL).await;
            }
        })
        .detach();
    }

    fn save_document(&mut self, id: WorkbookId, window: &mut Window, cx: &mut Context<Self>) {
        if self.documents.text(id).is_some() {
            self.save_text(id, window, cx);
            return;
        }
        if self.refuse_during_save_as(id, cx) {
            return;
        }
        let Some(document) = self.documents.get(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        let must_rename =
            document.may_lose_unseen_content() || document.is_macro_enabled() || document.read_only;
        match document.path.clone() {
            Some(path) if !must_rename => self.save_to(
                id,
                path,
                Some(Overwrite::InPlace),
                ChangedFile::Ask,
                window,
                cx,
            ),
            Some(_) => {
                let held = document::labels(&document.unsupported);
                let detail = if document.read_only {
                    t!("save.read_only_detail").to_string()
                } else if document.is_macro_enabled() {
                    let mut text = t!("save.macros_detail").to_string();
                    if !document.unsupported.is_empty() {
                        text.push(' ');
                        text.push_str(&t!("save.also_lost", list = held));
                    }
                    text
                } else {
                    t!("save.loses_detail", list = held)
                };
                self.offer_save_as(id, &detail, window, cx);
            }
            None => self.save_as_dialog(id, window, cx),
        }
    }

    fn offer_save_as(
        &mut self,
        id: WorkbookId,
        detail: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = self.ask(
            PromptLevel::Warning,
            t!("save.cannot_save_title"),
            Some(detail),
            &[t!("button.save_as"), t!("button.cancel")],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| {
                if choice == Some(0) {
                    this.save_as_dialog(id, window, cx)
                } else {
                    this.save_ended(id, false, window, cx)
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during save prompt");
            }
        })
        .detach();
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.close_formula_bar(true, window, cx);
        if let Some(id) = self.documents.shown_text().map(|text| text.id) {
            self.save_text_as(id, window, cx);
        } else if let Some(id) = self.documents.active_id() {
            self.save_as_dialog(id, window, cx);
        }
    }

    fn save_as_dialog(&mut self, id: WorkbookId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.begin_save_as(id, cx) {
            return;
        }
        let Some(document) = self.documents.get(id) else {
            self.save_as_flows.finish(id);
            self.save_ended(id, false, window, cx);
            return;
        };
        let directory = document
            .path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let suggested = document
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(
                || {
                    document
                        .name_with_marker()
                        .trim_start_matches("• ")
                        .to_string()
                },
                |stem| files::copy_stem(&stem.to_string_lossy()),
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
            let update = this.update_in(cx, |this, window, cx| match chosen {
                Some(path) => this.confirm_target(id, path, window, cx),
                None => {
                    this.save_as_flows.finish(id);
                    this.save_ended(id, false, window, cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during save dialog");
            }
        })
        .detach();
    }

    fn confirm_target(
        &mut self,
        id: WorkbookId,
        chosen: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = match self.documents.get(id) {
            Some(document) => document.path.clone(),
            None => {
                self.save_as_flows.finish(id);
                self.save_ended(id, false, window, cx);
                return;
            }
        };
        self.save_as_flows.start_checking(id);
        let delimited = files::is_delimited_text(&chosen);
        let path = if delimited {
            chosen.clone()
        } else {
            files::xlsx_target(&chosen)
        };
        let renamed = path != chosen;
        cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let probed = source.clone();
            let probe = cx
                .background_executor()
                .spawn(async move { files::probe_target(probed.as_deref(), &target) })
                .await;
            let checked = save_flow::CheckedTarget {
                path,
                source,
                probe,
            };
            let update = this.update_in(cx, |this, window, cx| {
                if delimited {
                    this.decide_export(id, checked, window, cx)
                } else {
                    this.decide_target(id, renamed, checked, window, cx)
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while checking the save target");
            }
        })
        .detach();
    }

    fn decide_export(
        &mut self,
        id: WorkbookId,
        checked: save_flow::CheckedTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let save_flow::CheckedTarget {
            path,
            source,
            probe,
        } = checked;
        self.save_as_flows.finish(id);
        let Some(document) = self.documents.get(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        if source != document.path {
            self.notify(Severity::Warning, t!("save.target_stale"), cx);
            self.save_ended(id, false, window, cx);
            return;
        }
        match delimited_action(&probe) {
            DelimitedAction::Export => self.export_csv(id, path, window, cx),
            DelimitedAction::ConfirmReplacing => {
                self.confirm_replace(id, path, Replacing::ExistingExport, window, cx);
            }
        }
    }

    fn decide_target(
        &mut self,
        id: WorkbookId,
        renamed: bool,
        checked: save_flow::CheckedTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let save_flow::CheckedTarget {
            path,
            source,
            probe,
        } = checked;
        self.save_as_flows.finish(id);
        let Some(document) = self.documents.get(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        if source != document.path {
            self.notify(Severity::Warning, t!("save.target_stale"), cx);
            self.save_ended(id, false, window, cx);
            return;
        }
        let action = target_action(
            &probe,
            renamed,
            document.is_macro_enabled() || document.read_only,
            document.may_lose_unseen_content(),
        );
        match action {
            TargetAction::RefuseOriginal => {
                self.notify(Severity::Error, t!("save.original_protected"), cx);
                self.save_ended(id, false, window, cx);
            }
            TargetAction::ConfirmReplacingOriginal => {
                let lost = document.unsupported.clone();
                self.confirm_replace(id, path, Replacing::Original(lost), window, cx);
            }
            TargetAction::SaveOverOriginal => self.save_to(
                id,
                path,
                Some(Overwrite::Original),
                ChangedFile::Ask,
                window,
                cx,
            ),
            TargetAction::ConfirmReplacingExisting => {
                self.confirm_replace(id, path, Replacing::Existing, window, cx);
            }
            TargetAction::SaveNew => self.save_to(id, path, None, ChangedFile::Ask, window, cx),
        }
    }

    fn confirm_replace(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        replacing: Replacing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exports = matches!(replacing, Replacing::ExistingExport);
        let (title, detail, overwrite) = match replacing {
            Replacing::Existing | Replacing::ExistingExport => (
                t!("save.replace_existing_title"),
                t!("save.replace_existing_detail", path = path.display()),
                None,
            ),
            Replacing::Original(lost) => (
                t!("save.replace_original_title"),
                t!(
                    "save.replace_original_detail",
                    list = document::labels(&lost)
                ),
                Some(Overwrite::Confirmed(lost)),
            ),
        };
        let answer = self.ask(
            PromptLevel::Critical,
            title,
            Some(&detail),
            &[t!("button.cancel"), t!("button.replace")],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| {
                if choice == Some(1) && exports {
                    this.export_csv(id, path, window, cx)
                } else if choice == Some(1) {
                    this.save_to(id, path, overwrite, ChangedFile::Ask, window, cx)
                } else {
                    this.save_ended(id, false, window, cx)
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during replace prompt");
            }
        })
        .detach();
    }

    fn confirm_changed_file(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        overwrite: Overwrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .documents
            .get(id)
            .map(Document::name)
            .unwrap_or_default();
        let answer = self.ask(
            PromptLevel::Warning,
            &t!("save.changed_on_disk_title", name = name),
            Some(t!("save.changed_on_disk_detail")),
            &changed_file_answers(),
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| match choice {
                Some(CHANGED_FILE_REPLACE) => {
                    this.save_to(id, path, Some(overwrite), ChangedFile::Replace, window, cx)
                }
                Some(CHANGED_FILE_SAVE_AS) => this.save_as_dialog(id, window, cx),
                _ => this.save_ended(id, false, window, cx),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during changed file prompt");
            }
        })
        .detach();
    }

    fn save_to(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        // None for a file Save As gave a name of its own.
        overwrite: Option<Overwrite>,
        changed: ChangedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        let Some(shared) = document.begin_file_job(FileJob::Saving) else {
            let reason = file_job_refusal(document.file_job());
            self.notify(Severity::Warning, reason, cx);
            self.save_ended(id, false, window, cx);
            return;
        };
        let edits_at_start = document.edit_count();
        let generation = document.generation();
        let guard = document.file_guard();
        self.busy = Some(t!("busy.saving").into());
        self.start_long_job(LongJob::Saving, cx);
        cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let Some(overwrite) = overwrite else {
                        return document::write_document_copy(&shared, &target, save_xlsx)
                            .map(|kept| Saved::Written { kept });
                    };
                    document::save_document_file(
                        &shared,
                        &target,
                        overwrite.accepted(),
                        changed,
                        guard,
                        save_xlsx,
                    )
                    .map(|saved| match saved {
                        DocumentSave::Written { kept } => Saved::Written { kept },
                        DocumentSave::Refused { dropped } => Saved::Refused { dropped, overwrite },
                        DocumentSave::BaseChanged => Saved::BaseChanged { overwrite },
                    })
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(t!("busy.saving"), cx);
                this.end_long_job(LongJob::Saving, cx);
                let shown = this.documents.active_id() == Some(id);
                let wrote = matches!(result, Ok(Saved::Written { .. }));
                let mut follow_up = AfterJob::Nothing;
                let current = this.documents.get_mut(id).is_some_and(|document| {
                    document.end_file_job(generation);
                    let current = document.is_current(generation);
                    if current {
                        let end = if wrote {
                            JobEnd::Saved {
                                edited_meanwhile: document.edit_count() != edits_at_start,
                            }
                        } else {
                            JobEnd::Failed
                        };
                        follow_up = document.settle_save(generation, end);
                    }
                    current
                });
                let Some(document) = this.documents.get_mut(id).filter(|_| current) else {
                    if let Err(error) = result {
                        this.notify_failure(save_failure(&path, &error), cx);
                    }
                    this.save_ended(id, false, window, cx);
                    return;
                };
                let fully_saved;
                match result {
                    Ok(Saved::Written { kept }) => {
                        document.saved_over_file(kept);
                        document.path = Some(path);
                        let edited_meanwhile = document.edit_count() != edits_at_start;
                        document.dirty = edited_meanwhile;
                        fully_saved = !edited_meanwhile;
                        if shown {
                            window.set_window_title(&this.documents.shown_title());
                        }
                        this.notify(Severity::Info, t!("notice.saved"), cx);
                    }
                    // Nothing was written; the file on disk is as it was.
                    Ok(Saved::Refused { dropped, overwrite }) => {
                        this.flush_edits(id, cx);
                        match overwrite {
                            Overwrite::InPlace => {
                                let list = document::labels(&dropped);
                                let detail = t!("save.loses_detail", list = list);
                                this.offer_save_as(id, &detail, window, cx);
                            }
                            Overwrite::Original | Overwrite::Confirmed(_) => {
                                let replacing = Replacing::Original(dropped);
                                this.confirm_replace(id, path, replacing, window, cx);
                            }
                        }
                        return;
                    }
                    Ok(Saved::BaseChanged { overwrite }) => {
                        this.flush_edits(id, cx);
                        this.confirm_changed_file(id, path, overwrite, window, cx);
                        return;
                    }
                    Err(error) => {
                        let asks_again = document.path.as_ref() == Some(&path);
                        this.notify_failure(
                            offer_another_location(save_failure(&path, &error)),
                            cx,
                        );
                        this.flush_edits(id, cx);
                        // Locked by another program or read-only: offer Save As right away,
                        // unless this already was a new location picked in Save As.
                        if asks_again && shown {
                            this.save_as_dialog(id, window, cx);
                        } else {
                            this.save_ended(id, false, window, cx);
                        }
                        return;
                    }
                }
                this.flush_edits(id, cx);
                this.save_ended(id, fully_saved, window, cx);
                if follow_up == AfterJob::SaveAgain {
                    this.save_again(id, window, cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during save");
            }
        })
        .detach();
    }

    fn new_workbook(&mut self, _: &NewWorkbook, window: &mut Window, cx: &mut Context<Self>) {
        self.create_document(window, cx);
    }

    fn selection(&self, cx: &App) -> Range {
        self.grid.read(cx).selection().range()
    }

    fn style(&mut self, change: StyleChange, window: &mut Window, cx: &mut Context<Self>) {
        self.last_style = Some(change);
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let range = self.selection(cx);
        self.edit(window, cx, move |wb| {
            wb.as_one_undo_step(|wb| {
                wb.apply_style(sheet, range, change)?;
                if matches!(change, StyleChange::NumberFormat(_)) {
                    widen_for_numbers(wb, sheet, range)?;
                }
                Ok(())
            })
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
            .documents
            .active()
            .and_then(|document| Some((document.workbook()?, document.sheet)))
            .is_some_and(|(wb, sheet)| read(&wb.cell(sheet, active).style));
        self.style(make(!current), window, cx);
    }

    // Ctrl+Shift+> and < move through Excel's font size list from the active cell's size.
    fn step_font_size(&mut self, grow: bool, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.grid.read(cx).selection().active;
        let current = self
            .documents
            .active()
            .and_then(|document| Some((document.workbook()?, document.sheet)))
            .and_then(|(wb, sheet)| wb.cell(sheet, active).style.font_size)
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
        let Some(document) = self.documents.active() else {
            return;
        };
        let sheet = document.sheet;
        let outcome = document.workbook_mut().map(|mut workbook| {
            let end = workbook.used_end(sheet);
            let clipped = range.clip_to(end).unwrap_or(Range::single(range.start));
            if clipped.cell_count() > clipboard::MAX_COPY_CELLS {
                return None;
            }
            Some(workbook.copy(sheet, clipped))
        });
        match outcome {
            None => self.notify(Severity::Warning, t!("notice.still_calculating"), cx),
            Some(None) => {
                self.notify(
                    Severity::Warning,
                    t!("notice.selection_too_large_to_copy"),
                    cx,
                );
            }
            Some(Some(Ok(copied))) => {
                cx.write_to_clipboard(ClipboardItem::new_string(copied.text.clone()));
                self.clipboard_source = Some(InternalClip {
                    copied,
                    cut,
                    range,
                    sheet,
                });
                self.grid
                    .update(cx, |grid, cx| grid.set_marquee(Some(range), cx));
            }
            Some(Some(Err(error))) => self.notify(Severity::Error, error.to_string(), cx),
        }
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(sheet) = self.active_sheet() else {
            return;
        };
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
        let cut_source = internal
            .as_ref()
            .filter(|clip| clip.cut)
            .map(|clip| clip.range);
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
        self.forget_review_range(sheet, Range::new(origin, end), cx);
        if let Some(source) = cut_source {
            self.forget_review_range(sheet, source, cx);
        }
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
        let Some(sheet) = self.active_sheet() else {
            return;
        };
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
        self.forget_review_range(sheet, Range::new(origin, end), cx);
        self.grid.update(cx, |grid, cx| {
            grid.set_marquee(None, cx);
            grid.select(origin, end, cx);
        });
    }

    fn switch_sheet(&mut self, sheet: SheetId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        if sheet.0 as usize >= document.sheets.len() || sheet == document.sheet {
            return;
        }
        document.sheet = sheet;
        self.reset_grid(window, cx);
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        TitleBar::new().h(rem_px(34.0)).child(
            h_flex()
                .size_full()
                .items_center()
                .text_sm()
                .child(
                    h_flex()
                        .w(rem_px(160.0))
                        .items_center()
                        .gap_2()
                        .text_color(muted)
                        .child(
                            div().occlude().child(
                                Button::new("toggle-sidebar")
                                    .ghost()
                                    .compact()
                                    .icon(if self.sidebar.visible {
                                        gpui_kit::assets::IconName::PanelLeftClose
                                    } else {
                                        gpui_kit::assets::IconName::PanelLeftOpen
                                    })
                                    .tooltip(keymap::labeled(
                                        cx,
                                        if self.sidebar.visible {
                                            t!("sidebar.hide_tooltip")
                                        } else {
                                            t!("sidebar.show_tooltip")
                                        },
                                        &ToggleSidebar,
                                    ))
                                    .on_click(|event, window, cx| {
                                        if sidebar::is_primary_click(event) {
                                            window.dispatch_action(ToggleSidebar.boxed_clone(), cx)
                                        }
                                    }),
                            ),
                        )
                        .child("Zenkai"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_center()
                        .child(
                            self.documents
                                .shown_name_with_marker()
                                .unwrap_or_else(|| "Zenkai".to_string()),
                        ),
                )
                .child(
                    h_flex()
                        .w(rem_px(160.0))
                        .items_center()
                        .justify_end()
                        .text_color(muted)
                        .child(self.render_chat_toggle(cx)),
                ),
        )
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
            .h(rem_px(32.0))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(match &self.go_to {
                Some((input, _)) => div()
                    .key_context("NameBox")
                    .w(rem_px(120.0))
                    .child(Input::new(input))
                    .into_any_element(),
                None => div()
                    .id("name-box")
                    .role(Role::Button)
                    .aria_label(t!("grid.name_box_label", name = name))
                    .w(rem_px(96.0))
                    .h(rem_px(24.0))
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
                    .aria_label(t!("grid.formula_bar_label"))
                    .flex_1()
                    .child(Input::new(&bar.input))
            }))
            .when(self.formula_bar.is_none(), |row| {
                row.child(
                    div()
                        .id("formula-content")
                        .role(Role::TextInput)
                        .aria_label(t!("grid.formula_bar_label"))
                        .aria_value(content.clone())
                        .flex_1()
                        .h(rem_px(24.0))
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
        let active = self.documents.active();
        let sheets = active.map_or(&[][..], |document| document.sheets.as_slice());
        let tabs = sheets.iter().map(|sheet| {
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
            .h(rem_px(32.0))
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
                            .selected_index(active.map_or(0, |document| document.sheet.0 as usize))
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
                                    if double && this.active_sheet() == Some(sheet) {
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
        let Some(sheet) = self.active_sheet() else {
            return;
        };
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
            self.notify(Severity::Warning, t!("notice.select_rows_or_columns"), cx);
            return;
        }
        match action {
            StructureEdit::InsertRows => {
                self.edit(window, cx, move |wb| {
                    wb.insert_rows(sheet, selection.start.row, rows)
                });
            }
            StructureEdit::DeleteRows => {
                self.edit(window, cx, move |wb| {
                    wb.delete_rows(sheet, selection.start.row, rows)
                });
            }
            StructureEdit::InsertColumns => {
                self.edit(window, cx, move |wb| {
                    wb.insert_columns(sheet, selection.start.col, cols)
                });
            }
            StructureEdit::DeleteColumns => {
                self.edit(window, cx, move |wb| {
                    wb.delete_columns(sheet, selection.start.col, cols)
                });
            }
            StructureEdit::FreezePanes => {
                let frozen = self
                    .documents
                    .active()
                    .and_then(Document::workbook)
                    .map(|wb| wb.frozen(sheet))
                    .unwrap_or_default();
                let (rows, cols) = if frozen == (0, 0) {
                    (active.row.get(), active.col.get())
                } else {
                    (0, 0)
                };
                if (rows, cols) != (0, 0) && !self.grid.read(cx).is_on_screen(active) {
                    self.notify(
                        Severity::Warning,
                        t!("notice.freeze_needs_visible_cell"),
                        cx,
                    );
                    return;
                }
                self.edit(window, cx, move |wb| wb.set_frozen(sheet, rows, cols));
            }
        }
    }

    // Ctrl+- and Ctrl++ reach the grid while a cell is being typed in; they must not edit the sheet.
    fn insert_or_delete_selection(
        &mut self,
        insert: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.grid.read(cx).editor().is_some() {
            return;
        }
        match selection_edit(insert, self.selection(cx)) {
            Some(edit) => self.structure_edit(window, cx, edit),
            None => self.notify(Severity::Warning, t!("notice.select_rows_or_columns"), cx),
        }
    }

    // With a single cell selected, Excel sorts its current region.
    fn sort(&mut self, descending: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        // Taken before the region is selected, which moves the active cell to its corner.
        let key = self.grid.read(cx).selection().active.col;
        let mut range = self.selection(cx);
        if range.cell_count() == 1 {
            range = self.select_current_region(cx);
        }
        if range.rows() < 2 {
            self.notify(Severity::Info, t!("notice.nothing_to_sort"), cx);
            return;
        }
        self.forget_review_range(sheet, range, cx);
        self.edit(window, cx, move |wb| wb.sort(sheet, range, key, descending));
    }

    // The new format comes from the active cell, as in Excel, and applies to the selection.
    fn step_decimals(&mut self, more: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Not expressible as a repeatable StyleChange; F4 must not repeat an older one.
        self.last_style = None;
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let range = self.selection(cx);
        let active = self.grid.read(cx).selection().active;
        let Some(view) = self
            .documents
            .active()
            .and_then(Document::workbook)
            .map(|workbook| workbook.cell(sheet, active))
        else {
            self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
            return;
        };
        let Some(code) = decimals::step_decimals(
            &view.style.num_fmt,
            (view.kind == zenkai_types::ValueKind::Number).then_some(view.text.as_str()),
            more,
        ) else {
            return;
        };
        self.edit(window, cx, move |wb| {
            wb.as_one_undo_step(|wb| {
                wb.set_number_format(sheet, range, &code)?;
                widen_for_numbers(wb, sheet, range)
            })
        });
    }

    fn toggle_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let range = self.selection(cx);
        let active = self.grid.read(cx).selection().active;
        let Some(wrapped) = self
            .documents
            .active()
            .and_then(Document::workbook)
            .map(|wb| wb.cell(sheet, active).style.wrap)
        else {
            self.notify(Severity::Warning, t!("notice.still_calculating"), cx);
            return;
        };
        let wrap = !wrapped;
        self.edit(window, cx, move |wb| {
            wb.as_one_undo_step(|wb| {
                wb.apply_style(sheet, range, StyleChange::Wrap(wrap))?;
                if !wrap {
                    return Ok(());
                }
                grow_wrapped_rows(wb, sheet, range)
            })
        });
    }

    // The rows (or columns) the selection touches, as Excel's Hide and Unhide.
    fn hide(&mut self, rows: bool, hidden: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let range = self.selection(cx);
        let whole_sheet = if rows {
            range.rows() == zenkai_types::MAX_ROWS
        } else {
            range.cols() == zenkai_types::MAX_COLS
        };
        if hidden && whole_sheet {
            self.notify(Severity::Warning, t!("notice.cannot_hide_everything"), cx);
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
        let Some(document) = self.documents.active() else {
            return;
        };
        let (sheet, generation) = (document.sheet, document.generation());
        let view = document
            .workbook()
            .map(|wb| wb.cell(sheet, active))
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
            sheet,
            generation,
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
                t!("notice.invalid_number_format", error = error),
                cx,
            );
            return;
        }
        // Applied where it was opened, never to another sheet or document.
        let unchanged = self.documents.active().is_some_and(|document| {
            dialog.sheet == document.sheet && dialog.generation == document.generation()
        });
        if !unchanged {
            self.close_format_dialog(window, cx);
            self.notify(
                Severity::Warning,
                t!("notice.sheet_changed_reopen_format"),
                cx,
            );
            return;
        }
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        self.edit(window, cx, move |wb| {
            wb.as_one_undo_step(|wb| {
                wb.set_number_format(sheet, range, &code)?;
                widen_for_numbers(wb, sheet, range)
            })
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
            .unwrap_or_else(|_| t!("format.invalid_preview").to_string());
        let input = dialog.code.clone();
        let on_category = move |code: &'static str, window: &mut Window, cx: &mut App| {
            input.update(cx, |state, cx| state.set_value(code, window, cx));
        };
        Some(
            div()
                .absolute()
                .top(rem_px(96.0))
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
        if self.active_sheet() == Some(sheet) {
            self.show_typed(Range::single(pos), &text, cx);
        }
        self.edit(window, cx, move |wb| enter_text(wb, sheet, pos, &text));
    }

    // Editing in the formula bar: Enter or leaving the bar enters the text in the
    // cell, Esc leaves it unchanged (Excel's behaviour without point mode).
    fn open_formula_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        if self.formula_bar.is_some() || document.read_only {
            return;
        }
        let pos = self.grid.read(cx).selection().active;
        // While a recalculation has the workbook the cell's text is unknown; opening
        // with an empty bar would then clear the cell on Enter.
        let Some(original) = document.workbook().map(|wb| wb.input(document.sheet, pos)) else {
            return;
        };
        let (sheet, generation) = (document.sheet, document.generation());
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
            sheet,
            generation,
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
        let same_document = self
            .documents
            .active()
            .is_some_and(|document| document.generation() == bar.generation);
        if commit && same_document {
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
        let Some(sheet) = self.active_sheet() else {
            return Range::single(active);
        };
        let Some(workbook) = self.documents.active().and_then(Document::workbook) else {
            return Range::single(active);
        };
        let Some(contents) = document::contents_of(&workbook, sheet) else {
            return Range::single(active);
        };
        let region = region::current_region(active, |pos| contents(pos).is_filled());
        self.grid
            .update(cx, |grid, cx| grid.select(region.start, region.end, cx));
        region
    }

    fn freeze(&mut self, rows: u32, cols: u16, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        self.edit(window, cx, move |wb| wb.set_frozen(sheet, rows, cols));
    }

    fn fill(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let range = self.selection(cx);
        self.edit(window, cx, move |wb| wb.fill(sheet, range, down));
    }

    // Alt+= proposes =SUM over the numbers right above (or to the left), in edit mode.
    // With a range selected it writes the totals directly, in one undo step.
    fn auto_sum(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbook) = self.documents.active().and_then(Document::workbook) else {
            return;
        };
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let active = self.grid.read(cx).selection().active;
        let Some(contents) = document::contents_of(&workbook, sheet) else {
            return;
        };
        let selection = self.selection(cx);
        if selection.start != selection.end {
            let totals = auto_sum::totals(selection, |pos| contents(pos).is_filled());
            drop(contents);
            drop(workbook);
            self.edit(window, cx, move |wb| {
                wb.as_one_undo_step(|wb| {
                    for total in &totals {
                        wb.set_input(sheet, total.target, &format!("=SUM({})", total.sum))?;
                    }
                    Ok(())
                })
            });
            return;
        }
        let is_number = |pos: CellPos| matches!(contents(pos), Contents::Number(_));
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
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let active = self.grid.read(cx).selection().active;
        if !time {
            // An ISO date is recognised and formatted by the engine in one undo step.
            let text = now.format("%Y-%m-%d").to_string();
            self.edit(window, cx, move |wb| enter_text(wb, sheet, active, &text));
            return;
        }
        let midnight = now.date().and_hms_opt(0, 0, 0).unwrap_or(now);
        // Excel stores a time of day as the fraction of a day.
        let fraction = (now - midnight).num_milliseconds() as f64 / 86_400_000.0;
        let text = fraction.to_string();
        self.edit(window, cx, move |wb| {
            wb.as_one_undo_step(|wb| {
                wb.set_input(sheet, active, &text)?;
                wb.apply_style(
                    sheet,
                    Range::single(active),
                    StyleChange::NumberFormat(NumberFormat::Time),
                )
            })
        });
    }

    // Double-click on a header edge: as wide as the longest displayed text in the column.
    fn auto_fit(&mut self, col: ColIdx, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
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
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        if self.edit(window, cx, move |wb| wb.duplicate_sheet(sheet))
            && let Some(document) = self.documents.active_mut()
        {
            document.pending_sheet = Some(SheetId(sheet.0 + 1));
        }
    }

    fn add_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let new_sheet = SheetId(u32::try_from(document.sheets.len()).unwrap_or(0));
        if self.edit(window, cx, |wb| wb.add_sheet().map(|_| ()))
            && let Some(document) = self.documents.active_mut()
        {
            document.pending_sheet = Some(new_sheet);
        }
    }

    fn open_go_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.revert_theme_preview(cx);
        if self.forward_to_pdf(window, cx, |view, window, cx| {
            view.focus_page_input(window, cx)
        }) {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("goto.placeholder")));
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        let target = input.read(cx).value().to_string();
                        this.close_go_to(window, cx);
                        this.go_to_target(&target, window, cx);
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

    fn go_to_target(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let (current, sheets) = (document.sheet, document.sheets.clone());
        let names = if go_to::needs_names(target) {
            let read = document.workbook().map(|workbook| workbook.defined_names());
            let Some(names) = read else {
                self.notify(Severity::Warning, t!("notice.go_to_busy"), cx);
                return;
            };
            names
        } else {
            Vec::new()
        };
        match go_to::resolve(target, current, &sheets, &names) {
            Ok((sheet, range)) => {
                self.switch_sheet(sheet, window, cx);
                self.grid.update(cx, |grid, cx| grid.show_range(range, cx));
            }
            Err(error) => {
                let message = match error {
                    GoToError::NotAReference => t!("notice.not_a_reference", target = target),
                    GoToError::UnknownSheet(sheet) => t!("notice.unknown_sheet", sheet = sheet),
                    GoToError::UnknownName(name) => t!("notice.unknown_name", name = name),
                };
                self.notify(Severity::Warning, message, cx);
            }
        }
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
        self.revert_theme_preview(cx);
        let Some(document) = self.documents.active() else {
            return;
        };
        let current = document
            .sheets
            .get(document.sheet.0 as usize)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(t!("rename.placeholder"));
            state.set_value(current, window, cx);
            state
        });
        let subscription = cx.subscribe_in(&input, window, |this, input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let name = input.read(cx).value().trim().to_string();
                this.close_rename(window, cx);
                if !name.is_empty()
                    && let Some(sheet) = this.active_sheet()
                {
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
                .h(rem_px(36.0))
                .px_2()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(div().text_sm().child(t!("rename.label")))
                .child(div().w(rem_px(260.0)).child(Input::new(input)))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("rename.hint")),
                ),
        )
    }

    fn delete_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        if document.sheets.len() < 2 {
            self.notify(Severity::Warning, t!("notice.last_sheet"), cx);
            return;
        }
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let name = document
            .sheets
            .get(sheet.0 as usize)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let answer = self.ask(
            PromptLevel::Warning,
            &t!("sheet.delete_title", name = name),
            Some(t!("sheet.delete_detail")),
            &[t!("button.cancel"), t!("button.delete")],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Some(1)
                && let Err(error) = this.update_in(cx, |this, window, cx| {
                    // Excel shows the sheet that takes the deleted one's place.
                    let Some(document) = this.documents.active() else {
                        return;
                    };
                    let last_after =
                        u32::try_from(document.sheets.len().saturating_sub(2)).unwrap_or(0);
                    if this.edit(window, cx, move |wb| wb.delete_sheet(sheet))
                        && let Some(document) = this.documents.active_mut()
                    {
                        document.pending_sheet = Some(SheetId(sheet.0.min(last_after)));
                    }
                })
            {
                tracing::debug!(%error, "workspace closed during delete prompt");
            }
        })
        .detach();
    }

    fn move_sheet(&mut self, left: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        let Some(document) = self.documents.active() else {
            return;
        };
        let last = u32::try_from(document.sheets.len().saturating_sub(1)).unwrap_or(0);
        let target = if left {
            sheet.0.checked_sub(1)
        } else {
            (sheet.0 < last).then_some(sheet.0 + 1)
        };
        let Some(target) = target else {
            return;
        };
        if self.edit(window, cx, move |wb| wb.move_sheet(sheet, target))
            && let Some(document) = self.documents.active_mut()
        {
            document.pending_sheet = Some(SheetId(target));
        }
    }

    // Opening a large file takes seconds; the status bar text alone went unnoticed.
    // Recalculations and saves reach the pill through the job pill, after a delay, so
    // short ones never flash it.
    fn render_busy(
        &self,
        busy_label: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let long_job = self.job_pill.shown(Instant::now()).map(|job| match job {
            LongJob::Saving => t!("busy.saving"),
            LongJob::Calculating => t!("busy.calculating"),
            LongJob::Autosaving => t!("busy.autosaving"),
        });
        let busy = busy_label
            .filter(|busy| ![t!("busy.calculating"), t!("busy.saving")].contains(&busy.as_ref()))
            .or_else(|| long_job.map(SharedString::from))?;
        let theme = cx.theme();
        let indicator = if cx.reduce_motion() {
            div().child("…").into_any_element()
        } else {
            Spinner::new().color(theme.primary).into_any_element()
        };
        Some(
            div()
                .absolute()
                .top(rem_px(96.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_4()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .text_color(theme.popover_foreground)
                        .shadow_md()
                        .child(indicator)
                        .child(busy),
                ),
        )
    }

    fn render_status(
        &self,
        busy: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let access_chip = self.render_access_chip(cx);
        let theme = cx.theme();
        let mut left = h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .gap_3()
            .items_center();
        if let Some(busy) = busy {
            left = left.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.primary)
                    .child(busy),
            );
        } else if let Some(notice) = self.notices.status() {
            let (icon, color) = severity_icon(notice.severity, theme);
            let summary = notice.text.replace('\n', " ");
            let label = format!("{}: {summary}", severity_title(notice.severity));
            left = left.child(
                h_flex()
                    .id("status-notice")
                    .role(Role::Button)
                    .aria_label(label)
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .text_color(color)
                    .tooltip(move |window, cx| {
                        Tooltip::new(keymap::labeled(
                            cx,
                            t!("palette.show_messages"),
                            &ShowMessages,
                        ))
                        .build(window, cx)
                    })
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowMessages), cx))
                    .child(Icon::new(icon).size_3().flex_none())
                    .child(div().min_w_0().truncate().child(summary)),
            );
        } else {
            left = left.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(t!("status.ready")),
            );
        }
        let automatic =
            cx.global::<AgentConfig>().state.current.agents.permission == PermissionMode::Automatic;
        let mut right = h_flex()
            .flex_none()
            .whitespace_nowrap()
            .gap_4()
            .items_center()
            .text_color(theme.muted_foreground)
            .when(automatic, |this| {
                this.child(
                    div()
                        .text_color(theme.warning)
                        .child(t!("status.agents_automatic")),
                )
            })
            .children(access_chip);
        if let Some(text) = self.documents.shown_text() {
            let (line, column) = self
                .shown_pane()
                .map_or((1, 1), |entry| entry.pane.read(cx).caret(cx));
            let saved = if text.dirty {
                t!("markdown.status.unsaved")
            } else {
                t!("markdown.status.saved")
            };
            right = right
                .child(t!("markdown.status.name"))
                .child(t!("markdown.status.position", line = line, column = column))
                .child(t!("markdown.status.encoding"))
                .child(saved);
        }
        if let Some(view) = self
            .shown_pdf_view()
            .filter(|view| view.read(cx).is_ready())
        {
            let view = view.read(cx);
            right = right
                .child(t!("pdf.status.name"))
                .child(t!(
                    "pdf.status.page",
                    page = view.current_page().0 + 1,
                    total = view.page_count()
                ))
                .child(view.zoom_label())
                .child(t!("status.read_only"));
        }
        match self.stats.filter(|_| self.documents.active().is_some()) {
            Some(stats) if stats.count > 1 => {
                let chosen = &cx.global::<AgentConfig>().settings().general.status_summary;
                for item in chosen.items() {
                    if let Some(value) = stats.value(*item) {
                        right = right.child(format!(
                            "{}: {}",
                            summary_label(*item),
                            format_number(value)
                        ));
                    }
                }
            }
            None if self.documents.active().is_some() => {
                right = right.child(t!("status.selection_too_large"))
            }
            _ => {}
        }
        // Modes that change what the sheet shows or allows are named, never only implied.
        if self.show_formulas {
            right = right.child(t!("status.showing_formulas"));
        }
        if self
            .documents
            .active()
            .is_some_and(|document| document.read_only)
        {
            right = right.child(t!("status.read_only"));
        }
        if self.documents.shown_text().is_none() && self.documents.shown_pdf().is_none() {
            let zoom = self.grid.read(cx).zoom();
            right = right.child(format!("{:.0}%", zoom * 100.0));
        }
        if self.diagnostics {
            let recalc = self
                .last_recalc
                .map_or("-".to_string(), |d| format!("{} ms", d.as_millis()));
            let frame = self.grid.read(cx).last_paint();
            right = right
                .child(t!("status.memory", mb = self.memory_mb))
                .child(t!(
                    "status.grid_paint",
                    ms = format!("{:.1}", frame.as_secs_f64() * 1000.0)
                ))
                .child(t!("status.last_recalc", value = recalc));
        }
        h_flex()
            .h(rem_px(STATUS_BAR_HEIGHT))
            .px_3()
            .gap_4()
            .justify_between()
            .items_center()
            .text_xs()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .child(left)
            .child(right.id("status-summary").context_menu({
                let chosen = cx
                    .global::<AgentConfig>()
                    .settings()
                    .general
                    .status_summary
                    .clone();
                let grid_focus = self.grid.focus_handle(cx);
                move |menu, _, _| summary_menu(menu, &chosen, grid_focus.clone())
            }))
    }
}

fn file_job_refusal(job: FileJob) -> &'static str {
    match job {
        FileJob::Idle => t!("notice.still_calculating"),
        FileJob::Saving | FileJob::Autosaving => t!("notice.still_saving"),
        FileJob::Hibernating => t!("notice.still_hibernating"),
    }
}

fn changed_file_answers() -> [PromptButton; 3] {
    [
        PromptButton::cancel(t!("button.cancel")),
        PromptButton::new(t!("button.replace")),
        PromptButton::new(t!("button.save_as")),
    ]
}

fn empty_workbook() -> Workbook {
    Workbook::new_empty().unwrap_or_else(|error| {
        tracing::error!(%error, "could not create an empty workbook");
        std::process::exit(1)
    })
}

fn summary_label(item: SummaryItem) -> &'static str {
    match item {
        SummaryItem::Average => t!("status.summary.average"),
        SummaryItem::Count => t!("status.summary.count"),
        SummaryItem::NumericalCount => t!("status.summary.numerical_count"),
        SummaryItem::Min => t!("status.summary.min"),
        SummaryItem::Max => t!("status.summary.max"),
        SummaryItem::Sum => t!("status.summary.sum"),
    }
}

// Excel's status bar menu: the checked figures are the ones shown.
fn summary_menu(menu: PopupMenu, chosen: &StatusSummary, grid_focus: FocusHandle) -> PopupMenu {
    let entry = |menu: PopupMenu, item: SummaryItem, action: Box<dyn Action>| {
        menu.menu_with_check(summary_label(item), chosen.contains(item), action)
    };
    let menu = menu.action_context(grid_focus);
    let menu = entry(menu, SummaryItem::Average, Box::new(ToggleSummaryAverage));
    let menu = entry(menu, SummaryItem::Count, Box::new(ToggleSummaryCount));
    let menu = entry(
        menu,
        SummaryItem::NumericalCount,
        Box::new(ToggleSummaryNumericalCount),
    );
    let menu = entry(menu, SummaryItem::Min, Box::new(ToggleSummaryMin));
    let menu = entry(menu, SummaryItem::Max, Box::new(ToggleSummaryMax));
    entry(menu, SummaryItem::Sum, Box::new(ToggleSummarySum))
}

fn toggle_summary_item(item: SummaryItem, cx: &mut App) {
    agent_settings::change(cx, move |settings| {
        settings.general.status_summary = settings.general.status_summary.toggled(item);
    });
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        let text = format!("{n:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl Workspace {
    fn render_workbook(
        &self,
        busy: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(toolbar::render(&self.active_style, &self.colors, cx))
            .child(self.render_formula_bar(cx))
            .children(self.render_agent_review(cx))
            .children(self.render_held_settings(cx))
            .children(self.render_find(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("grid-area")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.grid.clone())
                            .children(self.render_review_popover(cx))
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
            .child(self.render_status(busy, cx))
    }

    fn render_empty(&self, busy: Option<SharedString>, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(self.render_held_settings(cx))
            .child(div().flex_1().min_h_0().child(start_view::render(
                &self.recent,
                &self.focus,
                cx,
            )))
            .child(self.render_status(busy, cx))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        interface_scale::apply(window, cx);
        self.rest_hidden_pdfs(window, cx);
        let busy_label = self.busy_label();
        let root = Self::leave_full_window_for_sheet_commands(v_flex(), cx);
        let theme = cx.theme();
        root.relative()
            .map(|root| pdfs::pdf_actions(root, cx))
            .key_context({
                let mut context = KeyContext::default();
                context.add("Workspace");
                if self.prompts.blocks_input() {
                    context.add(PROMPT_OPEN_CONTEXT);
                }
                context
            })
            // A file dropped on the window opens like File > Open.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first().cloned() {
                    this.open_path(path, window, cx);
                }
            }))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::new_workbook))
            .on_action(cx.listener(|this, _: &CycleMarkdownView, window, cx| {
                this.cycle_markdown_view(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReloadMarkdown, window, cx| {
                this.reload_shown_text(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &KeepMarkdownChanges, _, cx| {
                    this.keep_shown_text_changes(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.request_close(CloseGoal::Quit, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                let goal = main_window::close_goal(cx);
                this.request_close(goal, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextDocument, window, cx| {
                this.step_document(Step::Next, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousDocument, window, cx| {
                this.step_document(Step::Previous, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CloseDocument, window, cx| this.close_active(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ReopenClosedDocument, window, cx| {
                this.reopen_closed(window, cx)
            }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                this.edit(window, cx, |wb| wb.undo());
            }))
            .on_action(cx.listener(|this, _: &Redo, window, cx| {
                this.edit(window, cx, |wb| wb.redo());
            }))
            .on_action(cx.listener(|this, _: &Copy, window, cx| {
                if !this.forward_to_pdf(window, cx, |view, _, cx| view.copy_selection(cx)) {
                    this.copy(false, cx);
                }
            }))
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
            .on_action(cx.listener(|_, _: &ToggleSummaryAverage, _, cx| {
                toggle_summary_item(SummaryItem::Average, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleSummaryCount, _, cx| {
                toggle_summary_item(SummaryItem::Count, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleSummaryNumericalCount, _, cx| {
                toggle_summary_item(SummaryItem::NumericalCount, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleSummaryMin, _, cx| {
                toggle_summary_item(SummaryItem::Min, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleSummaryMax, _, cx| {
                toggle_summary_item(SummaryItem::Max, cx)
            }))
            .on_action(cx.listener(|_, _: &ToggleSummarySum, _, cx| {
                toggle_summary_item(SummaryItem::Sum, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleFormulas, _, cx| {
                this.show_formulas = !this.show_formulas;
                let message = if this.show_formulas {
                    t!("notice.showing_formulas")
                } else {
                    t!("notice.showing_values")
                };
                this.notify(Severity::Info, message, cx);
                this.fetch_cells(CellsRefresh::Replace, cx);
            }))
            .on_action(
                cx.listener(|this, _: &PasteValues, window, cx| this.paste_values(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FormatCells, window, cx| {
                    this.open_format_dialog(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &GenerateData, window, cx| this.open_generate(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ApplyNumberFormat, window, cx| {
                this.apply_format_dialog(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseFormatDialog, window, cx| {
                this.close_format_dialog(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ConfirmCloseDialog, window, cx| {
                this.confirm_close_dialog(window, cx)
            }))
            .on_action(cx.listener(|this, _: &DiscardCloseDialog, window, cx| {
                this.discard_close_dialog(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CancelCloseDialog, window, cx| {
                this.cancel_close_dialog(window, cx)
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
                if let Some(sheet) = this.active_sheet() {
                    let range = this.selection(cx);
                    this.edit(window, cx, move |wb| wb.clear_formats(sheet, range));
                }
            }))
            .on_action(cx.listener(|this, _: &ClearAll, window, cx| {
                if let Some(sheet) = this.active_sheet() {
                    let range = this.selection(cx);
                    this.edit(window, cx, move |wb| wb.clear_all(sheet, range));
                }
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
                interface_scale::larger(cx);
                this.announce_interface_size(cx);
            }))
            .on_action(cx.listener(|this, _: &InterfaceSmaller, _, cx| {
                interface_scale::smaller(cx);
                this.announce_interface_size(cx);
            }))
            .on_action(cx.listener(|this, _: &InterfaceReset, _, cx| {
                interface_scale::reset(cx);
                this.announce_interface_size(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleReduceMotion, _, cx| {
                let reduce = !cx.reduce_motion();
                cx.set_reduce_motion(reduce);
                let message = if reduce {
                    t!("notice.reduce_motion_on")
                } else {
                    t!("notice.reduce_motion_off")
                };
                this.notify(Severity::Info, message, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| {
                if this.forward_to_pdf(window, cx, |view, _, cx| view.step_zoom(true, cx))
                    || this.forward_to_text(cx, |pane, cx| pane.zoom_in(cx))
                {
                    return;
                }
                this.grid.update(cx, |g, cx| g.set_zoom(g.zoom() + 0.1, cx));
                this.refresh_cells(cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| {
                if this.forward_to_pdf(window, cx, |view, _, cx| view.step_zoom(false, cx))
                    || this.forward_to_text(cx, |pane, cx| pane.zoom_out(cx))
                {
                    return;
                }
                this.grid.update(cx, |g, cx| g.set_zoom(g.zoom() - 0.1, cx));
                this.refresh_cells(cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomReset, window, cx| {
                if this.forward_to_pdf(window, cx, |view, _, cx| view.actual_size(cx))
                    || this.forward_to_text(cx, |pane, cx| pane.zoom_reset(cx))
                {
                    return;
                }
                this.grid.update(cx, |g, cx| g.set_zoom(1.0, cx));
                this.refresh_cells(cx);
            }))
            .on_action(cx.listener(|this, _: &ResetDocumentZoom, window, cx| {
                if !this.forward_to_pdf(window, cx, |view, _, cx| view.actual_size(cx)) {
                    this.forward_to_text(cx, |pane, cx| pane.zoom_reset(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &NextSheet, window, cx| {
                if let Some(sheet) = this.active_sheet() {
                    this.switch_sheet(SheetId(sheet.0 + 1), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousSheet, window, cx| {
                if let Some(sheet) = this.active_sheet() {
                    this.switch_sheet(SheetId(sheet.0.saturating_sub(1)), window, cx);
                }
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
            .on_action(cx.listener(|this, _: &InsertSelection, window, cx| {
                this.insert_or_delete_selection(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteSelection, window, cx| {
                this.insert_or_delete_selection(false, window, cx)
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
            .on_action(
                cx.listener(|this, _: &SearchFiles, window, cx| this.toggle_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseSearch, window, cx| this.close_search(window, cx)),
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
            .on_action(cx.listener(|this, _: &CloseChart, window, cx| this.close_chart(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleDiagnostics, _, cx| {
                this.diagnostics = !this.diagnostics;
                cx.notify();
            }))
            .on_action(|_: &ToggleTheme, _, cx| theme::cycle(cx))
            .on_action(
                cx.listener(|this, _: &SelectTheme, window, cx| this.open_theme_picker(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CancelThemePicker, window, cx| {
                this.cancel_theme_picker(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleAgentChat, window, cx| this.toggle_chat(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusAgentChat, window, cx| {
                this.open_chat(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CycleChatLayout, window, cx| {
                this.change_chat_layout(ChatLayoutState::cycled, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleChatFullWindow, window, cx| {
                this.change_chat_layout(ChatLayoutState::toggled_full_window, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DockChat, window, cx| {
                this.change_chat_layout(|state| state.set(ChatLayout::Docked), window, cx)
            }))
            .on_action(cx.listener(|this, _: &SplitChatBesideSheet, window, cx| {
                this.change_chat_layout(|state| state.set(ChatLayout::SideBySide), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FillWindowWithChat, window, cx| {
                this.change_chat_layout(|state| state.set(ChatLayout::FullWindow), window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddSelectionToChat, window, cx| {
                this.add_selection_to_chat(window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewAgentConversation, window, cx| {
                this.new_agent_conversation(window, cx)
            }))
            .on_action(cx.listener(|this, _: &AllowChatPermission, _, cx| {
                this.answer_chat_permission(true, cx)
            }))
            .on_action(cx.listener(|this, _: &DenyChatPermission, _, cx| {
                this.answer_chat_permission(false, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusSidebar, window, cx| this.focus_sidebar(window, cx)),
            )
            .on_action(cx.listener(|this, _: &WidenSidebar, window, cx| {
                this.step_panel(Panel::Sidebar, Resize::Wider, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NarrowSidebar, window, cx| {
                this.step_panel(Panel::Sidebar, Resize::Narrower, window, cx)
            }))
            .on_action(cx.listener(|this, _: &WidenChat, window, cx| {
                this.step_panel(Panel::Chat, Resize::Wider, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NarrowChat, window, cx| {
                this.step_panel(Panel::Chat, Resize::Narrower, window, cx)
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.drag_panel(event, window, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_panel_drag(cx)),
            )
            .when(self.panels_dragging(), |root| root.cursor_col_resize())
            .on_action(cx.listener(|this, _: &NewSpace, window, cx| this.new_space(window, cx)))
            .on_action(
                cx.listener(|this, _: &RenameSpace, window, cx| this.rename_space(window, cx)),
            )
            .on_action(cx.listener(|this, _: &DeleteSpace, _, cx| this.delete_space(cx)))
            .on_action(cx.listener(|this, _: &CycleSpaceColor, _, cx| this.cycle_space_color(cx)))
            .on_action(
                cx.listener(|this, _: &CustomizeSpace, window, cx| {
                    this.customize_space(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ResetSpaceAppearance, _, cx| {
                    this.reset_space_appearance(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CloseSpacePanel, window, cx| {
                this.close_space_panel(window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteFile, window, cx| this.delete_file(window, cx)))
            .on_action(cx.listener(|this, _: &CloseSpaceRename, window, cx| {
                this.close_space_rename(window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveToPreviousSpace, _, cx| {
                this.shift_document(Neighbour::Previous, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveToNextSpace, _, cx| {
                this.shift_document(Neighbour::Next, cx)
            }))
            .on_action(cx.listener(|this, _: &ApplyHeldSettings, window, cx| {
                this.decide_held_settings(HeldDecision::Apply, window, cx)
            }))
            .on_action(cx.listener(|this, _: &KeepCurrentSettings, window, cx| {
                this.decide_held_settings(HeldDecision::Keep, window, cx)
            }))
            .on_action(cx.listener(|this, _: &LetAgentsEdit, _, cx| this.toggle_agent_editing(cx)))
            .on_action(
                cx.listener(|this, _: &DismissNotice, window, cx| this.dismiss_notice(window, cx)),
            )
            .on_action(cx.listener(|this, _: &zenkai_grid::Cancel, window, cx| {
                this.dismiss_notice(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyNotice, _, cx| this.copy_notice(cx)))
            .on_action(
                cx.listener(|this, _: &ShowMessages, window, cx| this.toggle_messages(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseMessages, window, cx| this.close_messages(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextMessage, _, cx| this.step_message(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousMessage, _, cx| this.step_message(false, cx)))
            .on_action(
                cx.listener(|this, _: &FocusNotice, window, cx| this.focus_notice(window, cx)),
            )
            .on_action(cx.listener(|this, _: &AllowAgentChange, window, cx| {
                this.decide_agent_change(Decision::Allow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextAgentChange, window, cx| {
                this.step_review(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousAgentChange, window, cx| {
                this.step_review(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &KeepAgentChange, window, cx| {
                this.keep_review_cell(window, cx)
            }))
            .on_action(cx.listener(|this, _: &RejectAgentChange, window, cx| {
                this.reject_review_cell(window, cx)
            }))
            .on_action(cx.listener(|this, _: &KeepAllAgentChanges, window, cx| {
                this.keep_all_review(window, cx)
            }))
            .on_action(cx.listener(|this, _: &RejectAllAgentChanges, window, cx| {
                this.reject_all_review(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowAgentChange, window, cx| {
                this.show_agent_change(window, cx)
            }))
            .on_action(cx.listener(|this, _: &DenyAgentChange, window, cx| {
                this.decide_agent_change(Decision::Deny, window, cx)
            }))
            .child(self.render_title_bar(cx))
            .children(self.render_agent_approval(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .children(
                        self.render_sidebar(window, cx)
                            .filter(|_| !self.chat_fills_window()),
                    )
                    .children((!self.chat_fills_window()).then(|| {
                        if self.documents.active().is_some() {
                            self.render_workbook(busy_label.clone(), cx)
                                .into_any_element()
                        } else if let Some(entry) = self.shown_pane() {
                            self.render_text(entry.pane.clone(), busy_label.clone(), cx)
                                .into_any_element()
                        } else if let Some(view) = self.shown_pdf_view() {
                            self.render_pdf(view, busy_label.clone(), cx)
                                .into_any_element()
                        } else {
                            self.render_empty(busy_label.clone(), cx).into_any_element()
                        }
                    }))
                    .children(self.render_chat(window, cx)),
            )
            .children(self.render_palette())
            .children(self.render_theme_picker(cx))
            .children(self.render_search(cx))
            .children(self.render_busy(busy_label, cx))
            .children(self.render_toast(cx))
            .children(self.render_notice_card(window, cx))
            .children(self.render_messages(cx))
            .children(self.render_csv_preview(cx))
            .children(self.render_format_dialog(cx))
            .children(self.render_close_dialog(cx))
            .children(self.render_generate())
            .children(self.render_space_panel(window, cx))
    }
}

// The scrim covers the window: a click outside the panel closes it, and Esc does too.
fn command_overlay(
    command: Command,
    context: &'static str,
    close: impl Fn() -> Box<dyn Action> + 'static,
) -> impl IntoElement {
    div()
        .key_context(context)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.dispatch_action(close(), cx)
        })
        .child(
            div()
                .absolute()
                .top(rem_px(72.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(rem_px(560.0))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(command.bordered(true).max_h(rem_px(420.0))),
                ),
        )
}

fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

// Excel's cell context menu, trimmed to what Zenkai supports. Actions go to the grid so
// they act on the selection the right click just set.
fn cell_menu(menu: PopupMenu, grid_focus: FocusHandle) -> PopupMenu {
    menu.action_context(grid_focus)
        .menu(t!("menu.cut"), Box::new(Cut))
        .menu(t!("menu.copy"), Box::new(Copy))
        .menu(t!("menu.paste"), Box::new(Paste))
        .menu(t!("menu.paste_values"), Box::new(PasteValues))
        .separator()
        .menu(t!("menu.insert_rows"), Box::new(InsertRows))
        .menu(t!("menu.insert_columns"), Box::new(InsertColumns))
        .menu(t!("menu.delete_rows"), Box::new(DeleteRows))
        .menu(t!("menu.delete_columns"), Box::new(DeleteColumns))
        .separator()
        .menu(t!("menu.clear_contents"), Box::new(DeleteForward))
        .menu(t!("menu.clear_formats"), Box::new(ClearFormats))
        .menu(t!("menu.clear_all"), Box::new(ClearAll))
        .separator()
        .menu(t!("menu.sort_ascending"), Box::new(SortAscending))
        .menu(t!("menu.sort_descending"), Box::new(SortDescending))
        .separator()
        .menu(t!("menu.generate_data"), Box::new(GenerateData))
        .separator()
        .menu(t!("menu.insert_chart"), Box::new(InsertChart))
}

// Excel's sheet tab menu; it acts on the active sheet.
fn sheet_menu(menu: PopupMenu, grid_focus: FocusHandle) -> PopupMenu {
    menu.action_context(grid_focus)
        .menu(t!("menu.insert_sheet"), Box::new(NewSheet))
        .menu(t!("menu.rename"), Box::new(RenameSheet))
        .menu(t!("menu.duplicate"), Box::new(DuplicateSheet))
        .menu(t!("menu.delete"), Box::new(DeleteSheet))
        .separator()
        .menu(t!("menu.move_left"), Box::new(MoveSheetLeft))
        .menu(t!("menu.move_right"), Box::new(MoveSheetRight))
}

fn rgb_of(color: Hsla) -> Rgb {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    Rgb(channel(rgba.r) << 16 | channel(rgba.g) << 8 | channel(rgba.b))
}

// Typed text with the wrap and widening it brings, as one undo step like Excel.
fn enter_text(
    wb: &mut Workbook,
    sheet: SheetId,
    pos: CellPos,
    text: &str,
) -> Result<(), EngineError> {
    wb.as_one_undo_step(|wb| {
        // Excel turns on wrap text for a cell typed with a line break.
        let wraps = text.contains('\n') && !text.starts_with('=');
        wb.set_input(sheet, pos, text)?;
        if wraps {
            wb.apply_style(sheet, Range::single(pos), StyleChange::Wrap(true))?;
            grow_wrapped_rows(wb, sheet, Range::single(pos))?;
        }
        widen_for_numbers(wb, sheet, Range::single(pos))
    })
}

// Excel widens a column that still has the default width when a formatted number or date
// typed into it would only show as ####. General numbers are shortened when painted
// instead, and text keeps overflowing.
// Each default-width column of `range` grows to its widest formatted number, once.
fn widen_for_numbers(wb: &mut Workbook, sheet: SheetId, range: Range) -> Result<(), EngineError> {
    let sizes = wb.sizes(sheet);
    let mut needed: std::collections::BTreeMap<ColIdx, f32> = Default::default();
    // Small ranges are read cell by cell, and so is the used part of a large one when it
    // is small; otherwise only where the sheet has content.
    let cells: Vec<CellPos> = if range.cell_count() <= MAX_AUTOFIT_CELLS as u64 {
        range.positions().collect()
    } else {
        match range.clip_to(wb.used_end(sheet)) {
            None => Vec::new(),
            Some(used) if used.cell_count() <= MAX_AUTOFIT_CELLS as u64 => used
                .positions()
                .filter(|pos| !wb.input(sheet, *pos).is_empty())
                .collect(),
            Some(_) => wb
                .filled_cells(sheet)
                .into_iter()
                .filter(|pos| range.contains(*pos))
                .take(MAX_AUTOFIT_CELLS)
                .collect(),
        }
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
// A range past MAX_AUTOFIT_CELLS keeps its row heights, as every cell of it would be read.
fn grow_wrapped_rows(wb: &mut Workbook, sheet: SheetId, range: Range) -> Result<(), EngineError> {
    if range.cell_count() > MAX_AUTOFIT_CELLS as u64 {
        return Ok(());
    }
    let sizes = wb.sizes(sheet);
    let custom_rows: std::collections::HashSet<zenkai_types::RowIdx> =
        sizes.rows.iter().map(|(row, _)| *row).collect();
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
    for pos in range.positions() {
        if wb.input(sheet, pos).is_empty() || custom_rows.contains(&pos.row) {
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
    use super::{
        CHANGED_FILE_REPLACE, CHANGED_FILE_SAVE_AS, DelimitedAction, StructureEdit, TargetAction,
        changed_file_answers, delimited_action, enter_text, grow_wrapped_rows, selection_edit,
        target_action, widen_for_numbers,
    };
    use crate::files::TargetProbe;
    use zenkai_engine::{Engine, Workbook};
    use zenkai_i18n::t;
    use zenkai_types::{CellPos, ColIdx, DEFAULT_COL_WIDTH, Range, RowIdx, SheetId};

    #[test]
    fn save_as_target_follows_the_probe_in_the_order_excel_asks() {
        let same = TargetProbe {
            same_as_source: true,
            exists: false,
        };
        let taken = TargetProbe {
            same_as_source: false,
            exists: true,
        };
        let free = TargetProbe {
            same_as_source: false,
            exists: false,
        };
        assert_eq!(
            target_action(&same, false, true, true),
            TargetAction::RefuseOriginal
        );
        assert_eq!(
            target_action(&same, false, false, true),
            TargetAction::ConfirmReplacingOriginal
        );
        assert_eq!(
            target_action(&same, true, false, false),
            TargetAction::SaveOverOriginal
        );
        assert_eq!(
            target_action(&taken, true, false, false),
            TargetAction::ConfirmReplacingExisting
        );
        assert_eq!(
            target_action(&taken, false, true, true),
            TargetAction::SaveNew
        );
        assert_eq!(
            target_action(&free, true, false, false),
            TargetAction::SaveNew
        );
    }

    #[test]
    fn delimited_export_only_asks_again_for_the_open_source_file() {
        let probe = |same_as_source, exists| TargetProbe {
            same_as_source,
            exists,
        };
        assert_eq!(
            delimited_action(&probe(false, true)),
            DelimitedAction::Export
        );
        assert_eq!(
            delimited_action(&probe(true, false)),
            DelimitedAction::ConfirmReplacing
        );
        assert_eq!(
            delimited_action(&probe(false, false)),
            DelimitedAction::Export
        );
    }

    #[test]
    fn insert_and_delete_follow_the_shape_of_the_selection() {
        let at = |row, col| CellPos::new(RowIdx::clamped(row), ColIdx::clamped(col));
        let max_row = i64::from(zenkai_types::MAX_ROWS) - 1;
        let max_col = i64::from(zenkai_types::MAX_COLS) - 1;
        let column = Range::new(at(0, 2), at(max_row, 3));
        let row = Range::new(at(4, 0), at(5, max_col));
        let cells = Range::new(at(1, 1), at(2, 2));
        assert!(matches!(
            selection_edit(false, column),
            Some(StructureEdit::DeleteColumns)
        ));
        assert!(matches!(
            selection_edit(true, column),
            Some(StructureEdit::InsertColumns)
        ));
        assert!(matches!(
            selection_edit(false, row),
            Some(StructureEdit::DeleteRows)
        ));
        assert!(matches!(
            selection_edit(true, row),
            Some(StructureEdit::InsertRows)
        ));
        let sheet = Range::new(at(0, 0), at(max_row, max_col));
        for insert in [true, false] {
            assert!(selection_edit(insert, cells).is_none());
            assert!(selection_edit(insert, sheet).is_none());
        }
    }

    #[test]
    fn changed_file_prompt_defaults_to_cancel() {
        let answers = changed_file_answers();
        assert!(answers[0].is_cancel());
        assert_eq!(
            answers.iter().filter(|answer| answer.is_cancel()).count(),
            1
        );
        assert_eq!(
            answers[CHANGED_FILE_REPLACE].label().as_ref(),
            t!("button.replace")
        );
        assert_eq!(
            answers[CHANGED_FILE_SAVE_AS].label().as_ref(),
            t!("button.save_as")
        );
    }

    const SHEET: SheetId = SheetId(0);

    fn a1() -> CellPos {
        CellPos::default()
    }

    fn row_height(wb: &Workbook, row: RowIdx) -> Option<f32> {
        wb.sizes(SHEET)
            .rows
            .iter()
            .find(|(candidate, _)| *candidate == row)
            .map(|(_, height)| *height)
    }

    fn column_is_widened(wb: &Workbook) -> bool {
        wb.sizes(SHEET)
            .columns
            .iter()
            .any(|span| span.first == a1().col && span.width > DEFAULT_COL_WIDTH)
    }

    fn priced_number(wb: &mut Workbook, format: &str) {
        wb.set_input(SHEET, a1(), "1234.5").unwrap();
        wb.set_number_format(SHEET, Range::single(a1()), format)
            .unwrap();
    }

    #[test]
    fn one_undo_takes_back_a_typed_date_and_the_widening_it_caused() {
        for date in ["1/2/2024", "2024-01-02"] {
            let mut wb = Workbook::new_empty().unwrap();
            let before = wb.cell(SHEET, a1());
            enter_text(&mut wb, SHEET, a1(), date).unwrap();
            assert!(column_is_widened(&wb), "{date}");
            let entered = wb.cell(SHEET, a1());
            let widened = wb.sizes(SHEET);

            let mut control = Workbook::new_empty().unwrap();
            control
                .set_column_width(SHEET, a1().col, widened.columns[0].width)
                .unwrap();
            control.undo().unwrap();

            wb.undo().unwrap();
            assert_eq!(wb.cell(SHEET, a1()), before, "{date}");
            assert_eq!(wb.sizes(SHEET), control.sizes(SHEET), "{date}");

            wb.redo().unwrap();
            assert_eq!(wb.cell(SHEET, a1()), entered, "{date}");
            assert_eq!(wb.sizes(SHEET), widened, "{date}");

            wb.undo().unwrap();
            enter_text(&mut wb, SHEET, a1(), "7").unwrap();
            assert_eq!(wb.cell(SHEET, a1()).text, "7", "{date}");
        }
    }

    #[test]
    fn a_typed_formula_is_computed_before_its_column_is_widened() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_number_format(SHEET, Range::single(a1()), "$#,##0.00")
            .unwrap();
        enter_text(&mut wb, SHEET, a1(), "=1234.5*1").unwrap();
        assert!(column_is_widened(&wb));
    }

    #[test]
    fn formatting_a_number_that_no_longer_fits_widens_its_column() {
        let mut wb = Workbook::new_empty().unwrap();
        priced_number(&mut wb, "$#,##0.00");
        widen_for_numbers(&mut wb, SHEET, Range::single(a1())).unwrap();
        assert!(column_is_widened(&wb));
    }

    #[test]
    fn a_number_in_general_format_does_not_widen_its_column() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), "123456789012345").unwrap();
        widen_for_numbers(&mut wb, SHEET, Range::single(a1())).unwrap();
        assert!(!column_is_widened(&wb));
    }

    #[test]
    fn a_formatted_number_that_fits_leaves_the_column_alone() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), "5").unwrap();
        wb.set_number_format(SHEET, Range::single(a1()), "0.0")
            .unwrap();
        widen_for_numbers(&mut wb, SHEET, Range::single(a1())).unwrap();
        assert!(!column_is_widened(&wb));
    }

    #[test]
    fn a_column_with_a_custom_width_is_never_widened() {
        let mut wb = Workbook::new_empty().unwrap();
        priced_number(&mut wb, "$#,##0.00");
        wb.set_column_width(SHEET, a1().col, 40.0).unwrap();
        widen_for_numbers(&mut wb, SHEET, Range::single(a1())).unwrap();
        let widths: Vec<f32> = wb
            .sizes(SHEET)
            .columns
            .iter()
            .map(|span| span.width)
            .collect();
        assert_eq!(widths.len(), 1);
        assert!(widths[0] < DEFAULT_COL_WIDTH);
    }

    #[test]
    fn a_whole_column_widens_for_the_numbers_in_its_used_part() {
        let mut wb = Workbook::new_empty().unwrap();
        priced_number(&mut wb, "$#,##0.00");
        widen_for_numbers(&mut wb, SHEET, Range::parse_a1("A1:A1048576").unwrap()).unwrap();
        assert!(column_is_widened(&wb));
    }

    #[test]
    fn cells_outside_the_range_do_not_widen_their_column() {
        let mut wb = Workbook::new_empty().unwrap();
        priced_number(&mut wb, "$#,##0.00");
        let elsewhere = Range::parse_a1("C3:D4").unwrap();
        widen_for_numbers(&mut wb, SHEET, elsewhere).unwrap();
        assert!(!column_is_widened(&wb));
    }

    #[test]
    fn long_wrapped_text_grows_a_default_height_row() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"word ".repeat(40)).unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::single(a1())).unwrap();
        let height = row_height(&wb, a1().row).unwrap();
        assert!(height > 2.0 * 19.0, "{height}");
    }

    #[test]
    fn text_wrapping_to_two_lines_sets_a_two_line_row_height() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"x".repeat(10)).unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::single(a1())).unwrap();
        let height = row_height(&wb, a1().row).unwrap();
        assert!((height - (2.0 * 19.0 + 4.0)).abs() < 1.0, "{height}");
    }

    #[test]
    fn text_that_fits_on_one_line_leaves_the_row_alone() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), "short").unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::single(a1())).unwrap();
        assert_eq!(row_height(&wb, a1().row), None);
    }

    #[test]
    fn numbers_never_grow_a_row() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), "123456789012345678").unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::single(a1())).unwrap();
        assert_eq!(row_height(&wb, a1().row), None);
    }

    #[test]
    fn a_row_with_a_custom_height_keeps_it() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"word ".repeat(40)).unwrap();
        wb.set_row_height(SHEET, a1().row, 30.0).unwrap();
        let before = row_height(&wb, a1().row);
        grow_wrapped_rows(&mut wb, SHEET, Range::single(a1())).unwrap();
        assert_eq!(row_height(&wb, a1().row), before);
    }

    #[test]
    fn wrapped_text_outside_the_range_does_not_grow_its_row() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"word ".repeat(40)).unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::parse_a1("C3:D4").unwrap()).unwrap();
        assert_eq!(row_height(&wb, a1().row), None);
    }

    #[test]
    fn a_range_past_the_autofit_cap_keeps_its_row_heights() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"word ".repeat(40)).unwrap();
        let whole_sheet = Range::parse_a1("A1:XFD1048576").unwrap();
        grow_wrapped_rows(&mut wb, SHEET, whole_sheet).unwrap();
        assert_eq!(row_height(&wb, a1().row), None);
    }

    #[test]
    fn only_the_rows_of_the_range_grow() {
        let mut wb = Workbook::new_empty().unwrap();
        let a3 = CellPos::parse_a1("A3").unwrap();
        wb.set_input(SHEET, a1(), &"word ".repeat(40)).unwrap();
        wb.set_input(SHEET, a3, &"word ".repeat(40)).unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::parse_a1("A1:XFD1").unwrap()).unwrap();
        assert!(row_height(&wb, a1().row).is_some());
        assert_eq!(row_height(&wb, a3.row), None);
    }

    #[test]
    fn the_tallest_wrapped_cell_sets_the_row_height() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"word ".repeat(10)).unwrap();
        let b1 = CellPos::parse_a1("B1").unwrap();
        wb.set_input(SHEET, b1, &"word ".repeat(20)).unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::parse_a1("A1:B1").unwrap()).unwrap();
        let height = row_height(&wb, a1().row).unwrap();
        assert!((height - (19.0 * 19.0 + 4.0)).abs() < 1.0, "{height}");
    }

    #[test]
    fn a_very_long_text_stops_growing_at_twenty_lines() {
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(SHEET, a1(), &"x".repeat(20_000)).unwrap();
        grow_wrapped_rows(&mut wb, SHEET, Range::single(a1())).unwrap();
        let height = row_height(&wb, a1().row).unwrap();
        assert!((height - (20.0 * 19.0 + 4.0)).abs() < 1.0, "{height}");
    }
}
