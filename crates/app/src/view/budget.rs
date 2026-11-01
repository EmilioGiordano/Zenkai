use std::time::{Duration, Instant};

use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::lifecycle::Lifecycle;
use super::{Severity, Workspace};
use crate::agent_settings;
use crate::document;
use crate::documents::{Dumped, Hibernating};
use crate::memory::{self, Candidate, Resting, Sleeper};
use crate::pending_save::{AfterJob, JobEnd};
use crate::recovery;

const SAMPLE_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum RestReason {
    MemoryPressure,
    Idle { minutes: u32 },
}

// A saved workbook that idles past the dump limits still leaves memory, as it did before
// dumps: its file is all it needs, only the undo history is lost.
fn unloads_when_refused(reason: RestReason, holds_unsaved_work: bool) -> bool {
    matches!(reason, RestReason::Idle { .. }) && !holds_unsaved_work
}

impl Workspace {
    pub(super) fn start_memory_sampler(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.memory_sampler = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                if let Some(memory_mb) = memory::current_committed_mb() {
                    let update = this.update_in(cx, |this, window, cx| {
                        this.memory_mb = memory_mb;
                        if this.sidebar.visible || this.diagnostics {
                            cx.notify();
                        }
                        this.enforce_memory_budget(window, cx);
                        this.rest_when_idle(window, cx);
                    });
                    if update.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(SAMPLE_EVERY).await;
            }
        }));
    }

    // The workbook on screen, the one being waited for and the one a change awaits approval on
    // never rest. A saved workbook is unloaded; one with unsaved work is hibernated, if its
    // recovery copy holds all of it.
    fn rest_candidates(&self) -> Vec<Candidate> {
        let active = self.documents.active_id();
        let wanted = self.documents.wanted();
        let awaiting_approval = self.awaiting_approval();
        let can_hibernate = self.recovery_dir.is_some() && self.hibernating.is_none();
        self.documents
            .iter()
            .map(|document| {
                let free = Some(document.id) != active
                    && Some(document.id) != wanted
                    && Some(document.id) != awaiting_approval;
                let resting = if !free {
                    None
                } else if document.can_unload() {
                    Some(Resting::Unload)
                } else if can_hibernate && document.can_hibernate() {
                    Some(Resting::Hibernate)
                } else {
                    None
                };
                Candidate {
                    id: document.id,
                    last_used: document.last_used,
                    resting,
                }
            })
            .collect()
    }

    fn enforce_memory_budget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.memory_mb <= self.memory_budget_mb {
            return;
        }
        let since_last = self.last_unload.map(|at| at.elapsed());
        let candidates = self.rest_candidates();
        let chosen = memory::next_under_pressure(
            self.memory_mb,
            self.memory_budget_mb,
            since_last,
            &candidates,
        );
        if let Some((id, resting)) = chosen {
            self.rest(id, resting, window, cx);
        }
    }

    // Workbooks off screen rest after the configured time. With the window hidden in the tray
    // the one on screen rests too, and comes back when the window is shown.
    fn rest_when_idle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(windows)]
        let hidden = crate::tray::hidden_to_tray(cx);
        #[cfg(not(windows))]
        let hidden = false;
        if hidden {
            self.hidden_since.get_or_insert_with(Instant::now);
        } else {
            self.hidden_since = None;
            self.wake_stepped_aside(window, cx);
        }
        // A close writes its own copies, and a dump could outlive a workbook it discards.
        if self.hibernating.is_some()
            || self.recovery_dir.is_none()
            || self.lifecycle == Lifecycle::Closing
            || self.departure.is_some()
            || self.close_dialog.is_some()
        {
            return;
        }
        let minutes = agent_settings::settings(cx).general.rest_after_minutes;
        let sleepers = self.idle_sleepers(cx);
        let chosen = memory::next_idle(
            minutes.map(|minutes| Duration::from_secs(u64::from(minutes) * 60)),
            self.hidden_since,
            Instant::now(),
            self.last_unload.map(|at| at.elapsed()),
            &sleepers,
        );
        if let (Some(id), Some(minutes)) = (chosen, minutes) {
            self.rest_idle(id, minutes, window, cx);
        }
    }

    // Workbooks being waited for or awaiting approval never rest, nor any that is busy. The one
    // on screen is offered only when it can leave the screen without disturbing the user: a
    // cell or formula being typed, or a dialog, keeps it where it is.
    fn idle_sleepers(&self, cx: &App) -> Vec<Sleeper> {
        let shown = self.documents.shown_id();
        let can_leave_screen = self.documents.wanted().is_none()
            && !self.typing_in_a_field(cx)
            && !self.prompts.blocks_input()
            && self.format_dialog.is_none()
            && self.chart.is_none();
        let wanted = self.documents.wanted();
        let awaiting_approval = self.awaiting_approval();
        self.documents
            .iter()
            .filter(|document| {
                Some(document.id) != wanted
                    && Some(document.id) != awaiting_approval
                    && document.can_dump()
                    && (Some(document.id) != shown || can_leave_screen)
            })
            .map(|document| Sleeper {
                id: document.id,
                last_used: document.last_used,
                on_screen: Some(document.id) == shown,
            })
            .collect()
    }

    fn rest_idle(
        &mut self,
        id: WorkbookId,
        minutes: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.active_id() == Some(id) {
            self.park_active(window, cx);
            if self.documents.step_aside_active().is_none() {
                return;
            }
            self.present_active(window, cx);
        }
        self.begin_hibernation(id, RestReason::Idle { minutes }, window, cx);
    }

    // The window is shown again: the workbook that rested with it comes back, loading if it
    // has already left memory.
    pub(super) fn wake_stepped_aside(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.documents.take_stepped_aside() {
            self.switch_to(id, window, cx);
        }
    }

    fn rest(
        &mut self,
        id: WorkbookId,
        resting: Resting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match resting {
            Resting::Unload => self.unload_workbook(id, cx),
            Resting::Hibernate => {
                self.begin_hibernation(id, RestReason::MemoryPressure, window, cx)
            }
        }
    }

    fn unload_workbook(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(document) = self.documents.get(id) else {
            return;
        };
        let (name, generation) = (document.name(), document.generation());
        if self.documents.unload(id) {
            self.previews.forget(generation);
            self.drop_retired(cx);
            self.last_unload = Some(Instant::now());
            self.probe_links(cx);
            self.notify(Severity::Info, t!("notice.unloaded", name = name), cx);
        }
    }

    fn drop_retired(&mut self, cx: &mut Context<Self>) {
        let retired = self.documents.take_retired();
        if !retired.is_empty() {
            cx.background_executor()
                .spawn(async move { drop(retired) })
                .detach();
        }
    }

    fn begin_hibernation(
        &mut self,
        id: WorkbookId,
        reason: RestReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let Some(Hibernating {
            generation,
            holds_unsaved_work,
            edits,
            workbook,
        }) = self.documents.begin_hibernation(id)
        else {
            return;
        };
        self.hibernating = Some(id);
        self.last_unload = Some(Instant::now());
        let dump = recovery::dump_file(&directory, id);
        let recovery_copy = recovery::document_file(&directory, id);
        cx.spawn_in(window, async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn({
                    let dump = dump.clone();
                    async move { document::write_hibernation(&workbook, &dump) }
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.hibernating = None;
                match written {
                    Ok(kept) => {
                        let dumped = Dumped {
                            generation,
                            edits,
                            recovery: holds_unsaved_work.then_some(recovery_copy),
                            dump: dump.clone(),
                            kept,
                        };
                        this.hibernation_written(id, dumped, reason, cx);
                    }
                    Err(error) if unloads_when_refused(reason, holds_unsaved_work) => {
                        tracing::warn!(%error, "idle dump refused, unloading the saved workbook");
                        this.documents.abandon_hibernation(id, generation, false);
                        this.unload_workbook(id, cx);
                    }
                    Err(error) => {
                        this.documents.abandon_hibernation(id, generation, true);
                        let name = this.documents.get(id).map(|document| document.name());
                        if let Some(name) = name {
                            let text = t!("notice.hibernation_failed", name = name, error = error);
                            this.notify(Severity::Warning, text, cx);
                        }
                    }
                }
                this.settle_save_after_hibernation(id, generation, window, cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while a workbook was hibernating");
            }
        })
        .detach();
    }

    // A Save pressed while the workbook was being written waits for the job, and the job is
    // over now whether the workbook left memory or not.
    fn settle_save_after_hibernation(
        &mut self,
        id: WorkbookId,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.get_mut(id) else {
            return;
        };
        if document.settle_save(generation, JobEnd::Autosaved) == AfterJob::SaveAgain {
            self.save_again(id, window, cx);
        }
    }

    // The dump is on disk and was checked. If the workbook changed or came back on screen in
    // the meantime the dump is stale, and the workbook stays in memory.
    fn hibernation_written(
        &mut self,
        id: WorkbookId,
        dumped: Dumped,
        reason: RestReason,
        cx: &mut Context<Self>,
    ) {
        let dump = dumped.dump.clone();
        let named = self
            .documents
            .get(id)
            .map(|document| (document.name(), document.generation()));
        if self.documents.finish_hibernation(id, dumped)
            && let Some((name, generation)) = named
        {
            self.previews.forget(generation);
            self.drop_retired(cx);
            self.probe_links(cx);
            self.persist_session(cx);
            let text = match reason {
                RestReason::MemoryPressure => t!("notice.hibernated", name = name),
                RestReason::Idle { minutes } => {
                    t!("notice.rested_idle", name = name, minutes = minutes)
                }
            };
            self.notify(Severity::Info, text, cx);
            return;
        }
        cx.background_executor()
            .spawn(async move { recovery::remove(&dump) })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{RestReason, unloads_when_refused};
    use std::path::PathBuf;

    use zenkai_engine::Workbook;

    use crate::documents::Documents;

    const IDLE: RestReason = RestReason::Idle { minutes: 15 };

    #[test]
    fn only_a_saved_workbook_resting_idle_falls_back_to_unloading() {
        assert!(unloads_when_refused(IDLE, false));
        assert!(!unloads_when_refused(IDLE, true));
        assert!(!unloads_when_refused(RestReason::MemoryPressure, false));
    }

    #[test]
    fn a_saved_workbook_whose_dump_was_refused_can_be_unloaded() {
        let book = || Workbook::new_empty().unwrap();
        let mut documents = Documents::new(book());
        documents.active_mut().unwrap().dirty = true;
        let id = documents.open(book(), Some(PathBuf::from("a.xlsx")), Vec::new());
        documents.open(book(), Some(PathBuf::from("b.xlsx")), Vec::new());
        let claim = documents.begin_hibernation(id).unwrap();
        assert!(!documents.unload(id), "the dump is still being written");
        documents.abandon_hibernation(id, claim.generation, false);
        assert!(documents.unload(id));
        assert!(documents.get(id).is_none());
    }
}
