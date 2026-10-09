use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::*;
use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Unsupported, Workbook, open_xlsx};
use zenkai_types::WorkbookId;

use super::{Severity, Workspace, empty_workbook, file_label};
use crate::document::Document;
use crate::documents::{Installed, Intent, Loaded, Step};
use crate::entry::{Entry, Link, LinkStatus};
use crate::files::{self, FileLoad, LoadFailure};
use crate::recovery;

const PROTECTED_VIEW: &str = "This file came from the internet: agents may only read it (Protected View). Ctrl+Shift+E lets them edit it.";

pub(super) enum LinkLoad {
    Loaded {
        file: Box<FileLoad>,
        // The recovery copy the work was read from, to adopt once the workbook is installed.
        copy: Option<PathBuf>,
        recovery_lost: bool,
    },
    Missing,
    Failed(String),
}

// Runs on the background executor. The recovery copy, when there is one, holds the unsaved
// work and is read instead of the file.
fn read_link(link: &Link) -> LinkLoad {
    let mut recovery_lost = link.recovery_lost;
    if let Some(copy) = link.recovery.as_ref().filter(|copy| copy.exists()) {
        match open_xlsx(copy) {
            Ok(opened) => {
                return LinkLoad::Loaded {
                    file: Box::new(FileLoad {
                        workbook: opened.workbook,
                        unsupported: Vec::new(),
                        read_only: false,
                        // A recovery copy does not record where the work came from; it fails closed.
                        origin: FileOrigin::Internet,
                    }),
                    copy: Some(copy.clone()),
                    recovery_lost: false,
                };
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the recovery copy");
                recovery_lost = true;
            }
        }
    } else if link.recovery.is_some() {
        recovery_lost = true;
    }
    let Some(path) = &link.path else {
        return LinkLoad::Missing;
    };
    match files::load_workbook(path) {
        Ok(file) => LinkLoad::Loaded {
            file: Box::new(file),
            copy: None,
            recovery_lost,
        },
        Err(LoadFailure::Missing) => LinkLoad::Missing,
        Err(failure) => LinkLoad::Failed(failure_text(&failure)),
    }
}

fn failure_text(failure: &LoadFailure) -> String {
    match failure {
        LoadFailure::Missing => "The file was not found.".to_string(),
        LoadFailure::Engine(error) => error.to_string(),
        LoadFailure::Unreadable { reason, fallback } => format!(
            "The file could not be opened: {reason}. Reading its values also failed: {fallback}"
        ),
    }
}

impl Workspace {
    pub(super) fn open_document(
        &mut self,
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WorkbookId {
        self.park_active(window, cx);
        let id = self.documents.open(workbook, path, unsupported);
        self.refuse_orphaned_agent_change(window, cx);
        self.present_active(window, cx);
        id
    }

    pub(super) fn create_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Workbook::new_empty() {
            Ok(workbook) => {
                self.park_active(window, cx);
                self.documents.create(workbook);
                self.present_active(window, cx);
            }
            Err(error) => self.notify(Severity::Error, error.to_string(), cx),
        }
    }

    pub(super) fn switch_to(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.documents.entry(id) {
            None => {}
            Some(Entry::Link(_)) => {
                self.documents.want(id, Intent::Switch);
                self.begin_load(id, window, cx);
            }
            Some(Entry::Loaded(_)) if id == self.documents.active_id() => {
                self.documents.activate(id);
            }
            Some(Entry::Loaded(_)) => {
                self.park_active(window, cx);
                self.documents.activate(id);
                self.present_active(window, cx);
            }
        }
    }

    pub(super) fn step_document(
        &mut self,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.len() < 2 {
            return;
        }
        let id = self.documents.step(step);
        self.switch_to(id, window, cx);
        let name = self
            .documents
            .entry(id)
            .map(Entry::name)
            .unwrap_or_default();
        let position = self.documents.position_of(id);
        let count = self.documents.len();
        self.notify(
            Severity::Info,
            format!("{name} ({position} of {count})"),
            cx,
        );
    }

