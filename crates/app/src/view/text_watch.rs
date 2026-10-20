use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::lifecycle::Lifecycle;
use super::{Severity, Workspace};
use crate::files;
use crate::recovery;
use crate::text_document::{DiskNotice, Writing};
use crate::text_file::{self, DiskChange, Reaction, ReadError, Stamp};

const WATCH_EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct TextWatch {
    last: Option<Instant>,
    running: bool,
}

struct Probe {
    id: WorkbookId,
    epoch: u64,
    path: PathBuf,
}

#[derive(Clone, Copy)]
enum Reload {
    // Nothing is asked: it happens only to a text that has no edits of its own.
    Quiet { edits: u64, seen: Option<Stamp> },
    Chosen,
}

impl Workspace {
    // Asks the disk, off the UI thread, whether the files of the open texts changed. Shares
    // and devices are not probed: asking about them can send credentials to a host.
    pub(super) fn watch_texts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = self
            .text_watch
            .last
            .is_some_and(|last| last.elapsed() < WATCH_EVERY);
        if self.text_watch.running || waiting || self.lifecycle == Lifecycle::Closing {
            return;
        }
        let probes: Vec<Probe> = self
            .documents
            .texts()
            .filter(|text| text.writing == Writing::Idle)
            .filter_map(|text| {
                let path = text.path.clone()?;
                (!files::is_remote_or_device(&path)).then_some(Probe {
                    id: text.id,
                    epoch: text.stamp_epoch,
                    path,
                })
            })
            .collect();
        if probes.is_empty() {
            return;
        }
        self.text_watch.running = true;
        self.text_watch.last = Some(Instant::now());
        cx.spawn_in(window, async move |this, cx| {
            let seen = cx
                .background_executor()
                .spawn(async move {
                    probes
                        .into_iter()
                        .map(|probe| (probe.id, probe.epoch, text_file::stamp_of(&probe.path)))
                        .collect::<Vec<_>>()
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.text_watch.running = false;
                for (id, epoch, now) in seen {
                    this.apply_disk_state(id, epoch, now, window, cx);
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while watching files");
            }
        })
        .detach();
    }

    fn apply_disk_state(
        &mut self,
        id: WorkbookId,
        epoch: u64,
        now: Option<Stamp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.documents.text_mut(id) else {
            return;
        };
        if text.stamp_epoch != epoch || text.writing == Writing::Saving {
            return;
        }
        let change = match (text.notice, now) {
            (DiskNotice::Deleted, Some(_)) => DiskChange::Changed,
            _ => text_file::disk_change(text.stamp, now),
        };
        let edits = text.edit_count();
        if change == DiskChange::Changed && now.is_some() && text.reload_failed_at == now {
            return;
        }
        match text_file::react(change, text.dirty) {
            Reaction::Nothing => {}
            Reaction::ReloadQuietly => {
                self.reload_text_from_disk(id, Reload::Quiet { edits, seen: now }, window, cx)
            }
            Reaction::OfferReload => self.set_text_notice(id, DiskNotice::ChangedWithEdits, cx),
            Reaction::KeepAndMarkDeleted => self.set_text_notice(id, DiskNotice::Deleted, cx),
        }
    }

    fn set_text_notice(&mut self, id: WorkbookId, notice: DiskNotice, cx: &mut Context<Self>) {
        let Some(text) = self.documents.text_mut(id) else {
            return;
        };
        if text.notice == notice {
            return;
        }
        text.notice = notice;
        self.sync_text(id, cx);
    }

    // Edits are never thrown away by a keystroke: with unsaved ones, only the bar that says
    // the file changed offers to reload.
    pub(super) fn reload_shown_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.documents.shown_text() else {
            return;
        };
        let id = text.id;
        if text.dirty && text.notice != DiskNotice::ChangedWithEdits {
            self.notify(Severity::Warning, t!("markdown.notice.reload_blocked"), cx);
        } else {
            self.reload_text(id, window, cx);
        }
    }

    pub(super) fn keep_shown_text_changes(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.documents.shown_text() else {
            return;
        };
        let (id, offered) = (text.id, text.notice == DiskNotice::ChangedWithEdits);
        if offered {
            self.keep_local_text(id, cx);
        }
    }

    pub(super) fn reload_text(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_text_from_disk(id, Reload::Chosen, window, cx);
    }

    fn reload_text_from_disk(
        &mut self,
        id: WorkbookId,
        how: Reload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.documents.text(id).and_then(|text| text.path.clone()) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { text_file::read(&path) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.apply_reload(id, how, read, window, cx)
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during a reload");
            }
        })
        .detach();
    }

    fn apply_reload(
        &mut self,
        id: WorkbookId,
        how: Reload,
        read: Result<(text_file::Decoded, Option<Stamp>), ReadError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.documents.text(id) else {
            return;
        };
        if let Reload::Quiet { edits, .. } = how
            && (text.dirty || text.edit_count() != edits)
        {
            return;
        }
        let name = text.name();
        match read {
            Ok((decoded, stamp)) => {
                if let Some(entry) = self.text_panes.get(&id) {
                    entry
                        .pane
                        .update(cx, |pane, cx| pane.replace_text(&decoded.text, window, cx));
                }
                if let Some(text) = self.documents.text_mut(id) {
                    text.format = decoded.format;
                    text.reloaded(stamp);
                    text.recovery_removed();
                }
                if let Some(directory) = self.recovery_dir.clone() {
                    let copy = recovery::text_file(&directory, id);
                    cx.background_executor()
                        .spawn(async move { recovery::remove(&copy) })
                        .detach();
                }
                self.sync_text(id, cx);
                window.set_window_title(&self.window_title());
                self.notify(
                    Severity::Info,
                    t!("markdown.notice.reloaded", name = name),
                    cx,
                );
            }
            Err(ReadError::Missing) => self.set_text_notice(id, DiskNotice::Deleted, cx),
            Err(error) => {
                if let Reload::Quiet { seen, .. } = how
                    && let Some(text) = self.documents.text_mut(id)
                {
                    text.reload_failed_at = seen;
                }
                self.notify(Severity::Error, error.to_string(), cx);
            }
        }
    }

    // The edits stay and the file as it is now becomes the one they are saved over.
    pub(super) fn keep_local_text(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let Some(path) = self.documents.text(id).and_then(|text| text.path.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let now = cx
                .background_executor()
                .spawn(async move { text_file::stamp_of(&path) })
                .await;
            let update = this.update(cx, |this, cx| {
                if let Some(text) = this.documents.text_mut(id) {
                    text.set_stamp(now);
                    text.notice = if now.is_some() {
                        DiskNotice::None
                    } else {
                        DiskNotice::Deleted
                    };
                }
                this.sync_text(id, cx);
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while keeping local edits");
            }
        })
        .detach();
    }
}
