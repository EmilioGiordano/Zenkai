use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;
use zenkai_engine::{EngineError, open_xlsx};
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::document::{self, FileJob, SharedWorkbook};
use crate::documents::Intent;
use crate::recovery;
use crate::session::{self, Loaded};

const CLOSING: &str = "Saving your session…";
const CLOSE_POLL: Duration = Duration::from_millis(50);
const CLOSE_POLLS_BEFORE_ASKING: u32 = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lifecycle {
    Running,
    Closing,
}

struct RecoveryWrite {
    id: WorkbookId,
    generation: u64,
    workbook: SharedWorkbook,
    target: PathBuf,
}

impl Workspace {
    // Opens the previous session next to the blank workbook, then the file asked for on the
    // command line, which wins the screen.
    pub(super) fn start_session(
        &mut self,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(directory) = recovery::directory() else {
            tracing::warn!("no data directory for recovery files; autosave is off");
            self.apply_session(Loaded::Absent, Vec::new(), initial, window, cx);
            return;
        };
        match recovery::lock_session(&directory) {
            Ok(lock) => self.session_lock = Some(lock),
            Err(error) => {
                tracing::warn!(%error, "could not lock the recovery session; autosave is off");
                self.notify(
                    Severity::Warning,
                    format!("Autosave is off, the recovery folder is not usable: {error}"),
                    cx,
                );
                self.apply_session(Loaded::Absent, Vec::new(), initial, window, cx);
                return;
            }
        }
        self.recovery_dir = Some(directory.clone());
        cx.spawn_in(window, async move |this, cx| {
            let task_directory = directory.clone();
            let (session, leftovers) = cx
                .background_executor()
                .spawn(async move {
                    let session = recovery::session_directory()
                        .map_or(Loaded::Absent, |directory| session::load(&directory));
                    (session, recovery::leftovers(&task_directory))
                })
                .await;
            if let Err(error) = this.update_in(cx, |this, window, cx| {
                this.apply_session(session, leftovers, initial, window, cx)
            }) {
                tracing::debug!(%error, "workspace closed before the session was restored");
            }
            loop {
                cx.background_executor()
                    .timer(recovery::AUTOSAVE_EVERY)
                    .await;
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
        leftovers: Vec<PathBuf>,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut referenced: Vec<String> = Vec::new();
        match loaded {
            Loaded::Restored(session) => {
                self.sidebar.visible = session.sidebar_visible;
                self.sync_memory_sampler(cx);
                referenced = session
                    .recovery_files()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                if let Some(active) = self
                    .documents
                    .restore(&session, self.recovery_dir.as_deref())
                {
                    self.documents.want(active, Intent::Restore);
                    self.begin_load(active, window, cx);
                }
                self.probe_links(cx);
            }
            Loaded::Unreadable => self.notify(
                Severity::Warning,
                "The previous session could not be read. It was kept as session.json.unreadable.",
                cx,
            ),
            Loaded::Absent => {}
        }
        // Unsaved work the session points to comes back with its workbook; anything else a
        // crashed run left behind is still offered.
        let strays: Vec<PathBuf> = leftovers
            .into_iter()
            .filter(|path| {
                !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| referenced.iter().any(|kept| kept == name))
            })
            .collect();
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
                        format!("Autosave failed, recovery is not protecting this work: {error}"),
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
        let detail = format!(
            "Zenkai closed unexpectedly and kept {} unsaved workbook(s). Open them?",
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
                            std::fs::rename(&path, recovery::document_file(directory, id))
                    {
                        tracing::warn!(?path, %error, "could not adopt the recovery file");
                    }
                    recovery::remove_with_lock(&path);
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

    // Closing keeps unsaved work: every workbook holding some is written to its recovery
    // copy, and the session lists them so the next start brings them back.
    pub(super) fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lifecycle == Lifecycle::Closing {
            return;
        }
        self.lifecycle = Lifecycle::Closing;
        self.park_active(window, cx);
        self.busy = Some(CLOSING.into());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let mut polls = 0;
            let writes = loop {
                let ready = this.update(cx, |this, _| this.begin_recovery_writes());
                match ready {
                    Ok(Some(writes)) => break writes,
                    Ok(None) if polls < CLOSE_POLLS_BEFORE_ASKING => polls += 1,
                    Ok(None) => {
                        this.update_in(cx, |this, window, cx| {
                            this.ask_to_quit_anyway("A workbook is still busy.", window, cx)
                        })
                        .ok();
                        return;
                    }
                    Err(_) => return,
                }
                cx.background_executor().timer(CLOSE_POLL).await;
            };
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
                            let result = recovery::write(
                                &document::read_shared(&write.workbook),
                                &write.target,
                            );
                            (write.id, result)
                        })
                        .collect::<Vec<(WorkbookId, Result<(), EngineError>)>>()
                })
                .await;
            let failed = this.update_in(cx, |this, window, cx| {
                for (id, generation) in &generations {
                    if let Some(document) = this.documents.get_mut(*id) {
                        document.end_file_job(*generation);
                    }
                }
                let failures: Vec<String> = results
                    .iter()
                    .filter_map(|(_, result)| result.as_ref().err().map(ToString::to_string))
                    .collect();
                if failures.is_empty() {
                    this.finish_close_with_session(window, cx);
                } else {
                    this.ask_to_quit_anyway(&failures.join("; "), window, cx);
                }
            });
            if let Err(error) = failed {
                tracing::debug!(%error, "workspace gone while closing");
            }
        })
        .detach();
    }

    // `None` while a recalculation or an autosave still holds a workbook that needs saving.
    fn begin_recovery_writes(&mut self) -> Option<Vec<RecoveryWrite>> {
        let Some(directory) = self.recovery_dir.clone() else {
            return Some(Vec::new());
        };
        let ready = self
            .documents
            .iter()
            .filter(|document| document.needs_recovery())
            .all(|document| document.file_job() == FileJob::Idle && !document.has_pending());
        if !ready {
            return None;
        }
        let writes = self
            .documents
            .iter_mut()
            .filter(|document| document.needs_recovery())
            .filter_map(|document| {
                let workbook = document.begin_file_job(FileJob::Autosaving)?;
                Some(RecoveryWrite {
                    id: document.id,
                    generation: document.generation(),
                    workbook,
                    target: recovery::document_file(&directory, document.id),
                })
            })
            .collect();
        Some(writes)
    }

    fn finish_close_with_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unprotected = self.recovery_dir.is_none()
            && self
                .documents
                .iter()
                .any(|document| document.needs_recovery());
        if unprotected {
            self.ask_to_quit_anyway("Autosave is off, so nothing can be kept.", window, cx);
            return;
        }
        let directory = self.recovery_dir.clone();
        let session = self.documents.snapshot(self.sidebar.visible, |id| {
            directory
                .as_deref()
                .map(|directory| recovery::document_file(directory, id))
                .unwrap_or_default()
        });
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
        let detail = format!(
            "{reason} Quitting now loses the changes that are not saved. Cancel to keep working."
        );
        let answer = window.prompt(
            PromptLevel::Critical,
            "Your unsaved work could not be kept",
            Some(&detail),
            &["Cancel", "Quit anyway"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let quit = answer.await == Ok(1);
            let update = this.update(cx, |this, cx| {
                if quit {
                    cx.quit();
                } else {
                    this.lifecycle = Lifecycle::Running;
                    this.clear_busy(CLOSING, cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace gone while closing");
            }
        })
        .detach();
    }
}
