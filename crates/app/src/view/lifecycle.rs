use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gpui_kit::*;
use zenkai_agent::preferences::CloseWithUnsaved;
use zenkai_engine::{EngineError, SaveTarget, Unsupported, open_xlsx};
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::text_save::write_recovery;
use super::{Severity, Workspace};
use crate::autosave::{self, Due};
use crate::close_prompt::{CloseGoal, ClosePlan};
use crate::document::{self, FileJob, SharedWorkbook, SnapshotOutcome};
use crate::documents::Intent;
use crate::entry::Entry;
use crate::job_pill::LongJob;
use crate::pending_save::{AfterJob, JobEnd};
use crate::recovery;
use zenkai_agent::settings_file::{self, SettingsPaths};

use crate::agent_settings;
use crate::session::{self, Loaded, Session};
use crate::space_settings;
use crate::text_disk::Writing;
use crate::text_file::Format;

const AUTOSAVE_TICK: Duration = Duration::from_secs(1);
const CLOSE_POLL: Duration = Duration::from_millis(50);
const CLOSE_POLLS_BEFORE_ASKING: u32 = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lifecycle {
    Running,
    Closing,
}

struct RecoveryBatch {
    writes: Vec<RecoveryWrite>,
    texts: Vec<TextRecoveryWrite>,
    edits: Vec<(WorkbookId, u64)>,
}

struct TextRecoveryWrite {
    id: WorkbookId,
    text: String,
    format: Format,
    target: PathBuf,
}

struct RecoveryWrite {
    id: WorkbookId,
    generation: u64,
    workbook: SharedWorkbook,
    target: PathBuf,
}

// Read from the file itself: the settings state loads in parallel with the window, so it may
// not be ready when the session is.
fn restore_enabled() -> bool {
    let Ok(paths) = SettingsPaths::from_environment() else {
        return true;
    };
    match settings_file::load(&paths.settings()) {
        Ok(settings) => settings.general.restore_session,
        Err(error) => {
            tracing::warn!(%error, "settings could not be read; restoring the session");
            true
        }
    }
}

fn strays(leftovers: Vec<PathBuf>, referenced: &[String]) -> Vec<PathBuf> {
    leftovers
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| referenced.iter().any(|kept| kept == name))
        })
        .collect()
}

