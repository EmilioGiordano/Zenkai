use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::*;
use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Stamp, Unsupported, Workbook, XlsxReader, open_xlsx};
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace, file_label};
use crate::agent_settings;
use crate::document;
use crate::documents::{Installed, Intent, Loaded, Provenance, Step};
use crate::entry::{Entry, Link, LinkStatus};
use crate::files::{self, FileLoad, LoadFailure};
use crate::recovery;
use crate::text_document::DocumentKind;

pub(super) enum LinkLoad {
    Loaded {
        file: Box<FileLoad>,
        provenance: Provenance,
        // The recovery copy the work was read from, to adopt once the workbook is installed.
        copy: Option<PathBuf>,
        recovery_lost: bool,
        // The memory dump could not be read, so the work came back from the recovery copy
        // without its undo history.
        dump_lost: bool,
        // The file is not as the workbook last had it: it changed while the workbook was out
        // of memory, and what loaded is the file as it is now.
        file_changed: bool,
    },
    Missing,
    Failed(String),
}

// Runs on the background executor. The recovery copy, when there is one, holds the unsaved
// work and is read instead of the file.
fn read_link(link: &Link, reader: XlsxReader) -> LinkLoad {
    let hibernation = link
        .rested
        .as_ref()
        .and_then(|rested| rested.hibernation.as_ref());
    let mut dump_lost = false;
    if let Some(hibernation) = hibernation {
        match Workbook::wake(&hibernation.dump, &hibernation.kept) {
            Ok(workbook) => {
                recovery::remove(&hibernation.dump);
                return LinkLoad::Loaded {
                    file: Box::new(FileLoad {
                        workbook,
                        unsupported: Vec::new(),
                        read_only: link.read_only,
                        origin: link.origin.unwrap_or(FileOrigin::Internet),
                        reader_fallback: None,
                    }),
                    provenance: Provenance::Dump,
                    copy: None,
                    recovery_lost: false,
                    dump_lost: false,
                    file_changed: file_changed_since_rest(link),
                };
            }
            Err(error) => {
                tracing::warn!(%error, "could not wake a hibernated workbook");
                dump_lost = true;
            }
        }
    }
    let mut recovery_lost = link.recovery_lost;
    if let Some(copy) = link.recovery.as_ref().filter(|copy| copy.exists()) {
        match open_xlsx(copy) {
            Ok(opened) => {
                if let Some(hibernation) = hibernation {
                    recovery::remove(&hibernation.dump);
                }
                return LinkLoad::Loaded {
                    file: Box::new(FileLoad {
                        workbook: opened.workbook,
                        unsupported: Vec::new(),
                        read_only: false,
                        // A recovery copy does not record where the work came from; it fails closed.
                        origin: FileOrigin::Internet,
                        reader_fallback: None,
                    }),
                    provenance: Provenance::Recovery,
                    copy: Some(copy.clone()),
                    recovery_lost: false,
                    dump_lost,
                    file_changed: file_changed_since_rest(link),
                };
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the recovery copy");
                recovery_lost = true;
            }
        }
    } else if link.recovery.is_some() {
        recovery_lost = true;
    }
    if dump_lost {
        // The file holds none of the unsaved work, so loading it would show the user an older
        // workbook as if nothing was lost. The link and its dump stay for another try.
        return LinkLoad::Failed(t!("open.hibernation_lost").to_string());
    }
    let Some(path) = &link.path else {
        return LinkLoad::Missing;
    };
    match files::load_workbook(path, reader) {
        Ok(file) => LinkLoad::Loaded {
            file: Box::new(file),
            provenance: Provenance::File,
            copy: None,
            recovery_lost,
            dump_lost: false,
            file_changed: changed_while_out_of_memory(link, path),
        },
        Err(LoadFailure::Missing) => LinkLoad::Missing,
        Err(failure) => LinkLoad::Failed(failure_text(&failure)),
    }
}

// Checked after the read, so a change between the check and the read is never missed.
fn changed_while_out_of_memory(link: &Link, path: &std::path::Path) -> bool {
    let Some(rested) = &link.rested else {
        return false;
    };
    Stamp::of(path).is_ok_and(|now| rested.file.changed(now))
}

