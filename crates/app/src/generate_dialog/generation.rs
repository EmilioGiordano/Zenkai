use zenkai_datagen::DatagenError;

use super::draft::{self, Block, WriteIssue};
use super::{DEBOUNCE, DialogEvent, GenerateDialog, Ready, Write};
use gpui_kit::*;
use zenkai_i18n::t;

impl GenerateDialog {
    // Generation runs on the background executor; a newer request makes older results stale.
    pub(super) fn schedule(&mut self, cx: &mut Context<Self>) {
        self.request += 1;
        let request = self.request;
        if self.draft.has_local_issue() {
            self.working = false;
            return;
        }
        self.working = true;
        let spec = self.draft.spec();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let current = this.update(cx, |this, _| this.request == request);
            if !matches!(current, Ok(true)) {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move { zenkai_datagen::generate(&spec) })
                .await;
            let update = this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.working = false;
                this.ready = Some(Ready { request, result });
                if std::mem::take(&mut this.submit_when_ready) {
                    this.generate(cx);
                }
                cx.notify();
            });
            if let Err(error) = update {
                tracing::debug!(%error, "generate dialog closed during generation");
            }
        })
        .detach();
    }

    pub(super) fn current_ready(&self) -> Option<&Ready> {
        self.ready
            .as_ref()
            .filter(|ready| ready.request == self.request)
    }

    pub(super) fn generation_error(&self) -> Option<&DatagenError> {
        self.current_ready()
            .and_then(|ready| ready.result.as_ref().err())
    }

    pub(super) fn column_error(&self, index: usize) -> Option<String> {
        if let Some(local) = self
            .draft
            .columns()
            .get(index)
            .and_then(|c| c.local_issue())
        {
            return Some(local);
        }
        match self.generation_error() {
            Some(DatagenError::Column {
                position, problem, ..
            }) if *position == index => Some(problem.to_string()),
            _ => None,
        }
    }

    pub(super) fn footer_error(&self) -> Option<String> {
        if let Some(issue) = self.draft.field_issue() {
            return Some(issue.to_string());
        }
        if let Some(issue) = &self.problem {
            return Some(issue.to_string());
        }
        match self.generation_error() {
            Some(DatagenError::Column { .. }) | None => {
                self.draft.write_issue().map(|i| i.to_string())
            }
            Some(other) => Some(other.to_string()),
        }
    }

    pub(super) fn can_generate(&self) -> bool {
        !self.draft.has_local_issue()
            && self.draft.write_issue().is_none()
            && self.generation_error().is_none()
    }

    pub(crate) fn generate(&mut self, cx: &mut Context<Self>) {
        if !self.can_generate() {
            return;
        }
        let Some(ready) = self.current_ready() else {
            self.submit_when_ready = true;
            cx.notify();
            return;
        };
        if ready.result.is_err() {
            return;
        }
        let Some(Ready {
            result: Ok(table), ..
        }) = self.ready.take()
        else {
            return;
        };
        let rows = table.len() as u32;
        let notice = t!(
            "gen.notice_generated",
            rows = draft::count_label(rows, draft::Counted::Row)
        );
        let block = self.draft.block(Some(table));
        self.submit(block, notice, cx);
    }

    pub(super) fn submit(
        &mut self,
        block: Result<Block, WriteIssue>,
        notice: String,
        cx: &mut Context<Self>,
    ) {
        match block {
            Ok(block) => {
                self.outgoing = Some(Write {
                    target: self.target,
                    block,
                    notice,
                });
                cx.emit(DialogEvent::Write);
            }
            Err(issue) => {
                self.problem = Some(issue);
                cx.notify();
            }
        }
    }

    pub fn take_write(&mut self) -> Option<Write> {
        self.outgoing.take()
    }

    pub(crate) fn save_headers(&mut self, cx: &mut Context<Self>) {
        let changed = self.draft.changed_headers();
        if changed == 0 || self.draft.has_local_issue() {
            return;
        }
        let notice = t!(
            "gen.notice_headers_saved",
            headers = draft::count_label(changed as u32, draft::Counted::Header)
        );
        let block = self.draft.block(None);
        self.submit(block, notice, cx);
    }
}
