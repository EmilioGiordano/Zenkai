use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::*;
use zenkai_engine::{EngineError, open_xlsx};
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::document::{self, FileJob, SharedWorkbook};
use crate::documents::Intent;
use crate::recovery;
use zenkai_agent::settings_file::{self, SettingsPaths};

use crate::agent_settings;
use crate::session::{self, Loaded, Session};
use crate::space_settings;

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
    edits: Vec<(WorkbookId, u64)>,
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
            'cycles: loop {
                let started = Instant::now();
                loop {
                    cx.background_executor().timer(AUTOSAVE_TICK).await;
                    let Ok(every) = this.update(cx, |_, cx| {
                        Duration::from_secs(u64::from(
                            agent_settings::settings(cx).general.autosave_seconds,
                        ))
                    }) else {
                        break 'cycles;
                    };
                    if started.elapsed() >= every {
                        break;
                    }
                }
                if this.update(cx, |this, cx| this.autosave_all(cx)).is_err() {
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
                cx.set_global(session.space_appearance);
                referenced = session
                    .recovery_files()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                self.sidebar.visible = session.sidebar_visible;
                self.panels.widths = session.panel_widths.validated();
                if let Some(active) = self
                    .documents
                    .restore(&session, self.recovery_dir.as_deref())
                {
                    self.documents.want(active, Intent::Restore);
                    self.begin_load(active, window, cx);
                }
                self.probe_links(cx);
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

    fn autosave_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<WorkbookId> = self.documents.iter().map(|document| document.id).collect();
        for id in ids {
            self.autosave(id, cx);
        }
        self.persist_session(cx);
    }

    fn autosave(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        let target = recovery::document_file(&directory, id);
        if !document.needs_recovery() {
            cx.background_executor()
                .spawn(async move { recovery::remove(&target) })
                .detach();
            return;
        }
        let Some(shared) = document.begin_file_job(FileJob::Autosaving) else {
            return;
        };
        let generation = document.generation();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { recovery::write(&document::read_shared(&shared), &target) })
                .await;
            let update = this.update(cx, |this, cx| {
                let Some(document) = this.documents.get_mut(id) else {
                    return;
                };
                document.end_file_job(generation);
                if !document.is_current(generation) {
                    return;
                }
                if let Err(error) = result {
                    tracing::warn!(%error, "autosave failed");
                    this.notify(
                        Severity::Warning,
                        t!("notice.autosave_failed", error = error),
                        cx,
                    );
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
        let answer = window.prompt(
            PromptLevel::Warning,
            t!("recovery.title"),
            Some(&detail),
            &[t!("recovery.open"), t!("button.discard")],
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
                        window.set_window_title(&document.title());
                    }
                    // Kept as this document's own recovery copy until the work is saved.
                    if let Some(directory) = &this.recovery_dir
                        && let Err(error) =
                            recovery::adopt(&path, &recovery::document_file(directory, id))
                    {
                        tracing::warn!(?path, %error, "could not adopt the recovery file");
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

    // Closing keeps unsaved work: every workbook holding some is written to its recovery
    // copy, and the session lists them so the next start brings them back.
    pub(super) fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                match this.update(cx, |this, _| this.begin_recovery_writes()) {
                    Ok(Some(batch)) => break batch,
                    Ok(None) if polls < CLOSE_POLLS_BEFORE_ASKING => polls += 1,
                    Ok(None) => {
                        let asked = this.update_in(cx, |this, window, cx| {
                            this.ask_to_quit_anyway(t!("quit.busy_reason"), window, cx)
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
            let RecoveryBatch { writes, edits } = batch;
            let generations: Vec<(WorkbookId, u64)> = writes
                .iter()
                .map(|write| (write.id, write.generation))
                .collect();
            let results = cx
                .background_executor()
                .spawn(async move {
                    writes
                        .into_iter()
                        .map(|write| {
                            recovery::write(&document::read_shared(&write.workbook), &write.target)
                        })
                        .collect::<Vec<Result<(), EngineError>>>()
                })
                .await;
            let finished = this.update_in(cx, |this, window, cx| {
                for (id, generation) in &generations {
                    if let Some(document) = this.documents.get_mut(*id) {
                        document.end_file_job(*generation);
                    }
                }
                // Edits made while the copies were written are not in them: write again.
                if this.documents.unsaved_edits() != edits {
                    this.lifecycle = Lifecycle::Running;
                    this.request_close(window, cx);
                    return;
                }
                let failures: Vec<String> = results
                    .iter()
                    .filter_map(|result| result.as_ref().err().map(ToString::to_string))
                    .collect();
                if failures.is_empty() {
                    this.finish_close_with_session(window, cx);
                } else {
                    this.ask_to_quit_anyway(&failures.join("; "), window, cx);
                }
            });
            if let Err(error) = finished {
                tracing::debug!(%error, "workspace gone while closing");
            }
        })
        .detach();
    }

    // `None` while a recalculation or an autosave still holds a workbook that needs saving.
    fn begin_recovery_writes(&mut self) -> Option<RecoveryBatch> {
        let Some(directory) = self.recovery_dir.clone() else {
            return Some(RecoveryBatch {
                writes: Vec::new(),
                edits: self.documents.unsaved_edits(),
            });
        };
        let edits = self.documents.unsaved_edits();
        let mut writes = Vec::new();
        let mut blocked = false;
        for document in self
            .documents
            .iter_mut()
            .filter(|document| document.needs_recovery())
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
        Some(RecoveryBatch { writes, edits })
    }

    fn finish_close_with_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unprotected = self.recovery_dir.is_none()
            && self
                .documents
                .iter()
                .any(|document| document.needs_recovery());
        if unprotected {
            self.ask_to_quit_anyway(t!("quit.autosave_off_reason"), window, cx);
            return;
        }
        self.write_session_and_quit(cx);
    }

    fn snapshot_session(&self, cx: &App) -> Session {
        let directory = self.recovery_dir.clone();
        self.documents
            .snapshot(self.sidebar.visible, space_settings::current(cx), |id| {
                directory
                    .as_deref()
                    .map(|directory| recovery::document_file(directory, id))
            })
            .with_panel_widths(self.panels.widths)
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
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = session::save(&directory, &session) {
                    tracing::warn!(%error, "could not save the session");
                }
            })
            .detach();
    }

    // The spaces and links are kept even when unsaved work could not be.
    fn write_session_and_quit(&mut self, cx: &mut Context<Self>) {
        let directory = self.recovery_dir.clone();
        let session = self.snapshot_session(cx);
        let session_directory = recovery::session_directory();
        let clean: Vec<PathBuf> = self
            .documents
            .iter()
            .filter(|document| !document.needs_recovery())
            .filter_map(|document| {
                directory
                    .as_deref()
                    .map(|directory| recovery::document_file(directory, document.id))
            })
            .collect();
        cx.spawn(async move |_, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    clean.iter().for_each(|file| recovery::remove(file));
                    match session_directory {
                        Some(directory) => session::save(&directory, &session),
                        None => Ok(()),
                    }
                })
                .await;
            if let Err(error) = written {
                tracing::warn!(%error, "could not save the session");
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    fn ask_to_quit_anyway(&mut self, reason: &str, window: &mut Window, cx: &mut Context<Self>) {
        let detail = t!("quit.detail", reason = reason);
        let answer = window.prompt(
            PromptLevel::Critical,
            t!("quit.title"),
            Some(&detail),
            &[t!("button.cancel"), t!("quit.anyway")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let quit = answer.await == Ok(1);
            let update = this.update(cx, |this, cx| {
                if quit {
                    this.write_session_and_quit(cx);
                } else {
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