// The unsaved work of a hibernated workbook still stands on the file it was opened from, which
// a later save would overwrite.
fn file_changed_since_rest(link: &Link) -> bool {
    link.path
        .as_deref()
        .is_some_and(|path| changed_while_out_of_memory(link, path))
}

pub(super) fn failure_text(failure: &LoadFailure) -> String {
    match failure {
        LoadFailure::Missing => t!("open.not_found").to_string(),
        LoadFailure::Engine(error) => error.to_string(),
        LoadFailure::Unreadable { reason, fallback } => {
            t!("open.unreadable", reason = reason, fallback = fallback)
        }
    }
}

impl Workspace {
    pub(super) fn open_document(
        &mut self,
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WorkbookId {
        self.park_active(window, cx);
        let id = self.documents.open(workbook, path, unsupported);
        self.refuse_orphaned_agent_change(window, cx);
        self.present_active(window, cx);
        id
    }

    pub(super) fn create_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Workbook::new_empty() {
            Ok(workbook) => {
                self.park_active(window, cx);
                self.documents.create(workbook);
                self.present_active(window, cx);
            }
            Err(error) => self.notify(Severity::Error, error.to_string(), cx),
        }
    }

    pub(super) fn switch_to(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.documents.entry(id) {
            None => {}
            Some(Entry::Link(_)) => {
                self.documents.want(id, Intent::Switch);
                self.begin_load(id, window, cx);
            }
            Some(Entry::Loaded(_) | Entry::Text(_)) if self.documents.shown_id() == Some(id) => {
                self.documents.activate(id);
            }
            Some(Entry::Loaded(_) | Entry::Text(_)) => {
                self.park_active(window, cx);
                self.documents.activate(id);
                self.present_active(window, cx);
            }
        }
    }

    pub(super) fn step_document(
        &mut self,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.documents.step(step) else {
            return;
        };
        if self.documents.shown_id() == Some(id) {
            return;
        }
        self.switch_to(id, window, cx);
        let name = self
            .documents
            .entry(id)
            .map(Entry::name)
            .unwrap_or_default();
        let position = self.documents.position_of(id);
        let count = self.documents.len();
        self.notify(
            Severity::Info,
            t!(
                "notice.workbook_position",
                name = name,
                position = position,
                total = count
            ),
            cx,
        );
    }

