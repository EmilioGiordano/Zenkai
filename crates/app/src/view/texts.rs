use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::base::v_flex;
use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace, file_label};
use crate::documents::{Installed, Provenance, TextLoad};
use crate::entry::{Entry, Link, LinkStatus};
use crate::markdown_pane::{MarkdownPane, PaneEvent, PaneState, folder_of};
use crate::recovery;
use crate::text_disk::RecoveredFile;
use crate::text_document::{DocumentKind, ViewMode};
use crate::text_file::{self, Decoded, ReadError, Stamp, StampRecord};

pub(super) struct TextPane {
    pub(super) pane: Entity<MarkdownPane>,
    _events: Subscription,
}

pub(super) type TextPanes = HashMap<WorkbookId, TextPane>;

enum TextLink {
    Loaded {
        decoded: Decoded,
        stamp: Option<Stamp>,
        base: Option<StampRecord>,
        file_exists: bool,
        copy: Option<PathBuf>,
        recovery_lost: bool,
    },
    Missing,
    Failed(String),
}

// Runs on the background executor. The recovery copy, when there is one, holds the unsaved
// work and is read instead of the file.
fn read_text_link(link: &Link) -> TextLink {
    let mut recovery_lost = link.recovery_lost;
    if let Some(copy) = link.recovery.as_ref().filter(|copy| copy.exists()) {
        match text_file::read(copy) {
            Ok((decoded, _)) => {
                return TextLink::Loaded {
                    decoded,
                    stamp: link.path.as_deref().and_then(text_file::stamp_of),
                    base: link.base,
                    file_exists: link.path.as_deref().is_some_and(Path::exists),
                    copy: Some(copy.clone()),
                    recovery_lost: false,
                };
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the recovery copy of a text");
                recovery_lost = true;
            }
        }
    } else if link.recovery.is_some() {
        recovery_lost = true;
    }
    let Some(path) = &link.path else {
        return TextLink::Missing;
    };
    match text_file::read(path) {
        Ok((decoded, stamp)) => TextLink::Loaded {
            decoded,
            stamp,
            base: None,
            file_exists: true,
            copy: None,
            recovery_lost,
        },
        Err(ReadError::Missing) => TextLink::Missing,
        Err(error) => TextLink::Failed(error.to_string()),
    }
}

impl Workspace {
    pub(super) fn default_text_mode(&self, cx: &App) -> ViewMode {
        crate::agent_settings::settings(cx)
            .general
            .markdown_opens_in
            .into()
    }

    pub(super) fn shown_pane(&self) -> Option<&TextPane> {
        self.text_panes.get(&self.documents.shown_text()?.id)
    }

    pub(super) fn text_state(&self, id: WorkbookId) -> PaneState {
        self.documents
            .text(id)
            .map_or_else(PaneState::default, |text| PaneState {
                folder: folder_of(text.path.as_deref()),
                notice: text.notice,
            })
    }

    pub(super) fn sync_text(&mut self, id: WorkbookId, cx: &mut Context<Self>) {
        let state = self.text_state(id);
        if let Some(entry) = self.text_panes.get(&id) {
            entry.pane.update(cx, |pane, cx| pane.sync(state, cx));
        }
        cx.notify();
    }

    fn add_pane(
        &mut self,
        id: WorkbookId,
        text: &str,
        mode: ViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.text_state(id);
        let pane = cx.new(|cx| MarkdownPane::new(text, mode, state, window, cx));
        let events = cx.subscribe_in(
            &pane,
            window,
            move |this, _, event: &PaneEvent, window, cx| this.on_pane_event(id, event, window, cx),
        );
        self.text_panes.insert(
            id,
            TextPane {
                pane,
                _events: events,
            },
        );
    }

