use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::autosave::{self, Due};
use crate::files;
use crate::job_pill::LongJob;
use crate::recovery;
use crate::text_disk::Writing;
use crate::text_document::markdown_target;
use crate::text_file::{self, DiskChange, Format, Stamp};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Guard {
    // The file being saved over must still be the one that was read.
    InPlace,
    Chosen,
}

enum Written {
    Saved(Option<Stamp>),
    DiskChanged,
    Failed(String),
}

fn write_checked(
    path: &Path,
    text: &str,
    format: Format,
    known: Option<Stamp>,
    guard: Guard,
) -> Written {
    if guard == Guard::InPlace
        && text_file::disk_change(known, text_file::stamp_of(path)) == DiskChange::Changed
    {
        return Written::DiskChanged;
    }
    match text_file::write(path, text, format) {
        Ok(stamp) => Written::Saved(stamp),
        Err(error) => Written::Failed(error),
    }
}

impl Workspace {
    pub(super) fn save_text(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.documents.text(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        if text.writing == Writing::Saving {
            self.notify(Severity::Warning, t!("notice.still_saving"), cx);
            self.save_ended(id, false, window, cx);
            return;
        }
        match text.path.clone() {
            Some(path) => self.write_text(id, path, Guard::InPlace, window, cx),
            None => self.save_text_as(id, window, cx),
        }
    }

    pub(super) fn save_text_as(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.documents.text(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        let directory = text
            .path
            .as_ref()
            .and_then(|path| path.parent().map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let suggested = text.name();
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let path = match chosen.await {
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
            let update = this.update_in(cx, |this, window, cx| match path {
                Some(path) => this.confirm_text_target(id, markdown_target(&path), window, cx),
                None => this.save_ended(id, false, window, cx),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during save dialog");
            }
        })
        .detach();
    }

    fn confirm_text_target(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.documents.text(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        let same = text
            .path
            .as_ref()
            .is_some_and(|source| files::same_file(source, &path, files::Unresolved::ComparePaths));
        if same {
            self.write_text(id, path, Guard::InPlace, window, cx);
            return;
        }
        if !path.exists() {
            self.write_text(id, path, Guard::Chosen, window, cx);
            return;
        }
        let answer = self.ask(
            PromptLevel::Critical,
            t!("save.replace_existing_title"),
            Some(&t!("markdown.save.replace_detail", path = path.display())),
            &[t!("button.cancel"), t!("button.replace")],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| {
                if choice == Some(1) {
                    this.write_text(id, path, Guard::Chosen, window, cx)
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

    fn confirm_changed_text(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .documents
            .text(id)
            .map(|text| text.name())
            .unwrap_or_default();
        let answer = self.ask(
            PromptLevel::Critical,
            &t!("markdown.save.changed_title", name = name),
            Some(t!("markdown.save.changed_detail")),
            &[
                t!("button.replace"),
                t!("button.save_as"),
                t!("button.cancel"),
            ],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            let update = this.update_in(cx, |this, window, cx| match choice {
                Some(0) => this.write_text(id, path, Guard::Chosen, window, cx),
                Some(1) => this.save_text_as(id, window, cx),
                _ => this.save_ended(id, false, window, cx),
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during changed file prompt");
            }
        })
        .detach();
    }

    fn write_text(
        &mut self,
        id: WorkbookId,
        path: PathBuf,
        guard: Guard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.text_panes.get(&id).map(|entry| entry.pane.clone()) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        let content = pane.read(cx).text(cx).to_string();
        let Some(text) = self.documents.text_mut(id) else {
            self.save_ended(id, false, window, cx);
            return;
        };
        text.writing = Writing::Saving;
        let (edits, format, known) = (text.edit_count(), text.format, text.stamp);
        self.busy = Some(t!("busy.saving").into());
        self.start_long_job(LongJob::Saving, cx);
        cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move { write_checked(&target, &content, format, known, guard) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(t!("busy.saving"), cx);
                this.end_long_job(LongJob::Saving, cx);
                let Some(text) = this.documents.text_mut(id) else {
                    this.save_ended(id, false, window, cx);
                    return;
                };
                text.writing = Writing::Idle;
                match outcome {
                    Written::Saved(stamp) => {
                        text.saved(edits, stamp);
                        text.path = Some(path.clone());
                        let fully_saved = !text.dirty;
                        this.after_text_saved(id, &path, window, cx);
                        this.save_ended(id, fully_saved, window, cx);
                    }
                    Written::DiskChanged => this.confirm_changed_text(id, path, window, cx),
                    Written::Failed(error) => {
                        this.notify(
                            Severity::Error,
                            t!("notice.save_failed_pick_another", error = error),
                            cx,
                        );
                        let shown = this.documents.shown_id() == Some(id);
                        if guard == Guard::InPlace && shown {
                            this.save_text_as(id, window, cx);
                        } else {
                            this.save_ended(id, false, window, cx);
                        }
                    }
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during save");
            }
        })
        .detach();
    }

    fn after_text_saved(
        &mut self,
        id: WorkbookId,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_recent(path, cx);
        if let (Some(directory), Some(text)) =
            (self.recovery_dir.clone(), self.documents.text_mut(id))
            && !text.needs_recovery()
        {
            text.recovery_removed();
            let copy = recovery::text_file(&directory, id);
            cx.background_executor()
                .spawn(async move { recovery::remove(&copy) })
                .detach();
        }
        self.sync_text(id, cx);
        window.set_window_title(&self.window_title());
        self.notify(Severity::Info, t!("notice.saved"), cx);
        self.persist_session(cx);
    }

    pub(super) fn autosave_texts(
        &mut self,
        every: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<WorkbookId> = self.documents.texts().map(|text| text.id).collect();
        for id in ids {
            self.autosave_text(id, every, window, cx);
        }
    }

    fn autosave_text(
        &mut self,
        id: WorkbookId,
        every: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let Some(pane) = self.text_panes.get(&id).map(|entry| entry.pane.clone()) else {
            return;
        };
        let Some(text) = self.documents.text_mut(id) else {
            return;
        };
        let due = autosave::due(
            text.since_last_edit(),
            text.since_last_autosave_attempt(),
            text.recovery_stale_for(),
            every,
        );
        if due == Due::No || text.writing != Writing::Idle {
            return;
        }
        let target = recovery::text_file(&directory, id);
        text.mark_autosave_attempt();
        if !text.needs_recovery() {
            text.recovery_removed();
            cx.background_executor()
                .spawn(async move { recovery::remove(&target) })
                .detach();
            return;
        }
        if text.recovery_is_current() {
            return;
        }
        text.writing = Writing::Autosaving;
        let (edits, format, had_copy) = (text.edit_count(), text.format, text.has_recovery_copy());
        let content = pane.read(cx).text(cx).to_string();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { write_recovery(&target, &content, format) })
                .await;
            let update = this.update_in(cx, |this, _, cx| {
                let Some(text) = this.documents.text_mut(id) else {
                    return;
                };
                text.writing = Writing::Idle;
                match result {
                    Ok(()) => {
                        text.recovery_written(edits);
                        if !had_copy {
                            this.persist_session(cx);
                        }
                    }
                    Err(error) => this.notify(
                        Severity::Warning,
                        t!("notice.autosave_failed", error = error),
                        cx,
                    ),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during autosave");
            }
        })
        .detach();
    }
}

pub(super) fn write_recovery(target: &Path, text: &str, format: Format) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| t!("file.write_failed", path = parent.display(), error = error))?;
    }
    text_file::write(target, text, format).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{Guard, Written, write_checked, write_recovery};
    use crate::text_file::{self, Format, LineEnding};

    #[test]
    fn saving_in_place_over_the_file_that_was_read_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "old\r\n").unwrap();
        let (decoded, stamp) = text_file::read(&path).unwrap();
        let outcome = write_checked(&path, "new\n", decoded.format, stamp, Guard::InPlace);
        assert!(matches!(outcome, Written::Saved(Some(_))));
        assert_eq!(std::fs::read(&path).unwrap(), b"new\r\n");
    }

    #[test]
    fn saving_in_place_over_a_file_another_program_changed_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let (_, stamp) = text_file::read(&path).unwrap();
        std::fs::write(&path, "theirs, and longer").unwrap();
        let outcome = write_checked(&path, "mine", Format::default(), stamp, Guard::InPlace);
        assert!(matches!(outcome, Written::DiskChanged));
        assert_eq!(std::fs::read(&path).unwrap(), b"theirs, and longer");
    }

    #[test]
    fn a_chosen_replacement_writes_over_a_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let (_, stamp) = text_file::read(&path).unwrap();
        std::fs::write(&path, "theirs, and longer").unwrap();
        let outcome = write_checked(&path, "mine", Format::default(), stamp, Guard::Chosen);
        assert!(matches!(outcome, Written::Saved(_)));
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");
    }

    #[test]
    fn saving_a_deleted_file_writes_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let (_, stamp) = text_file::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let outcome = write_checked(&path, "kept", Format::default(), stamp, Guard::InPlace);
        assert!(matches!(outcome, Written::Saved(_)));
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
    }

    #[test]
    fn a_recovery_copy_creates_its_folder_and_keeps_the_format() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("recovery").join("autosave-1-2-3.md");
        let format = Format {
            line_ending: LineEnding::CrLf,
            bom: true,
        };
        write_recovery(&target, "a\nb\n", format).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"\xEF\xBB\xBFa\r\nb\r\n");
        let (decoded, _) = text_file::read(&target).unwrap();
        assert_eq!(decoded.text, "a\nb\n");
        assert_eq!(decoded.format, format);
    }
}
