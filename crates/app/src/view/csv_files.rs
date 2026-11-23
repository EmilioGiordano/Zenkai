use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::*;
use zenkai_engine::EngineError;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Overwrite, Severity, Workspace, file_job_refusal};
use crate::csv_export_size::ExportSize;
use crate::csv_options::{CsvBinding, Losses};
use crate::csv_save::{CsvSave, CsvSaveRequest, LargeWrite, save_csv_file};
use crate::document::{ChangedFile, Document, FileJob};
use crate::failure::{offer_another_location, save_failure};
use crate::job_pill::LongJob;
use crate::pending_save::{AfterJob, JobEnd};
use crate::sidebar_item::human_size;

// Windows makes the first answer the default button, so Cancel goes first.
const KEEP_CSV: usize = 1;
const SAVE_AS_XLSX: usize = 2;
const LARGE_SAVE: usize = 1;

fn loss_answers() -> [PromptButton; 3] {
    [
        PromptButton::cancel(t!("button.cancel")),
        PromptButton::new(t!("button.keep_csv")),
        PromptButton::new(t!("button.save_as_xlsx")),
    ]
}

fn large_answers() -> [PromptButton; 2] {
    [
        PromptButton::cancel(t!("button.cancel")),
        PromptButton::new(t!("button.save")),
    ]
}

fn loss_list(loss: Losses) -> String {
    let named = [
        (loss.sheets, t!("save.csv_loss.sheets")),
        (loss.formulas, t!("save.csv_loss.formulas")),
        (loss.formatting, t!("save.csv_loss.formatting")),
        (loss.number_formats, t!("save.csv_loss.number_formats")),
        (loss.moved_cells, t!("save.csv_loss.moved_cells")),
    ];
    let lost: Vec<&str> = named
        .into_iter()
        .filter_map(|(lost, label)| lost.then_some(label))
        .collect();
    lost.join(", ")
}

struct CsvSaveRun {
    id: WorkbookId,
    path: PathBuf,
    generation: u64,
    edits_at_start: u64,
    sheets: usize,
    // Carried through the format prompt, so a Replace already chosen is not asked again.
    changed: ChangedFile,
}

