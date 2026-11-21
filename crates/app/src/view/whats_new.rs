use gpui_kit::base::v_flex;
use gpui_kit::*;
use zenkai_types::WorkbookId;

use super::Workspace;
use crate::markdown_restricted::vetted_link;
use crate::whats_new::{NotesEvent, NotesView};

pub(super) struct NotesPane {
    id: WorkbookId,
    view: Entity<NotesView>,
    _events: Subscription,
}

impl Workspace {
    pub(super) fn shown_notes_view(&self) -> Option<Entity<NotesView>> {
        self.documents.shown_notes()?;
        self.notes_pane.as_ref().map(|pane| pane.view.clone())
    }

    pub(super) fn show_whats_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.documents.notes_id() {
            self.switch_to(id, window, cx);
        } else {
            self.park_active(window, cx);
            let id = self.documents.open_notes();
            let view = cx.new(NotesView::new);
            let events =
                cx.subscribe_in(&view, window, |this, _, event: &NotesEvent, window, cx| {
                    this.on_notes_event(event, window, cx)
                });
            self.notes_pane = Some(NotesPane {
                id,
                view,
                _events: events,
            });
        }
        self.present_notes(window, cx);
    }

    // Links reach here as text from the tab; only what `vetted_link` accepts is asked about,
    // and the prompt shows and opens the address it returns.
    fn on_notes_event(&mut self, event: &NotesEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            NotesEvent::OpenLink(link) => {
                if let Some(address) = vetted_link(link) {
                    self.confirm_web_link(address, window, cx);
                }
            }
        }
    }

    pub(super) fn present_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stats = None;
        self.forget_find_results();
        window.set_window_title(&self.window_title());
        if let Some(pane) = &self.notes_pane {
            pane.view.update(cx, |view, cx| view.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn forget_notes(&mut self, id: WorkbookId) {
        if self.notes_pane.as_ref().is_some_and(|pane| pane.id == id) {
            self.notes_pane = None;
        }
    }

    pub(super) fn render_notes(
        &self,
        view: Entity<NotesView>,
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

    // The tab on screen takes the zoom command when there is one, as a Markdown tab does.
    pub(super) fn forward_to_notes(
        &mut self,
        cx: &mut Context<Self>,
        command: impl FnOnce(&mut NotesView, &mut Context<NotesView>),
    ) -> bool {
        let Some(view) = self.shown_notes_view() else {
            return false;
        };
        view.update(cx, |view, cx| command(view, cx));
        true
    }
}
