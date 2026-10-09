use std::time::{Duration, Instant};

use gpui_kit::*;

use super::{Severity, Workspace};
use crate::memory::{self, Candidate};

const SAMPLE_EVERY: Duration = Duration::from_secs(1);

impl Workspace {
    pub(super) fn start_memory_sampler(&mut self, cx: &mut Context<Self>) {
        self.memory_sampler = Some(cx.spawn(async move |this, cx| {
            loop {
                let memory =
                    memory_stats::memory_stats().map_or(0, |m| m.physical_mem / 1024 / 1024);
                let update = this.update(cx, |this, cx| {
                    this.memory_mb = u64::try_from(memory).unwrap_or(u64::MAX);
                    if this.sidebar.visible || this.diagnostics {
                        cx.notify();
                    }
                    this.enforce_memory_budget(cx);
                });
                if update.is_err() {
                    break;
                }
                cx.background_executor().timer(SAMPLE_EVERY).await;
            }
        }));
    }

    // Workbooks with unsaved work are never unloaded.
    fn enforce_memory_budget(&mut self, cx: &mut Context<Self>) {
        if self.memory_mb <= self.memory_budget_mb {
            return;
        }
        let active = self.documents.active_id();
        let wanted = self.documents.wanted();
        let awaiting_approval = self.awaiting_approval();
        let candidates: Vec<Candidate> = self
            .documents
            .iter()
            .map(|document| Candidate {
                id: document.id,
                last_used: document.last_used,
                idle: document.id != active
                    && Some(document.id) != wanted
                    && Some(document.id) != awaiting_approval
                    && document.can_unload(),
            })
            .collect();
        let since_last = self.last_unload.map(|at| at.elapsed());
        let Some(id) = memory::next_to_unload(
            self.memory_mb,
            self.memory_budget_mb,
            since_last,
            &candidates,
        ) else {
            return;
        };
        let Some(document) = self.documents.get(id) else {
            return;
        };
        let (name, generation) = (document.name(), document.generation());
        if self.documents.unload(id) {
            self.previews.forget(generation);
            self.last_unload = Some(Instant::now());
            self.probe_links(cx);
            self.notify(
                Severity::Info,
                format!("Unloaded {name} to free memory, undo history included. It loads again when you open it."),
                cx,
            );
        }
    }
}