impl Workspace {
    pub(super) fn save_csv(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        binding: CsvBinding,
        changed: ChangedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let large = self
            .large_csv_confirmed
            .get(&id)
            .map_or(LargeWrite::Ask, |bytes| LargeWrite::Confirmed(*bytes));
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
        let run = CsvSaveRun {
            id,
            path: path.clone(),
            generation: document.generation(),
            edits_at_start: document.edit_count(),
            sheets: document.sheets.len(),
            changed,
        };
        let request = CsvSaveRequest {
            path,
            binding,
            sheets: run.sheets,
            sheet: document.sheet,
            changed,
            large,
            guard: document.file_guard(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.export_cancels.insert(id, Arc::clone(&cancel));
        self.busy = Some(t!("busy.saving").into());
        self.start_long_job(LongJob::Saving, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { save_csv_file(&shared, &request, &cancel) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.export_cancels.remove(&id);
                this.clear_busy(t!("busy.saving"), cx);
                this.end_long_job(LongJob::Saving, cx);
                this.csv_save_ended(run, result, window, cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during a CSV save");
            }
        })
        .detach();
    }

    fn csv_save_ended(
        &mut self,
        run: CsvSaveRun,
        result: Result<CsvSave, EngineError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let CsvSaveRun {
            id,
            path,
            generation,
            edits_at_start,
            sheets,
            changed,
        } = run;
        let shown = self.documents.active_id() == Some(id);
        let mut follow_up = AfterJob::Nothing;
        let current = self.documents.get_mut(id).is_some_and(|document| {
            document.end_file_job(generation);
            let current = document.is_current(generation);
            if current {
                let end = if matches!(result, Ok(CsvSave::Written(..))) {
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
        let Some(document) = self.documents.get_mut(id).filter(|_| current) else {
            if let Err(error) = result {
                self.notify_failure(save_failure(&path, &error), cx);
            }
            self.save_ended(id, false, window, cx);
            return;
        };
        let fully_saved = match result {
            Ok(CsvSave::Written(seen, options, consumed)) => {
                document.saved_over_file(Vec::new());
                document.guard_file(seen);
                let edited_meanwhile = document.edit_count() != edits_at_start;
                if let Some(binding) = document.csv.as_mut() {
                    binding.options.get_or_insert(options);
                    binding.saved(consumed);
                }
                document.dirty = edited_meanwhile;
                if shown {
                    window.set_window_title(&self.documents.shown_title());
                }
                if sheets > 1 {
                    let text = t!("notice.saved_active_sheet_only", path = path.display());
                    self.notify(Severity::Warning, text, cx);
                } else {
                    self.notify(Severity::Info, t!("notice.saved"), cx);
                }
                !edited_meanwhile
            }
            Ok(CsvSave::WouldLose(found)) => {
                self.flush_edits(id, cx);
                self.confirm_csv_loss(id, path, found, changed, window, cx);
                return;
            }
            Ok(refused @ (CsvSave::LossyFile | CsvSave::ShapeUnknown)) => {
                self.flush_edits(id, cx);
                let notice = match refused {
                    CsvSave::LossyFile => t!("save.csv_lossy"),
                    _ => t!("save.csv_shape_unknown"),
                };
                self.notify(Severity::Warning, notice, cx);
                if shown {
                    self.save_as_dialog(id, window, cx);
                } else {
                    self.save_ended(id, false, window, cx);
                }
                return;
            }
            Ok(CsvSave::TooLarge(size)) => {
                self.flush_edits(id, cx);
                self.confirm_large_csv(id, path, size, changed, window, cx);
                return;
            }
            Ok(CsvSave::FileChanged) => {
                self.flush_edits(id, cx);
                self.confirm_changed_file(id, path, Overwrite::InPlace, window, cx);
                return;
            }
            Err(EngineError::Cancelled) => {
                self.notify(Severity::Info, t!("notice.save_cancelled"), cx);
                self.flush_edits(id, cx);
                self.save_ended(id, false, window, cx);
                return;
            }
            Err(error) => {
                self.notify_failure(offer_another_location(save_failure(&path, &error)), cx);
                self.flush_edits(id, cx);
                if shown {
                    self.save_as_dialog(id, window, cx);
                } else {
                    self.save_ended(id, false, window, cx);
                }
                return;
            }
        };
        self.flush_edits(id, cx);
        self.save_ended(id, fully_saved, window, cx);
        if follow_up == AfterJob::SaveAgain {
            self.save_again(id, window, cx);
        }
    }

    fn confirm_large_csv(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        size: ExportSize,
        changed: ChangedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = t!(
            "save.large_detail",
            rows = size.rows,
            columns = size.columns,
            size = human_size(size.bytes),
            cells = size.cells
        );
        let answer = self.ask(
            PromptLevel::Warning,
            t!("save.large_title"),
            Some(&detail),
            &large_answers(),
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| {
                let confirmed = this
                    .documents
                    .get(id)
                    .and_then(|document| document.csv.clone())
                    .filter(|_| choice == Some(LARGE_SAVE));
                match confirmed {
                    Some(binding) => {
                        this.large_csv_confirmed.insert(id, size.bytes);
                        this.save_csv(id, path, binding, changed, window, cx);
                    }
                    None => this.save_ended(id, false, window, cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during the large save prompt");
            }
        })
        .detach();
    }

    fn confirm_csv_loss(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        found: Losses,
        changed: ChangedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .documents
            .get(id)
            .map(Document::name)
            .unwrap_or_default();
        let accepted = self
            .documents
            .get(id)
            .and_then(|document| document.csv.clone())
            .map_or_else(Losses::default, |binding| binding.accepted);
        let answer = self.ask(
            PromptLevel::Warning,
            &t!("save.csv_loses_title", name = name),
            Some(&t!(
                "save.csv_loses_detail",
                list = loss_list(found.beyond(accepted))
            )),
            &loss_answers(),
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| match choice {
                Some(KEEP_CSV) => {
                    let kept = this
                        .documents
                        .get_mut(id)
                        .and_then(|document| document.csv.as_mut())
                        .map(|binding| {
                            binding.accepted = binding.accepted.with(found);
                            binding.clone()
                        });
                    match kept {
                        Some(binding) => this.save_csv(id, path, binding, changed, window, cx),
                        None => this.save_ended(id, false, window, cx),
                    }
                }
                Some(SAVE_AS_XLSX) => this.save_as_dialog(id, window, cx),
                _ => this.save_ended(id, false, window, cx),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during the CSV format prompt");
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use zenkai_i18n::t;

    use super::{KEEP_CSV, Losses, SAVE_AS_XLSX, loss_answers, loss_list};

    #[test]
    fn the_format_prompt_defaults_to_cancel_and_offers_both_ways_out() {
        let answers = loss_answers();
        assert!(answers[0].is_cancel());
        assert_eq!(answers.iter().filter(|a| a.is_cancel()).count(), 1);
        assert_eq!(answers[KEEP_CSV].label().as_ref(), t!("button.keep_csv"));
        assert_eq!(
            answers[SAVE_AS_XLSX].label().as_ref(),
            t!("button.save_as_xlsx")
        );
    }

    #[test]
    fn the_prompt_names_only_what_would_be_lost() {
        let formulas_only = Losses {
            formulas: true,
            ..Losses::default()
        };
        assert_eq!(loss_list(formulas_only), t!("save.csv_loss.formulas"));
        let all = Losses {
            sheets: true,
            formulas: true,
            formatting: true,
            number_formats: true,
            moved_cells: true,
        };
        let list = loss_list(all);
        for label in [
            t!("save.csv_loss.sheets"),
            t!("save.csv_loss.formulas"),
            t!("save.csv_loss.formatting"),
            t!("save.csv_loss.number_formats"),
            t!("save.csv_loss.moved_cells"),
        ] {
            assert!(list.contains(label), "{list}");
        }
    }
}
