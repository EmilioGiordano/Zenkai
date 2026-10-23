use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::base::v_flex;
use gpui_kit::*;
use zenkai_types::WorkbookId;

use super::Workspace;
use crate::actions::{
    PdfActualSize, PdfFirstPage, PdfFitPage, PdfFitWidth, PdfLastPage, PdfNextPage,
    PdfPreviousPage, SelectPdfPage, TogglePdfOutline,
};
use crate::documents::Installed;
use crate::pdf::layout::{PageIdx, ZoomMode};
use crate::pdf::{DocId, PdfEvent, PdfView};

pub(super) struct PdfPane {
    pub(super) view: Entity<PdfView>,
    _events: Subscription,
}

pub(super) type PdfPanes = HashMap<WorkbookId, PdfPane>;

impl Workspace {
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
        let client = self.pdf_client.clone();
        let widths = self.panels.widths;
        let view =
            cx.new(|cx| PdfView::new(DocId(id.0), path, position, widths, client, window, cx));
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
                    self.pdf_places.remember(&tab.path, *position);
                }
                self.persist_session(cx);
                cx.notify();
            }
            PdfEvent::OpenWeb(url) => self.confirm_web_link(url.clone(), window, cx),
            PdfEvent::SideResized(width) => {
                self.panels.widths = self.panels.widths.with_pdf_width(*width);
                for (other, pane) in &self.pdf_panes {
                    if *other != id {
                        pane.view
                            .update(cx, |view, cx| view.set_side_width(*width, cx));
                    }
                }
                self.persist_session(cx);
            }
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
        let position = self.pdf_places.position_of(&path).unwrap_or_default();
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
        busy: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(self.render_held_settings(cx))
            .child(view)
            .child(self.render_status(busy, cx))
    }
}

impl Workspace {
    // The PDF on screen takes the command when there is one, wherever the keyboard focus is.
    pub(super) fn forward_to_pdf(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        command: impl FnOnce(&mut PdfView, &mut Window, &mut Context<PdfView>),
    ) -> bool {
        let Some(view) = self.shown_pdf_view() else {
            return false;
        };
        view.update(cx, |view, cx| command(view, window, cx));
        true
    }
}

pub(super) fn pdf_actions<E: InteractiveElement>(element: E, cx: &Context<Workspace>) -> E {
    element
        .on_action(cx.listener(|this, _: &PdfPreviousPage, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| view.step_page(false, cx));
        }))
        .on_action(cx.listener(|this, _: &PdfNextPage, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| view.step_page(true, cx));
        }))
        .on_action(cx.listener(|this, _: &PdfFirstPage, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| view.go_to_page(PageIdx(0), cx));
        }))
        .on_action(cx.listener(|this, _: &PdfLastPage, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| view.last_page(cx));
        }))
        .on_action(cx.listener(|this, _: &SelectPdfPage, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| view.select_page(cx));
        }))
        .on_action(cx.listener(|this, _: &PdfFitPage, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| {
                view.set_zoom(ZoomMode::FitPage, cx)
            });
        }))
        .on_action(cx.listener(|this, _: &PdfFitWidth, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| {
                view.set_zoom(ZoomMode::FitWidth, cx)
            });
        }))
        .on_action(cx.listener(|this, _: &PdfActualSize, window, cx| {
            this.forward_to_pdf(window, cx, |view, _, cx| view.actual_size(cx));
        }))
        .on_action(cx.listener(|this, _: &TogglePdfOutline, window, cx| {
            this.forward_to_pdf(window, cx, |view, window, cx| {
                view.toggle_outline(window, cx)
            });
        }))
}
