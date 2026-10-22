use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::base::v_flex;
use gpui_kit::*;
use zenkai_types::WorkbookId;

use super::Workspace;
use crate::documents::Installed;
use crate::pdf::{DocId, OpenError, PdfEvent, PdfPosition, PdfView, PdfWorker};

pub(super) struct PdfPane {
    pub(super) view: Entity<PdfView>,
    _events: Subscription,
}

pub(super) type PdfPanes = HashMap<WorkbookId, PdfPane>;

// Where the user was reading each PDF closed or reopened during this run. A tab still open
// keeps its position in the session instead.
pub(super) type PdfPlaces = HashMap<PathBuf, PdfPosition>;

impl Workspace {
    fn pdf_worker(&mut self) -> Result<PdfWorker, OpenError> {
        if let Some(worker) = &self.pdf_worker {
            return Ok(worker.clone());
        }
        let worker = PdfWorker::start()?;
        self.pdf_worker = Some(worker.clone());
        Ok(worker)
    }

    pub(super) fn shown_pdf_view(&self) -> Option<Entity<PdfView>> {
        let id = self.documents.shown_pdf()?.id;
        self.pdf_panes.get(&id).map(|pane| pane.view.clone())
    }

    fn add_pdf_view(&mut self, id: WorkbookId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, position)) = self
            .documents
            .pdf(id)
            .map(|tab| (tab.path.clone(), tab.position))
        else {
            return;
        };
        let worker = self.pdf_worker();
        let view = cx.new(|cx| PdfView::new(DocId(id.0), path, position, worker, window, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &PdfEvent, window, cx| this.on_pdf_event(id, event, window, cx),
        );
        self.pdf_panes.insert(
            id,
            PdfPane {
                view,
                _events: events,
            },
        );
    }

    fn on_pdf_event(
        &mut self,
        id: WorkbookId,
        event: &PdfEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PdfEvent::Moved(position) => {
                if let Some(tab) = self.documents.pdf_mut(id) {
                    tab.position = *position;
                    self.pdf_places.insert(tab.path.clone(), *position);
                }
                self.persist_session(cx);
                cx.notify();
            }
            PdfEvent::OpenWeb(url) => self.confirm_web_link(url.clone(), window, cx),
        }
    }

    pub(super) fn open_pdf_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = self.documents.find_by_path(&path) {
            self.switch_to(open, window, cx);
            return;
        }
        self.remember_recent(&path, cx);
        let position = self.pdf_places.get(&path).copied().unwrap_or_default();
        self.park_active(window, cx);
        let id = self.documents.open_pdf(path, position);
        self.add_pdf_view(id, window, cx);
        self.present_pdf(id, window, cx);
        self.persist_session(cx);
    }

    // A PDF listed in the sidebar comes on screen. Nothing is read here: the view asks the
    // reader thread to open the file, which answers in the background.
    pub(super) fn begin_pdf_load(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.documents.shows_when_loaded(id) {
            self.park_active(window, cx);
        }
        let outcome = self.documents.install_pdf(id);
        if matches!(outcome, Installed::Gone) {
            return;
        }
        self.add_pdf_view(id, window, cx);
        match outcome {
            Installed::OnScreen => self.present_pdf(id, window, cx),
            Installed::Background | Installed::Gone => cx.notify(),
        }
    }

    pub(super) fn present_pdf(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stats = None;
        self.forget_find_results();
        window.set_window_title(&self.window_title());
        if let Some(pane) = self.pdf_panes.get(&id) {
            pane.view.update(cx, |view, cx| view.focus(window, cx));
        }
        cx.notify();
    }

    // A PDF that is not on screen holds no bitmaps, so many open PDFs cost no more memory
    // than the one being read.
    pub(super) fn rest_hidden_pdfs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.documents.shown_pdf().map(|tab| tab.id);
        for (id, pane) in &self.pdf_panes {
            if Some(*id) != shown {
                pane.view.update(cx, |view, _| view.release_bitmaps(window));
            }
        }
    }

    pub(super) fn forget_pdf(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self.pdf_panes.remove(&id) {
            pane.view.update(cx, |view, _| view.release_bitmaps(window));
        }
    }

    pub(super) fn render_pdf(
        &self,
        view: Entity<PdfView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(self.render_held_settings(cx))
            .child(view)
            .child(self.render_status(cx))
    }
}