impl Workspace {
    // A file on the command line wins the screen over the restored workbook.
    pub(super) fn start_session(
        &mut self,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(directory) = recovery::directory() else {
            tracing::warn!("no data directory for recovery files; autosave is off");
            self.apply_session(Loaded::Absent, true, Vec::new(), initial, window, cx);
            return;
        };
        match recovery::lock_session(&directory) {
            Ok(lock) => self.session_lock = Some(lock),
            Err(error) => {
                tracing::warn!(%error, "could not lock the recovery session; autosave is off");
                self.notify(
                    Severity::Warning,
                    t!("notice.autosave_off_folder", error = error),
                    cx,
                );
                self.apply_session(Loaded::Absent, true, Vec::new(), initial, window, cx);
                return;
            }
        }
        self.recovery_dir = Some(directory.clone());
        cx.spawn_in(window, async move |this, cx| {
            let task_directory = directory.clone();
            let (session, restore, leftovers) = cx
                .background_executor()
                .spawn(async move {
                    recovery::remove_dead_dumps(&task_directory);
                    let session = recovery::session_directory()
                        .map_or(Loaded::Absent, |directory| session::load(&directory));
                    (
                        session,
                        restore_enabled(),
                        recovery::leftovers(&task_directory),
                    )
                })
                .await;
            if let Err(error) = this.update_in(cx, |this, window, cx| {
                this.apply_session(session, restore, leftovers, initial, window, cx)
            }) {
                tracing::debug!(%error, "workspace closed before the session was restored");
            }
            // Short ticks against the elapsed time, so a new interval applies at once.
            let mut persisted = Instant::now();
            loop {
                cx.background_executor().timer(AUTOSAVE_TICK).await;
                let update = this.update_in(cx, |this, window, cx| {
                    let every = Duration::from_secs(u64::from(
                        agent_settings::settings(cx).general.autosave_seconds,
                    ));
                    this.autosave_all(every, window, cx);
                    if persisted.elapsed() >= every {
                        persisted = Instant::now();
                        this.persist_session(cx);
                    }
                });
                if update.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_session(
        &mut self,
        loaded: Loaded,
        restore: bool,
        leftovers: Vec<PathBuf>,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut referenced: Vec<String> = Vec::new();
        match loaded {
            Loaded::Restored(session) => {
                let session = if restore {
                    session
                } else {
                    session.without_unsaved_work()
                };
                let reopened_as_saved = session.reopened_as_saved;
                cx.set_global(session.space_appearance);
                referenced = session
                    .recovery_files()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                self.sidebar.visible = session.sidebar_visible;
                self.panels.widths = session.panel_widths.validated();
                self.pdf_places = crate::pdf_places::PdfPlaces::from_records(&session.pdf_places);
                if let Some(active) = self
                    .documents
                    .restore(&session, self.recovery_dir.as_deref())
                {
                    self.documents.want(active, Intent::Restore);
                    self.begin_load(active, window, cx);
                }
                self.probe_links(cx);
                if reopened_as_saved > 0 {
                    self.notify(
                        Severity::Info,
                        t!("notice.reopened_as_saved", count = reopened_as_saved),
                        cx,
                    );
                    // The next write drops the count, so the notice shows once.
                    self.persist_session(cx);
                }
            }
            Loaded::Unreadable => {
                self.notify(Severity::Warning, t!("notice.session_unreadable"), cx)
            }
            Loaded::Absent => {}
        }
        // Unsaved work the session points to comes back with its workbook; anything else a
        // crashed run left behind is still offered.
        let strays = strays(leftovers, &referenced);
        if !strays.is_empty() {
            self.offer_recovery(strays, window, cx);
        }
        if let Some(path) = initial {
            self.open_path(path, window, cx);
        }
        cx.notify();
    }

    fn autosave_all(&mut self, every: Duration, window: &mut Window, cx: &mut Context<Self>) {
        // Closing writes its own copies; a stray autosave could outlive a discarded workbook.
        if self.lifecycle == Lifecycle::Closing {
            return;
        }
        let ids: Vec<WorkbookId> = self.documents.iter().map(|document| document.id).collect();
        for id in ids {
            self.autosave(id, every, window, cx);
        }
        self.autosave_texts(every, window, cx);
        self.watch_texts(window, cx);
    }

    fn autosave(
        &mut self,
        id: WorkbookId,
        every: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        let due = autosave::due(
            document.since_last_edit(),
            document.since_last_autosave_attempt(),
            document.recovery_stale_for(),
            every,
        );
        if due == Due::No {
            return;
        }
        let target = recovery::document_file(&directory, id);
        if !document.needs_recovery() {
            document.mark_autosave_attempt();
            document.set_autosaved_at(None);
            cx.background_executor()
                .spawn(async move { recovery::remove(&target) })
                .detach();
            return;
        }
        // Writing a large workbook takes seconds; nothing changed means the copy on disk
        // already holds this state.
        if document.recovery_is_current() {
            return;
        }
        let Some(shared) = document.begin_file_job(FileJob::Autosaving) else {
            return;
        };
        document.mark_autosave_attempt();
        let cancelled = match due {
            Due::Overdue => Arc::new(AtomicBool::new(false)),
            Due::AfterPause | Due::No => document.autosave_cancel(),
        };
        let edits = document.edit_count();
        let generation = document.generation();
        self.start_long_job(LongJob::Autosaving, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    document::write_snapshot_unless(
                        &shared,
                        FileJob::Autosaving,
                        &target,
                        SaveTarget::Copy,
                        &cancelled,
                        recovery::write,
                    )
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.end_long_job(LongJob::Autosaving, cx);
                let Some(document) = this.documents.get_mut(id) else {
                    return;
                };
                document.end_file_job(generation);
                if !document.is_current(generation) {
                    return;
                }
                let follow_up = document.settle_save(generation, JobEnd::Autosaved);
                match result {
                    Ok(SnapshotOutcome::Written { lacks }) => {
                        document.recovery_written(edits);
                        // A restored copy must not be saved over a file it lacks content of.
                        if document.recovery_lacks(&lacks) {
                            this.persist_session(cx);
                        }
                    }
                    Ok(SnapshotOutcome::Abandoned) => {}
                    Err(error) => this.notify(
                        Severity::Warning,
                        t!("notice.autosave_failed", error = error),
                        cx,
                    ),
                }
                if follow_up == AfterJob::SaveAgain {
                    this.save_again(id, window, cx);
                }
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = t!("recovery.detail", count = leftovers.len());
        let answer = self.ask(
            PromptLevel::Warning,
            t!("recovery.title"),
            Some(&detail),
            &[t!("recovery.open"), t!("button.discard")],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Some(choice) = answer.await else {
                return;
            };
            if choice == 1 {
                leftovers
                    .iter()
                    .for_each(|path| recovery::remove_with_lock(path));
                return;
            }
            let update = this.update_in(cx, |this, window, cx| {
                for path in leftovers {
                    this.load_recovered(path, window, cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during recovery");
            }
        })
        .detach();
    }

    fn load_recovered(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if recovery::is_text(&path) {
            self.load_recovered_text(path, window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move { open_xlsx(&path).map(|opened| (opened, path)) })
                .await;
            let update = this.update_in(cx, |this, window, cx| match opened {
                Ok((opened, path)) => {
                    let id =
                        this.open_document(opened.workbook, None, opened.unsupported, window, cx);
                    if let Some(document) = this.documents.get_mut(id) {
                        document.dirty = true;
                    }
                    window.set_window_title(&this.documents.shown_title());
                    // Kept as this document's own recovery copy until the work is saved.
                    if let Some(directory) = &this.recovery_dir {
                        match recovery::adopt(&path, &recovery::document_file(directory, id)) {
                            Ok(()) => {
                                if let Some(document) = this.documents.get_mut(id) {
                                    document.mark_recovery_adopted();
                                }
                            }
                            Err(error) => {
                                tracing::warn!(?path, %error, "could not adopt the recovery file")
                            }
                        }
                    }
                    this.notify(Severity::Warning, t!("notice.recovered_work"), cx);
                }
                Err(error) => this.notify(
                    Severity::Error,
                    t!("notice.recover_failed", error = error),
                    cx,
                ),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during recovery");
            }
        })
        .detach();
    }

    // With the default setting the unsaved workbooks are listed first and the user decides
    // which to save; "keep changes" goes straight to recovery copies.
    pub(crate) fn request_close(
        &mut self,
        goal: CloseGoal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(windows)]
        if !goal.keeps_running() && self.lifecycle == Lifecycle::Closing {
            self.quit_after_hide = true;
        }
        if !goal.keeps_running()
            && let Some(departure) = self.departure.as_mut()
        {
            departure.plan.goal = goal;
        }
        if self.close_dialog.is_some() {
            self.retarget_close_dialog(goal);
            self.focus_close_dialog(window, cx);
            return;
        }
        if self.lifecycle == Lifecycle::Closing
            || self.departure.is_some()
            || self.prompts.blocks_input()
        {
            return;
        }
        if !goal.keeps_running() {
            self.park_active(window, cx);
        }
        let general = &agent_settings::settings(cx).general;
        let when_unsaved = if goal.keeps_running() {
            general.close_window_with_unsaved
        } else {
            general.close_with_unsaved
        };
        let asks = when_unsaved == CloseWithUnsaved::Ask;
        let reopen_offered = general.restore_session && !goal.keeps_running();
        let unsaved = self.documents.unsaved();
        if asks && !unsaved.is_empty() {
            self.ask_to_close_app(unsaved, reopen_offered, goal, window, cx);
            return;
        }
        let plan = ClosePlan {
            goal,
            ..ClosePlan::default()
        };
        self.run_close(plan, window, cx);
    }

    // Closing keeps unsaved work: every workbook holding some, except the ones the plan
    // discards, is written to its recovery copy, and the session lists them so the next start
    // brings them back.
    pub(super) fn run_close(
        &mut self,
        plan: ClosePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle == Lifecycle::Closing {
            return;
        }
        self.lifecycle = Lifecycle::Closing;
        self.park_active(window, cx);
        self.busy = Some(t!("busy.closing").into());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let mut polls = 0;
            let batch = loop {
                match this.update(cx, |this, cx| this.begin_recovery_writes(&plan, cx)) {
                    Ok(Some(batch)) => break batch,
                    Ok(None) if polls < CLOSE_POLLS_BEFORE_ASKING => polls += 1,
                    Ok(None) => {
                        let asked = this.update_in(cx, |this, window, cx| {
                            #[cfg(windows)]
                            if plan.goal == CloseGoal::Hide {
                                this.abandon_hide(t!("quit.busy_reason"), cx);
                                return;
                            }
                            this.ask_to_quit_anyway(
                                t!("quit.busy_reason"),
                                plan.clone(),
                                window,
                                cx,
                            )
                        });
                        if let Err(error) = asked {
                            tracing::debug!(%error, "workspace gone while closing");
                        }
                        return;
                    }
                    Err(_) => return,
                }
                cx.background_executor().timer(CLOSE_POLL).await;
            };
            let RecoveryBatch {
                writes,
                texts,
                edits,
            } = batch;
            let text_ids: Vec<WorkbookId> = texts.iter().map(|write| write.id).collect();
            let generations: Vec<(WorkbookId, u64)> = writes
                .iter()
                .map(|write| (write.id, write.generation))
                .collect();
            let (results, text_results) = cx
                .background_executor()
                .spawn(async move {
                    let workbooks = writes
                        .into_iter()
                        .map(|write| {
                            document::write_snapshot(
                                &write.workbook,
                                FileJob::Autosaving,
                                &write.target,
                                SaveTarget::Copy,
                                recovery::write,
                            )
                        })
                        .collect::<Vec<Result<Vec<Unsupported>, EngineError>>>();
                    let texts = texts
                        .into_iter()
                        .map(|write| write_recovery(&write.target, &write.text, write.format))
                        .collect::<Vec<Result<(), String>>>();
                    (workbooks, texts)
                })
                .await;
            let finished = this.update_in(cx, |this, window, cx| {
                for ((id, generation), result) in generations.iter().zip(&results) {
                    if let Some(document) = this.documents.get_mut(*id) {
                        document.end_file_job(*generation);
                        document.clear_pending_save();
                        if let Ok(lacks) = result {
                            document.mark_recovery_adopted();
                            document.recovery_lacks(lacks);
                        }
                    }
                }
                for (id, result) in text_ids.iter().zip(&text_results) {
                    if let Some(text) = this.documents.text_mut(*id) {
                        text.writing = Writing::Idle;
                        if result.is_ok() {
                            text.mark_recovery_adopted();
                        }
                    }
                }
                // Edits made while the copies were written are not in them: write again.
                if this.unsaved_edits_kept(&plan) != edits {
                    this.lifecycle = Lifecycle::Running;
                    this.run_close(plan.clone(), window, cx);
                    return;
                }
                let failures: Vec<String> = results
                    .iter()
                    .filter_map(|result| result.as_ref().err().map(ToString::to_string))
                    .chain(
                        text_results
                            .iter()
                            .filter_map(|result| result.as_ref().err().cloned()),
                    )
                    .collect();
                #[cfg(windows)]
                if plan.goal == CloseGoal::Hide {
                    if failures.is_empty() {
                        this.finish_hide(plan.clone(), window, cx);
                    } else {
                        this.abandon_hide(&failures.join("; "), cx);
                    }
                    return;
                }
                if failures.is_empty() {
                    this.finish_close_with_session(plan.clone(), window, cx);
                } else {
                    this.ask_to_quit_anyway(&failures.join("; "), plan.clone(), window, cx);
                }
            });
            if let Err(error) = finished {
                tracing::debug!(%error, "workspace gone while closing");
            }
        })
        .detach();
    }

    fn unsaved_edits_kept(&self, plan: &ClosePlan) -> Vec<(WorkbookId, u64)> {
        self.documents
            .unsaved_edits()
            .into_iter()
            .filter(|(id, _)| plan.keeps(*id))
            .collect()
    }

    // `None` while a recalculation or an autosave still holds a workbook that needs saving.
    fn begin_recovery_writes(&mut self, plan: &ClosePlan, cx: &App) -> Option<RecoveryBatch> {
        // A workbook the plan discards must be idle too: an autosave finishing after its copy
        // was deleted would leave a file the next start offers as crash recovery.
        if self
            .documents
            .iter()
            .any(|document| !plan.keeps(document.id) && document.file_job() != FileJob::Idle)
            || self
                .documents
                .texts()
                .any(|text| text.writing != Writing::Idle)
        {
            return None;
        }
        let edits = self.unsaved_edits_kept(plan);
        let Some(directory) = self.recovery_dir.clone() else {
            return Some(RecoveryBatch {
                writes: Vec::new(),
                texts: Vec::new(),
                edits,
            });
        };
        let mut writes = Vec::new();
        let mut blocked = false;
        for document in self
            .documents
            .iter_mut()
            .filter(|document| document.needs_recovery() && plan.keeps(document.id))
        {
            match document.begin_file_job(FileJob::Autosaving) {
                Some(workbook) => writes.push(RecoveryWrite {
                    id: document.id,
                    generation: document.generation(),
                    workbook,
                    target: recovery::document_file(&directory, document.id),
                }),
                None => {
                    blocked = true;
                    break;
                }
            }
        }
        if blocked {
            for write in &writes {
                if let Some(document) = self.documents.get_mut(write.id) {
                    document.end_file_job(write.generation);
                }
            }
            return None;
        }
        let mut texts = Vec::new();
        let kept: Vec<WorkbookId> = self
            .documents
            .texts()
            .filter(|text| text.needs_recovery() && plan.keeps(text.id))
            .map(|text| text.id)
            .collect();
        for id in kept {
            let Some(entry) = self.text_panes.get(&id) else {
                continue;
            };
            let content = entry.pane.read(cx).text(cx).to_string();
            if let Some(text) = self.documents.text_mut(id) {
                text.writing = Writing::Autosaving;
                texts.push(TextRecoveryWrite {
                    id,
                    text: content,
                    format: text.format,
                    target: recovery::text_file(&directory, id),
                });
            }
        }
        Some(RecoveryBatch {
            writes,
            texts,
            edits,
        })
    }

    fn finish_close_with_session(
        &mut self,
        plan: ClosePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.holds_unprotected_work(&plan) {
            self.ask_to_quit_anyway(t!("quit.autosave_off_reason"), plan, window, cx);
            return;
        }
        self.write_session_and_quit(plan, cx);
    }

    // Without a recovery folder, work kept in memory has no copy to come back from.
    pub(super) fn holds_unprotected_work(&self, plan: &ClosePlan) -> bool {
        self.recovery_dir.is_none()
            && (self
                .documents
                .iter()
                .any(|document| document.needs_recovery() && plan.keeps(document.id))
                || self
                    .documents
                    .texts()
                    .any(|text| text.needs_recovery() && plan.keeps(text.id)))
    }

    fn recovery_file_of(&self, id: WorkbookId) -> Option<PathBuf> {
        let directory = self.recovery_dir.as_deref()?;
        Some(if self.documents.text(id).is_some() {
            recovery::text_file(directory, id)
        } else {
            recovery::document_file(directory, id)
        })
    }

    fn snapshot_session(&self, cx: &App) -> Session {
        self.documents
            .snapshot(self.sidebar.visible, space_settings::current(cx), |id| {
                self.recovery_file_of(id)
            })
            .with_panel_widths(self.panels.widths)
            .with_pdf_places(self.pdf_places.records())
    }

    fn snapshot_session_leaving(&self, plan: &ClosePlan, cx: &App) -> Session {
        let session = self
            .documents
            .snapshot_leaving(
                self.sidebar.visible,
                space_settings::current(cx),
                |id| self.recovery_file_of(id),
                &plan.discard,
            )
            .with_panel_widths(self.panels.widths)
            .with_pdf_places(self.pdf_places.records());
        if plan.start_empty {
            session.without_unsaved_work()
        } else {
            session
        }
    }

    // Written when something changed, so a crash leaves the spaces and files as they were a
    // minute ago instead of as they were at the last clean close.
    pub(super) fn persist_session(&mut self, cx: &mut Context<Self>) {
        let Some(directory) = recovery::session_directory() else {
            return;
        };
        if self.lifecycle == Lifecycle::Closing {
            return;
        }
        let session = self.snapshot_session(cx);
        if self.saved_session.as_ref() == Some(&session) {
            return;
        }
        self.saved_session = Some(session.clone());
        self.unwritten_session = Some((directory, session, session::SaveOrder::next()));
        if !self.session_writing {
            self.session_writing = true;
            self.write_next_session(cx);
        }
    }

    // One background write at a time, always of the latest snapshot, so a burst of changes such
    // as zoom steps neither queues a write each nor lets an older one land after a newer one.
    fn write_next_session(&mut self, cx: &mut Context<Self>) {
        let Some((directory, session, order)) = self.unwritten_session.take() else {
            self.session_writing = false;
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = session::save_in_order(&directory, &session, order) {
                        tracing::warn!(%error, "could not save the session");
                    }
                })
                .await;
            if let Err(error) = this.update(cx, |this, cx| this.write_next_session(cx)) {
                tracing::debug!(%error, "the workspace closed during a session write");
            }
        })
        .detach();
    }

    // The spaces and links are kept even when unsaved work could not be.
    pub(super) fn write_session_and_quit(&mut self, plan: ClosePlan, cx: &mut Context<Self>) {
        let session = self.snapshot_session_leaving(&plan, cx);
        let order = session::SaveOrder::next();
        let session_directory = recovery::session_directory();
        let dumps_directory = self.recovery_dir.clone();
        let mut clean: Vec<PathBuf> = self
            .documents
            .iter()
            .filter(|document| !document.needs_recovery() || !plan.keeps(document.id))
            .filter_map(|document| self.recovery_file_of(document.id))
            .chain(
                self.documents
                    .texts()
                    .filter(|text| !text.needs_recovery() || !plan.keeps(text.id))
                    .filter_map(|text| self.recovery_file_of(text.id)),
            )
            .collect();
        clean.extend(
            plan.discard
                .iter()
                .filter_map(|id| match self.documents.entry(*id) {
                    Some(Entry::Link(link)) => link.recovery.clone(),
                    _ => None,
                }),
        );
        cx.spawn(async move |_, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    // The copies go only once the session no longer points at them.
                    let saved = match session_directory {
                        Some(directory) => session::save_in_order(&directory, &session, order),
                        None => Ok(()),
                    };
                    if saved.is_ok() {
                        clean.iter().for_each(|file| recovery::remove(file));
                        if let Some(directory) = &dumps_directory {
                            recovery::remove_own_dumps(directory);
                        }
                    }
                    saved
                })
                .await;
            if let Err(error) = written {
                tracing::warn!(%error, "could not save the session");
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    pub(super) fn ask_to_quit_anyway(
        &mut self,
        reason: &str,
        plan: ClosePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(windows)]
        crate::tray::request_show(cx);
        let (detail, proceed_label) = if plan.goal.keeps_running() {
            (t!("hide.detail", reason = reason), t!("hide.anyway"))
        } else {
            (t!("quit.detail", reason = reason), t!("quit.anyway"))
        };
        let answer = self.ask(
            PromptLevel::Critical,
            t!("quit.title"),
            Some(&detail),
            &[t!("button.cancel"), proceed_label],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let proceed = answer.await == Some(1);
            let update = this.update_in(cx, |this, window, cx| {
                if proceed {
                    this.quit_or_hide(plan, window, cx);
                } else {
                    #[cfg(windows)]
                    {
                        this.quit_after_hide = false;
                    }
                    this.lifecycle = Lifecycle::Running;
                    this.clear_busy(t!("busy.closing"), cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace gone while closing");
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::strays;
    use crate::session::Session;
    use crate::session::{FileRecord, SpaceRecord, ViewRecord};

    fn session_pointing_at(recovery_file: &str) -> Session {
        let file = FileRecord {
            path: None,
            recovery: Some(recovery_file.to_string()),
            untitled: 1,
            dirty: true,
            read_only: false,
            kind: Default::default(),
            mode: Default::default(),
            base: None,
            pdf: Default::default(),
            text_zoom: Default::default(),
            unsupported: Vec::new(),
            active: true,
            sheet: 0,
            view: ViewRecord {
                active_row: 0,
                active_col: 0,
                corner_row: 0,
                corner_col: 0,
                top: 0,
                left: 0,
            },
        };
        Session::new(
            true,
            vec![SpaceRecord {
                name: "Work".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![file],
            }],
        )
    }

    fn referenced(session: &Session) -> Vec<String> {
        session
            .recovery_files()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_restored_session_keeps_its_unsaved_work_out_of_the_recovery_offer() {
        let kept = referenced(&session_pointing_at("autosave-1-2-3.xlsx"));
        let leftovers = vec![
            PathBuf::from("recovery/autosave-1-2-3.xlsx"),
            PathBuf::from("recovery/autosave-9-9-9.xlsx"),
        ];
        assert_eq!(
            strays(leftovers, &kept),
            [PathBuf::from("recovery/autosave-9-9-9.xlsx")]
        );
    }

    #[test]
    fn without_a_restore_the_previous_sessions_unsaved_work_is_still_offered() {
        let session = session_pointing_at("autosave-1-2-3.xlsx").without_unsaved_work();
        let kept = referenced(&session);
        assert!(kept.is_empty());
        let leftovers = vec![PathBuf::from("recovery/autosave-1-2-3.xlsx")];
        assert_eq!(strays(leftovers.clone(), &kept), leftovers);
    }

    #[test]
    fn without_a_restore_the_spaces_and_the_links_to_files_survive() {
        let mut session = session_pointing_at("autosave-1-2-3.xlsx");
        let mut saved = session.spaces[0].files[0].clone();
        saved.path = Some("data/q3.xlsx".to_string());
        saved.recovery = None;
        saved.dirty = false;
        session.spaces[0].files.push(saved);
        let kept = session.without_unsaved_work();
        assert_eq!(kept.spaces.len(), 1);
        assert_eq!(kept.spaces[0].name, "Work");
        assert_eq!(kept.spaces[0].files.len(), 1);
        assert_eq!(
            kept.spaces[0].files[0].path.as_deref(),
            Some("data/q3.xlsx")
        );
        assert!(kept.spaces[0].files.iter().all(|file| !file.active));
        assert!(kept.recovery_files().is_empty());
    }
}