    // Everything tied to the workbook on screen is put away or dropped: a pending copy,
    // the Format Cells dialog and open bars would otherwise act on the next one.
    pub(super) fn park_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.active_sheet() else {
            return;
        };
        self.close_formula_bar(true, window, cx);
        if let Some((pos, text)) = self.grid.update(cx, |grid, cx| grid.take_edit(cx)) {
            self.commit_text(sheet, pos, text, window, cx);
        }
        let view = self.grid.read(cx).view_state();
        if let Some(document) = self.documents.active_mut() {
            document.view = view;
        }
        self.clipboard_source = None;
        self.grid.update(cx, |grid, cx| grid.set_marquee(None, cx));
        self.format_dialog = None;
        self.chart = None;
        self.rename = None;
        self.go_to = None;
    }

    pub(super) fn present_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.documents.shown_text().map(|text| text.id) {
            self.present_text(id, window, cx);
            return;
        }
        let Some(view) = self.documents.active().map(|document| document.view) else {
            self.show_empty(window, cx);
            return;
        };
        self.forget_find_results();
        let sheet = self.sheet_view();
        self.grid.update(cx, |grid, cx| {
            grid.reset(sheet, cx);
            grid.restore_view(view, cx);
        });
        self.sync_pending_highlight(cx);
        self.sync_review_marks(cx);
        self.refresh_cells(cx);
        window.set_window_title(&self.window_title());
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn show_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stats = None;
        self.forget_find_results();
        window.set_window_title(&self.window_title());
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.documents.shown_id() {
            self.close_document(id, window, cx);
        }
    }

    pub(super) fn close_document(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_dialog.is_some()
            || self.departure.is_some()
            || self.documents.entry(id).is_none()
        {
            return;
        }
        // Typing in the cell or the formula bar counts as an edit of the document being closed.
        if self.documents.active_id() == Some(id) {
            self.park_active(window, cx);
        }
        if !self.documents.holds_unsaved_work(id) {
            self.finish_close(id, window, cx);
            return;
        }
        self.ask_to_close_tab(id, window, cx);
    }

    pub(super) fn finish_close(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_active = self.documents.shown_id() == Some(id);
        if was_active {
            self.park_active(window, cx);
        }
        let recovery_copy = match self.documents.entry(id) {
            Some(Entry::Link(link)) => link.recovery.clone(),
            Some(Entry::Loaded(document)) => {
                self.previews.forget(document.generation());
                self.recovery_dir
                    .as_deref()
                    .map(|directory| recovery::document_file(directory, id))
            }
            Some(Entry::Text(_)) => self
                .recovery_dir
                .as_deref()
                .map(|directory| recovery::text_file(directory, id)),
            None => None,
        };
        self.forget_text(id);
        if let Some(file) = recovery_copy {
            cx.background_executor()
                .spawn(async move { recovery::remove_with_lock(&file) })
                .detach();
        }
        if let Some(Entry::Link(link)) = self.documents.entry(id)
            && let Some(hibernation) = link
                .rested
                .as_ref()
                .and_then(|rested| rested.hibernation.as_ref())
        {
            let dump = hibernation.dump.clone();
            cx.background_executor()
                .spawn(async move { recovery::remove(&dump) })
                .detach();
        }
        self.documents.close(id);
        self.refuse_orphaned_agent_change(window, cx);
        if was_active {
            self.present_active(window, cx);
        }
        cx.notify();
    }

    pub(super) fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.documents.take_reopenable() {
            Some(path) => self.open_path(path, window, cx),
            None => self.notify(Severity::Info, t!("notice.no_closed_workbook"), cx),
        }
    }

    pub(crate) fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.show_sheet_beside_chat(cx);
        if DocumentKind::of(&path).is_text() {
            self.open_text_file(path, window, cx);
            return;
        }
        if files::is_delimited_text(&path) {
            self.import_csv(path, window, cx);
            return;
        }
        if let Some(open) = self.documents.find_by_path(&path) {
            self.switch_to(open, window, cx);
            return;
        }
        let load = self
            .open_loads
            .start(t!("busy.opening", name = file_label(&path)));
        cx.notify();
        let started = Instant::now();
        let reader = files::xlsx_reader(agent_settings::settings(cx).advanced.xlsx_reader);
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { files::load_workbook(&task_path, reader) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.open_loads.finish(load);
                cx.notify();
                match result {
                    Ok(file) => {
                        this.remember_recent(&path, cx);
                        if let Some(open) = this.documents.find_by_path(&path) {
                            this.switch_to(open, window, cx);
                            return;
                        }
                        let FileLoad {
                            workbook,
                            unsupported,
                            read_only,
                            origin,
                            reader_fallback,
                        } = file;
                        let id = this.open_document(workbook, Some(path), unsupported, window, cx);
                        if let Some(document) = this.documents.get_mut(id) {
                            document.read_only = read_only;
                            document.origin = origin;
                        }
                        this.announce_opened(Some(started), cx);
                        if reader_fallback.is_some() {
                            this.notify(Severity::Info, t!("notice.fast_reader_fallback"), cx);
                        }
                    }
                    Err(LoadFailure::Missing) => this.notify(
                        Severity::Error,
                        t!("open.path_not_found", path = path.display()),
                        cx,
                    ),
                    Err(failure) => this.notify(Severity::Error, failure_text(&failure), cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during open");
            }
        })
        .detach();
    }

    fn announce_opened(&mut self, started: Option<Instant>, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        if document.read_only {
            self.notify(Severity::Warning, t!("notice.opened_read_only"), cx);
        } else if !document.unsupported.is_empty() {
            let text = t!(
                "notice.opened_unsupported",
                list = document::labels(&document.unsupported)
            );
            self.notify(Severity::Warning, text, cx);
        } else if document.origin == FileOrigin::Internet && started.is_some() {
            self.notify(Severity::Warning, t!("notice.protected_view"), cx);
        } else if let Some(started) = started {
            let text = t!("notice.opened_in", ms = started.elapsed().as_millis());
            self.notify(Severity::Info, text, cx);
        }
    }

    pub(super) fn begin_load(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(link) = self.documents.start_loading(id) else {
            return;
        };
        let name = self
            .documents
            .entry(id)
            .map(Entry::name)
            .unwrap_or_default();
        let open = self.open_loads.start(t!("busy.opening", name = name));
        cx.notify();
        if link.kind.is_text() {
            self.begin_text_load(link, window, cx);
            return;
        }
        let reader = files::xlsx_reader(agent_settings::settings(cx).advanced.xlsx_reader);
        cx.spawn_in(window, async move |this, cx| {
            let load = cx
                .background_executor()
                .spawn(async move { read_link(&link, reader) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.open_loads.finish(open);
                this.finish_load(id, load, window, cx)
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during load");
            }
        })
        .detach();
    }

    fn finish_load(
        &mut self,
        id: WorkbookId,
        load: LinkLoad,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.documents.entry(id).map(Entry::name) else {
            self.departure_load_finished(id, window, cx);
            return;
        };
        match load {
            LinkLoad::Loaded {
                file,
                provenance,
                copy,
                recovery_lost,
                dump_lost,
                file_changed,
            } => {
                if self.documents.shows_when_loaded(id) {
                    self.park_active(window, cx);
                }
                let outcome = self.documents.install(Loaded {
                    id,
                    workbook: file.workbook,
                    unsupported: file.unsupported,
                    read_only: file.read_only,
                    origin: file.origin,
                    provenance,
                });
                self.refuse_orphaned_agent_change(window, cx);
                if let Some(copy) = copy {
                    self.adopt_recovery(id, copy, cx);
                }
                let path = self
                    .documents
                    .get(id)
                    .and_then(|document| document.path.clone());
                if let Some(path) = path {
                    self.remember_recent(&path, cx);
                }
                match outcome {
                    Installed::OnScreen => {
                        self.present_active(window, cx);
                        self.announce_loaded(&name, provenance, recovery_lost, dump_lost, cx);
                    }
                    Installed::Background => {
                        if dump_lost {
                            let text = t!("notice.hibernation_lost", name = name);
                            self.notify(Severity::Warning, text, cx);
                        }
                        cx.notify();
                    }
                    Installed::Gone => {}
                }
                if file_changed {
                    let text = match provenance {
                        Provenance::File => t!("notice.file_changed_while_unloaded", name = name),
                        Provenance::Dump | Provenance::Recovery => {
                            t!("notice.file_changed_while_hibernated", name = name)
                        }
                    };
                    self.notify(Severity::Warning, text, cx);
                }
            }
            LinkLoad::Missing => {
                self.documents.set_link_status(id, LinkStatus::Missing);
                let text = t!("notice.link_missing", name = name);
                self.notify(Severity::Warning, text, cx);
            }
            LinkLoad::Failed(message) => {
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

    // The copy moves under this session's own name only once its workbook is in the list, and
    // the session file is rewritten right after, so no crash leaves the two disagreeing for long.
    fn adopt_recovery(&mut self, id: WorkbookId, copy: PathBuf, cx: &mut Context<Self>) {
        let Some(directory) = self.recovery_dir.clone() else {
            return;
        };
        let target = recovery::document_file(&directory, id);
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
                if adopted && let Some(document) = this.documents.get_mut(id) {
                    document.mark_recovery_adopted();
                }
                this.persist_session(cx)
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while adopting a recovery file");
            }
        })
        .detach();
    }

    pub(super) fn announce_loaded(
        &mut self,
        name: &str,
        provenance: Provenance,
        recovery_lost: bool,
        dump_lost: bool,
        cx: &mut Context<Self>,
    ) {
        if dump_lost {
            let text = t!("notice.hibernation_lost", name = name);
            self.notify(Severity::Warning, text, cx);
        } else if recovery_lost {
            let text = t!("notice.recovery_lost", name = name);
            self.notify(Severity::Warning, text, cx);
        } else {
            match provenance {
                Provenance::Recovery => {
                    let text = t!("notice.recovered", name = name);
                    self.notify(Severity::Warning, text, cx);
                }
                Provenance::File => self.announce_opened(None, cx),
                Provenance::Dump => {}
            }
        }
    }

    // Whether each listed file still exists and how big it is, one background task per file so
    // a stalled share holds up only its own answer. Network and device paths are left alone
    // until the file is opened: asking about them can send credentials to a host the user has
    // not touched this session.
    pub(super) fn probe_links(&mut self, cx: &mut Context<Self>) {
        let probes: Vec<(WorkbookId, PathBuf)> = self
            .documents
            .entries()
            .filter_map(|entry| match entry {
                Entry::Link(link) => link
                    .path
                    .clone()
                    .filter(|path| !files::is_remote_or_device(path))
                    .map(|path| (link.id, path)),
                Entry::Loaded(_) | Entry::Text(_) => None,
            })
            .collect();
        for (id, path) in probes {
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { std::fs::metadata(path) })
                    .await;
                let update = this.update(cx, |this, cx| {
                    let waiting = matches!(
                        this.documents.entry(id),
                        Some(Entry::Link(link)) if link.status == LinkStatus::NotLoaded
                    );
                    match result {
                        Ok(metadata) => this.documents.set_link_size(id, Some(metadata.len())),
                        Err(error) if waiting && error.kind() == std::io::ErrorKind::NotFound => {
                            this.documents.set_link_status(id, LinkStatus::Missing)
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                });
                if let Err(error) = update {
                    tracing::debug!(%error, "workspace closed while probing a file");
                }
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    use zenkai_agent::protected_view::FileOrigin;
    use zenkai_engine::{Engine, SaveTarget, Stamp, Workbook, XlsxReader};
    use zenkai_grid::ViewState;
    use zenkai_types::{CellPos, SheetId, WorkbookId};

    use super::{LinkLoad, changed_while_out_of_memory, read_link};
    use crate::documents::Provenance;
    use crate::entry::{FileSeen, Hibernation, Link, LinkStatus, Rested};
    use crate::files::FileLoad;
    use crate::recovery;
    use crate::spaces::SpaceId;

    struct Setup {
        _dir: tempfile::TempDir,
        dump: PathBuf,
        copy: PathBuf,
        link: Link,
    }

    fn text(workbook: &Workbook) -> String {
        workbook.cell(SheetId(0), CellPos::default()).text
    }

    // A workbook with the edit "42" and an undo step, as a hibernated link with a dump and a
    // recovery copy that both hold the edit.
    fn hibernated() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let mut book = Workbook::new_empty().unwrap();
        book.set_input(SheetId(0), CellPos::default(), "42")
            .unwrap();
        let dump = dir.path().join("book.zkh");
        let copy = dir.path().join("copy.xlsx");
        let kept = book.hibernate(&dump).unwrap();
        recovery::write(book.save_snapshot(&copy, SaveTarget::Copy).unwrap()).unwrap();
        let link = Link {
            id: WorkbookId(7),
            space: SpaceId(0),
            untitled: 1,
            path: None,
            recovery: Some(copy.clone()),
            dirty: true,
            read_only: false,
            unsupported: Vec::new(),
            sheet: zenkai_types::SheetId(0),
            view: ViewState::default(),
            size: None,
            status: LinkStatus::NotLoaded,
            recovery_lost: false,
            origin: Some(FileOrigin::Local),
            kind: Default::default(),
            mode: Default::default(),
            base: None,
            rested: Some(Box::new(Rested {
                file: FileSeen {
                    stamp: None,
                    at: SystemTime::now(),
                },
                hibernation: Some(Hibernation {
                    dump: dump.clone(),
                    kept,
                    restored: false,
                }),
            })),
        };
        Setup {
            _dir: dir,
            dump,
            copy,
            link,
        }
    }

    fn damage(dump: &Path) {
        let mut bytes = std::fs::read(dump).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(dump, bytes).unwrap();
    }

    fn loaded(load: LinkLoad) -> (Box<FileLoad>, Provenance, bool) {
        match load {
            LinkLoad::Loaded {
                file,
                provenance,
                dump_lost,
                ..
            } => (file, provenance, dump_lost),
            LinkLoad::Missing => panic!("the link was reported missing"),
            LinkLoad::Failed(message) => panic!("the link failed to load: {message}"),
        }
    }

    #[test]
    fn a_hibernated_workbook_wakes_from_its_dump_with_its_history_and_the_dump_goes() {
        let setup = hibernated();
        let (file, provenance, dump_lost) = loaded(read_link(&setup.link, XlsxReader::Fast));
        assert_eq!(provenance, Provenance::Dump);
        assert!(!dump_lost);
        let mut workbook = file.workbook;
        assert_eq!(text(&workbook), "42");
        workbook.undo().unwrap();
        assert_eq!(text(&workbook), "");
        assert!(!setup.dump.exists());
        assert!(setup.copy.exists());
    }

    #[test]
    fn a_file_changed_while_a_workbook_was_hibernated_is_noticed_on_wake() {
        let mut setup = hibernated();
        let file = setup._dir.path().join("saved.xlsx");
        std::fs::write(&file, b"first").unwrap();
        setup.link.path = Some(file.clone());
        let stamp = Stamp::of(&file).unwrap();
        let hibernation = setup.link.rested.take().unwrap().hibernation;
        setup.link.rested = Some(Box::new(Rested {
            file: FileSeen {
                stamp: Some(stamp),
                at: SystemTime::now(),
            },
            hibernation,
        }));
        let LinkLoad::Loaded { file_changed, .. } = read_link(&setup.link, XlsxReader::Fast) else {
            panic!("the link did not load");
        };
        assert!(!file_changed);

        let mut setup = hibernated();
        std::fs::write(&file, b"second, longer").unwrap();
        setup.link.path = Some(file);
        let hibernation = setup.link.rested.take().unwrap().hibernation;
        setup.link.rested = Some(Box::new(Rested {
            file: FileSeen {
                stamp: Some(stamp),
                at: SystemTime::now(),
            },
            hibernation,
        }));
        let LinkLoad::Loaded {
            file_changed,
            provenance,
            ..
        } = read_link(&setup.link, XlsxReader::Fast)
        else {
            panic!("the link did not load");
        };
        assert_eq!(provenance, Provenance::Dump);
        assert!(file_changed);
    }

    #[test]
    fn a_dump_that_cannot_be_read_falls_back_to_the_recovery_copy_and_says_so() {
        for break_dump in [damage as fn(&Path), |dump| {
            std::fs::remove_file(dump).unwrap()
        }] {
            let setup = hibernated();
            break_dump(&setup.dump);
            let (file, provenance, dump_lost) = loaded(read_link(&setup.link, XlsxReader::Fast));
            assert_eq!(provenance, Provenance::Recovery);
            assert!(dump_lost);
            assert_eq!(text(&file.workbook), "42");
            assert!(!setup.dump.exists());
        }
    }

    #[test]
    fn with_neither_the_dump_nor_the_copy_the_stale_file_is_never_shown() {
        let mut setup = hibernated();
        damage(&setup.dump);
        std::fs::remove_file(&setup.copy).unwrap();
        let file = setup._dir.path().join("saved.xlsx");
        let mut older = Workbook::new_empty().unwrap();
        older
            .set_input(SheetId(0), CellPos::default(), "old")
            .unwrap();
        recovery::write(older.save_snapshot(&file, SaveTarget::Copy).unwrap()).unwrap();
        setup.link.path = Some(file);
        assert!(matches!(
            read_link(&setup.link, XlsxReader::Fast),
            LinkLoad::Failed(_)
        ));
        assert!(
            setup.dump.exists(),
            "the damaged dump is kept for another look"
        );
    }

    #[test]
    fn a_file_that_changed_while_the_workbook_was_out_of_memory_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        std::fs::write(&path, b"first").unwrap();
        let mut setup = hibernated();
        setup.link.path = Some(path.clone());
        let stamp = Stamp::of(&path).unwrap();
        setup.link.rested = Some(Box::new(Rested {
            file: FileSeen {
                stamp: Some(stamp),
                at: SystemTime::now(),
            },
            hibernation: None,
        }));
        assert!(!changed_while_out_of_memory(&setup.link, &path));
        std::fs::write(&path, b"second, longer").unwrap();
        assert!(changed_while_out_of_memory(&setup.link, &path));
    }

    #[test]
    fn without_an_engine_stamp_a_file_modified_after_the_unload_counts_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        std::fs::write(&path, b"first").unwrap();
        let mut setup = hibernated();
        setup.link.rested = Some(Box::new(Rested {
            file: FileSeen {
                stamp: None,
                at: SystemTime::now(),
            },
            hibernation: None,
        }));
        assert!(!changed_while_out_of_memory(&setup.link, &path));
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"second").unwrap();
        assert!(changed_while_out_of_memory(&setup.link, &path));
    }

    #[test]
    fn a_link_that_never_left_memory_has_nothing_to_compare() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.xlsx");
        std::fs::write(&path, b"first").unwrap();
        let mut setup = hibernated();
        setup.link.rested = None;
        assert!(!changed_while_out_of_memory(&setup.link, &path));
    }
}
