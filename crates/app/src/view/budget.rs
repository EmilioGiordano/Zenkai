use std::time::{Duration, Instant};

use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::document;
use crate::documents::{Dumped, Hibernating};
use crate::memory::{self, Candidate, Resting};
use crate::recovery;

const SAMPLE_EVERY: Duration = Duration::from_secs(1);

impl Workspace {
    pub(super) fn start_memory_sampler(&mut self, cx: &mut Context<Self>) {
        self.memory_sampler = Some(cx.spawn(async move |this, cx| {
            loop {
                if let Some(memory_mb) = memory::current_committed_mb() {
                    let update = this.update(cx, |this, cx| {
                        this.memory_mb = memory_mb;
                        if this.sidebar.visible || this.diagnostics {
                            cx.notify();
                        }
                        this.enforce_memory_budget(cx);
                        this.rest_while_away(cx);
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

    fn enforce_memory_budget(&mut self, cx: &mut Context<Self>) {
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
            self.rest(id, resting, cx);
        }
    }

    // The window hidden in the tray for a long time with workbooks nobody touched since.
    fn rest_while_away(&mut self, cx: &mut Context<Self>) {
        if crate::tray::hidden_to_tray(cx) {
            self.hidden_since.get_or_insert_with(Instant::now);
        } else {
            self.hidden_since = None;
        }
        let hidden_for = self.hidden_since.map(|since| since.elapsed());
        let since_last = self.last_unload.map(|at| at.elapsed());
        let candidates = self.rest_candidates();
        if let Some((id, resting)) =
            memory::next_when_away(hidden_for, Instant::now(), since_last, &candidates)
        {
            self.rest(id, resting, cx);
        }
    }

    fn rest(&mut self, id: WorkbookId, resting: Resting, cx: &mut Context<Self>) {
        match resting {
            Resting::Unload => self.unload_workbook(id, cx),
            Resting::Hibernate => self.begin_hibernation(id, cx),
        }
    }

    fn unload_workbook(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(document) = self.documents.get(id) else {
            return;
        };
        let (name, generation) = (document.name(), document.generation());
        if self.documents.unload(id) {
            self.previews.forget(generation);
            self.last_unload = Some(Instant::now());
            self.probe_links(cx);
            self.notify(Severity::Info, t!("notice.unloaded", name = name), cx);
        }
    }

    fn begin_hibernation(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let Some(Hibernating {
            generation,
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
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn({
                    let dump = dump.clone();
                    async move { document::write_hibernation(&workbook, &dump) }
                })
                .await;
            let update = this.update(cx, |this, cx| {
                this.hibernating = None;
                match written {
                    Ok(kept) => {
                        let dumped = Dumped {
                            generation,
                            edits,
                            recovery: recovery_copy,
                            dump: dump.clone(),
                            kept,
                        };
                        this.hibernation_written(id, dumped, cx);
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
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while a workbook was hibernating");
            }
        })
        .detach();
    }

    // The dump is on disk and was checked. If the workbook changed or came back on screen in
    // the meantime the dump is stale, and the workbook stays in memory.
    fn hibernation_written(&mut self, id: WorkbookId, dumped: Dumped, cx: &mut Context<Self>) {
        let dump = dumped.dump.clone();
        let named = self
            .documents
            .get(id)
            .map(|document| (document.name(), document.generation()));
        if self.documents.finish_hibernation(id, dumped)
            && let Some((name, generation)) = named
        {
            self.previews.forget(generation);
            self.probe_links(cx);
            self.persist_session(cx);
            self.notify(Severity::Info, t!("notice.hibernated", name = name), cx);
            return;
        }
        cx.background_executor()
            .spawn(async move { recovery::remove(&dump) })
            .detach();
    }
}