    // Everything tied to the workbook on screen is put away or dropped: a pending copy,
    // the Format Cells dialog and open bars would otherwise act on the next one.
    pub(super) fn park_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_formula_bar(true, window, cx);
        if let Some((pos, text)) = self.grid.update(cx, |grid, cx| grid.take_edit(cx)) {
            self.commit_text(self.documents.active().sheet, pos, text, window, cx);
        }
        let view = self.grid.read(cx).view_state();
        self.documents.active_mut().view = view;
        self.clipboard_source = None;
        self.grid.update(cx, |grid, cx| grid.set_marquee(None, cx));
        self.format_dialog = None;
        self.chart = None;
        self.rename = None;
        self.go_to = None;
    }

    pub(super) fn present_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.documents.active().view;
        self.forget_find_results();
        let sheet = self.sheet_view();
        self.grid.update(cx, |grid, cx| {
            grid.reset(sheet, cx);
            grid.restore_view(view, cx);
        });
        self.refresh_cells(cx);
        window.set_window_title(&self.documents.active().title());
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
    }

    pub(super) fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_document(self.documents.active_id(), window, cx);
    }

    pub(super) fn close_document(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing in the cell or the formula bar counts as an edit of the document being closed.
        if id == self.documents.active_id() {
            self.park_active(window, cx);
        }
        let Some(entry) = self.documents.entry(id) else {
            return;
        };
        if !entry.dirty() {
            self.finish_close(id, window, cx);
            return;
        }
        let edits = entry.loaded().map(Document::edit_count);
        let detail = format!("{} has changes that are not saved.", entry.name());
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved changes?",
            Some(&detail),
            &["Cancel", "Discard"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(1) {
                return;
            }
            let update = this.update_in(cx, |this, window, cx| {
                match this.documents.entry(id) {
                    None => {}
                    // Edited while the question was up: the answer covered older work.
                    Some(entry) if entry.loaded().map(Document::edit_count) != edits => {
                        this.close_document(id, window, cx)
                    }
                    Some(_) => this.finish_close(id, window, cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during discard prompt");
            }
        })
        .detach();
    }

    fn finish_close(&mut self, id: WorkbookId, window: &mut Window, cx: &mut Context<Self>) {
        let was_active = id == self.documents.active_id();
        if was_active {
            self.park_active(window, cx);
        }
        let recovery_copy = match self.documents.entry(id) {
            Some(Entry::Link(link)) => link.recovery.clone(),
            Some(Entry::Loaded(document)) => {
                self.previews.forget(document.generation());
                self.recovery_dir
                    .as_deref()
                    .map(|directory| recovery::document_file(directory, id))
            }
            None => None,
        };
        if let Some(file) = recovery_copy {
            cx.background_executor()
                .spawn(async move { recovery::remove_with_lock(&file) })
                .detach();
        }
        self.documents.close(id, empty_workbook);
        self.refuse_orphaned_agent_change(window, cx);
        if was_active {
            self.present_active(window, cx);
        }
        cx.notify();
    }

    pub(super) fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.documents.take_reopenable() {
            Some(path) => self.open_path(path, window, cx),
            None => self.notify(Severity::Info, "No closed workbook to reopen.", cx),
        }
    }

    pub(super) fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if files::is_delimited_text(&path) {
            self.import_csv(path, window, cx);
            return;
        }
        if let Some(open) = self.documents.find_by_path(&path) {
            self.switch_to(open, window, cx);
            return;
        }
        self.busy = Some(format!("Opening {}…", file_label(&path)).into());
        cx.notify();
        let started = Instant::now();
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { files::load_workbook(&task_path) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(file) => {
                        this.remember_recent(&path, cx);
                        if let Some(open) = this.documents.find_by_path(&path) {
                            this.switch_to(open, window, cx);
                            return;
                        }
                        let FileLoad {
                            workbook,
                            unsupported,
                            read_only,
                            origin,
                        } = file;
                        let id = this.open_document(workbook, Some(path), unsupported, window, cx);
                        if let Some(document) = this.documents.get_mut(id) {
                            document.read_only = read_only;
                            document.origin = origin;
                        }
                        this.announce_opened(Some(started), cx);
                    }
                    Err(LoadFailure::Missing) => this.notify(
                        Severity::Error,
                        format!("{} was not found.", path.display()),
                        cx,
                    ),
                    Err(failure) => this.notify(Severity::Error, failure_text(&failure), cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during open");
            }
        })
        .detach();
    }

    fn announce_opened(&mut self, started: Option<Instant>, cx: &mut Context<Self>) {
        let document = self.documents.active();
        if document.read_only {
            self.notify(
                Severity::Warning,
                "Opened read-only: values only, without formulas or formatting. Save As keeps a copy.",
                cx,
            );
        } else if !document.unsupported.is_empty() {
            let text = format!(
                "This file has content Zenkai does not keep yet ({}). Saving will ask for a new name.",
                document.unsupported_labels()
            );
            self.notify(Severity::Warning, text, cx);
        } else if document.origin == FileOrigin::Internet && started.is_some() {
            self.notify(Severity::Warning, PROTECTED_VIEW, cx);
        } else if let Some(started) = started {
            let text = format!("Opened in {} ms", started.elapsed().as_millis());
            self.notify(Severity::Info, text, cx);
        }
    }

    pub(super) fn begin_load(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(link) = self.documents.start_loading(id) else {
            return;
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let load = cx
                .background_executor()
                .spawn(async move { read_link(&link) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.finish_load(id, load, window, cx)
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during load");
            }
        })
        .detach();
    }

    fn finish_load(
        &mut self,
        id: WorkbookId,
        load: LinkLoad,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.documents.entry(id).map(Entry::name) else {
            return;
        };
        match load {
            LinkLoad::Loaded {
                file,
                copy,
                recovery_lost,
            } => {
                if self.documents.shows_when_loaded(id) {
                    self.park_active(window, cx);
                }
                let from_recovery = copy.is_some();
                let outcome = self.documents.install(Loaded {
                    id,
                    workbook: file.workbook,
                    unsupported: file.unsupported,
                    read_only: file.read_only,
                    origin: file.origin,
                    from_recovery,
                });
                self.refuse_orphaned_agent_change(window, cx);
                if let Some(copy) = copy {
                    self.adopt_recovery(id, copy, cx);
                }
                let path = self
                    .documents
                    .get(id)
                    .and_then(|document| document.path.clone());
                if let Some(path) = path {
                    self.remember_recent(&path, cx);
                }
                match outcome {
                    Installed::OnScreen => {
                        self.present_active(window, cx);
                        self.announce_loaded(&name, from_recovery, recovery_lost, cx);
                    }
                    Installed::Background => cx.notify(),
                    Installed::Gone => {}
                }
            }
            LinkLoad::Missing => {
                self.documents.set_link_status(id, LinkStatus::Missing);
                let text =
                    format!("{name} was not found. It stays in the sidebar until you remove it.");
                self.notify(Severity::Warning, text, cx);
            }
            LinkLoad::Failed(message) => {
                self.documents.set_link_status(id, LinkStatus::NotLoaded);
                self.notify(Severity::Error, format!("{name}: {message}"), cx);
            }
        }
    }

    // The copy moves under this session's own name only once its workbook is in the list, and
    // the session file is rewritten right after, so no crash leaves the two disagreeing for long.
    fn adopt_recovery(&mut self, id: WorkbookId, copy: PathBuf, cx: &mut Context<Self>) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let target = recovery::document_file(&directory, id);
        cx.spawn(async move |this, cx| {
            let moved = cx
                .background_executor()
                .spawn(
                    async move { recovery::adopt(&copy, &target).map_err(|error| (copy, error)) },
                )
                .await;
            if let Err((copy, error)) = moved {
                tracing::warn!(?copy, %error, "could not adopt the recovery file");
            }
            if let Err(error) = this.update(cx, |this, cx| this.persist_session(cx)) {
                tracing::debug!(%error, "workspace closed while adopting a recovery file");
            }
        })
        .detach();
    }

    fn announce_loaded(
        &mut self,
        name: &str,
        from_recovery: bool,
        recovery_lost: bool,
        cx: &mut Context<Self>,
    ) {
        if recovery_lost {
            let text = format!(
                "The unsaved changes of {name} could not be recovered; this is the saved file."
            );
            self.notify(Severity::Warning, text, cx);
        } else if from_recovery {
            let text = format!("Restored the unsaved changes of {name}. Save to keep them.");
            self.notify(Severity::Warning, text, cx);
        } else {
            self.announce_opened(None, cx);
        }
    }

    // Whether each listed file still exists and how big it is, one background task per file so
    // a stalled share holds up only its own answer. Network and device paths are left alone
    // until the file is opened: asking about them can send credentials to a host the user has
    // not touched this session.
    pub(super) fn probe_links(&mut self, cx: &mut Context<Self>) {
        let probes: Vec<(WorkbookId, PathBuf)> = self
            .documents
            .entries()
            .filter_map(|entry| match entry {
                Entry::Link(link) => link
                    .path
                    .clone()
                    .filter(|path| !files::is_remote_or_device(path))
                    .map(|path| (link.id, path)),
                Entry::Loaded(_) => None,
            })
            .collect();
        for (id, path) in probes {
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { std::fs::metadata(path) })
                    .await;
                let update = this.update(cx, |this, cx| {
                    let waiting = matches!(
                        this.documents.entry(id),
                        Some(Entry::Link(link)) if link.status == LinkStatus::NotLoaded
                    );
                    match result {
                        Ok(metadata) => this.documents.set_link_size(id, Some(metadata.len())),
                        Err(error) if waiting && error.kind() == std::io::ErrorKind::NotFound => {
                            this.documents.set_link_status(id, LinkStatus::Missing)
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                });
                if let Err(error) = update {
                    tracing::debug!(%error, "workspace closed while probing a file");
                }
            })
            .detach();
        }
    }
}