    fn on_pane_event(
        &mut self,
        id: WorkbookId,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Edited => {
                if let Some(text) = self.documents.text_mut(id) {
                    text.edited();
                }
                self.sync_text(id, cx);
                window.set_window_title(&self.window_title());
            }
            PaneEvent::Moved => cx.notify(),
            PaneEvent::Mode(mode) => {
                self.documents.set_mode(id, *mode);
                self.persist_session(cx);
            }
            PaneEvent::OpenLink(url) => self.open_document_link(id, url, window, cx),
            PaneEvent::Reload => self.reload_text(id, window, cx),
            PaneEvent::KeepMine => self.keep_local_text(id, cx),
        }
    }

    pub(super) fn render_text(
        &self,
        pane: Entity<MarkdownPane>,
        busy: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(self.render_held_settings(cx))
            .child(pane)
            .child(self.render_status(busy, cx))
    }

    pub(super) fn cycle_markdown_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.shown_pane() {
            entry
                .pane
                .update(cx, |pane, cx| pane.cycle_mode(window, cx));
        }
    }

    // A text comes on screen: what belongs to a workbook is put away, and the pane takes focus.
    pub(super) fn present_text(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stats = None;
        self.forget_find_results();
        window.set_window_title(&self.window_title());
        if let Some(entry) = self.text_panes.get(&id) {
            entry.pane.update(cx, |pane, cx| pane.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn open_text_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = self.documents.find_by_path(&path) {
            self.switch_to(open, window, cx);
            return;
        }
        self.busy = Some(t!("busy.opening", name = file_label(&path)).into());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { text_file::read(&task_path) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok((decoded, stamp)) => {
                        this.remember_recent(&path, cx);
                        if let Some(open) = this.documents.find_by_path(&path) {
                            this.switch_to(open, window, cx);
                            return;
                        }
                        this.install_opened_text(path, decoded, stamp, window, cx);
                    }
                    Err(ReadError::Missing) => this.notify(
                        Severity::Error,
                        t!("open.path_not_found", path = path.display()),
                        cx,
                    ),
                    Err(error) => this.notify(Severity::Error, error.to_string(), cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during open");
            }
        })
        .detach();
    }

    fn install_opened_text(
        &mut self,
        path: PathBuf,
        decoded: Decoded,
        stamp: Option<Stamp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = DocumentKind::of(&path);
        let mode = self.default_text_mode(cx);
        self.park_active(window, cx);
        let id = self
            .documents
            .open_text(Some(path), kind, mode, decoded.format, stamp);
        self.add_pane(id, &decoded.text, mode, window, cx);
        self.present_text(id, window, cx);
        self.persist_session(cx);
        if decoded.mixed_endings {
            self.notify(
                Severity::Warning,
                t!(
                    "markdown.notice.mixed_endings",
                    ending = decoded.format.line_ending.label()
                ),
                cx,
            );
        }
    }

    pub(super) fn begin_text_load(
        &mut self,
        link: Link,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = link.id;
        cx.spawn_in(window, async move |this, cx| {
            let load = cx
                .background_executor()
                .spawn(async move { read_text_link(&link) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.finish_text_load(id, load, window, cx)
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during load");
            }
        })
        .detach();
    }

    fn finish_text_load(
        &mut self,
        id: WorkbookId,
        load: TextLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.documents.entry(id).map(Entry::name) else {
            self.departure_load_finished(id, window, cx);
            return;
        };
        match load {
            TextLink::Loaded {
                decoded,
                stamp,
                base,
                file_exists,
                copy,
                recovery_lost,
            } => {
                let mode = match self.documents.entry(id) {
                    Some(Entry::Link(link)) => link.mode,
                    _ => ViewMode::default(),
                };
                if self.documents.shows_when_loaded(id) {
                    self.park_active(window, cx);
                }
                let from_recovery = copy.is_some();
                let (guard, moved) = if from_recovery {
                    text_file::recovered_base(base, stamp)
                } else {
                    (stamp, false)
                };
                let outcome = self.documents.install_text(TextLoad {
                    id,
                    format: decoded.format,
                    stamp: guard,
                    from_recovery,
                });
                if matches!(outcome, Installed::Gone) {
                    self.departure_load_finished(id, window, cx);
                    return;
                }
                if let Some(text) = self.documents.text_mut(id) {
                    text.notice =
                        RecoveredFile::judge(text.path.as_deref(), moved, file_exists).notice();
                }
                self.add_pane(id, &decoded.text, mode, window, cx);
                if let Some(copy) = copy {
                    self.adopt_text_recovery(id, copy, cx);
                }
                let path = self.documents.text(id).and_then(|text| text.path.clone());
                if let Some(path) = path {
                    self.remember_recent(&path, cx);
                }
                match outcome {
                    Installed::OnScreen => {
                        self.present_text(id, window, cx);
                        let provenance = if from_recovery {
                            Provenance::Recovery
                        } else {
                            Provenance::File
                        };
                        self.announce_loaded(&name, provenance, recovery_lost, false, cx);
                    }
                    Installed::Background | Installed::Gone => cx.notify(),
                }
            }
            TextLink::Missing => {
                self.documents.set_link_status(id, LinkStatus::Missing);
                let text = t!("notice.link_missing", name = name);
                self.notify_until_open(Severity::Warning, text, cx);
            }
            TextLink::Failed(message) => {
                self.documents.set_link_status(id, LinkStatus::NotLoaded);
                self.notify(
                    Severity::Error,
                    t!("notice.link_failed", name = name, message = message),
                    cx,
                );
            }
        }
        self.departure_load_finished(id, window, cx);
    }

    fn adopt_text_recovery(&mut self, id: WorkbookId, copy: PathBuf, cx: &mut Context<Self>) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let target = recovery::text_file(&directory, id);
        cx.spawn(async move |this, cx| {
            let moved = cx
                .background_executor()
                .spawn(
                    async move { recovery::adopt(&copy, &target).map_err(|error| (copy, error)) },
                )
                .await;
            let adopted = moved.is_ok();
            if let Err((copy, error)) = moved {
                tracing::warn!(?copy, %error, "could not adopt the recovery file");
            }
            let update = this.update(cx, |this, cx| {
                if adopted && let Some(text) = this.documents.text_mut(id) {
                    text.mark_recovery_adopted();
                }
                this.persist_session(cx)
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while adopting a recovery file");
            }
        })
        .detach();
    }

    // A crashed run left this text behind with no file to return to.
    pub(super) fn load_recovered_text(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { text_file::read(&path).map(|(decoded, _)| (decoded, path)) })
                .await;
            let update = this.update_in(cx, |this, window, cx| match read {
                Ok((decoded, copy)) => {
                    let mode = this.default_text_mode(cx);
                    this.park_active(window, cx);
                    let id = this.documents.open_text(
                        None,
                        DocumentKind::Markdown,
                        mode,
                        decoded.format,
                        None,
                    );
                    if let Some(text) = this.documents.text_mut(id) {
                        text.edited();
                    }
                    this.add_pane(id, &decoded.text, mode, window, cx);
                    this.adopt_text_recovery(id, copy, cx);
                    this.present_text(id, window, cx);
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

    pub(super) fn forget_text(&mut self, id: WorkbookId) {
        self.text_panes.remove(&id);
    }
}
