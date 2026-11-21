mod links;
mod plan;
mod render;

use std::sync::Arc;

use gpui_kit::component::text::TextViewState;
use gpui_kit::*;

use crate::text_zoom::TextZoom;
use crate::updates::{self, Notes, Status, Updates, Version};
use plan::{Body, Plan};

pub const CONTEXT: &str = "WhatsNew";

const SCROLL_LINE: f32 = 40.0;

pub enum NotesEvent {
    OpenLink(String),
}

struct Drawn {
    state: Entity<TextViewState>,
    text: SharedString,
    loaded: bool,
}

struct Shown {
    plan: Plan,
    bodies: Vec<Option<Drawn>>,
}

// The What's new tab: the notes the update check fetched, read-only. Each version is a heading
// the tab draws and a body the Markdown text view draws in its restricted mode.
pub struct NotesView {
    shown: Option<Shown>,
    seen: Option<(Arc<Notes>, Status)>,
    zoom: TextZoom,
    focus: FocusHandle,
    scroll: ScrollHandle,
    refresh_task: Option<Task<()>>,
    _updates: Subscription,
}

impl EventEmitter<NotesEvent> for NotesView {}

impl NotesView {
    pub fn new(cx: &mut Context<Self>) -> NotesView {
        let updates = cx.observe_global::<Updates>(|this, cx| this.refresh(cx));
        let mut view = NotesView {
            shown: None,
            seen: None,
            zoom: TextZoom::default(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            refresh_task: None,
            _updates: updates,
        };
        view.refresh(cx);
        view
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.zoom = self.zoom.larger();
        cx.notify();
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.zoom = self.zoom.smaller();
        cx.notify();
    }

    pub fn zoom_reset(&mut self, cx: &mut Context<Self>) {
        self.zoom = TextZoom::default();
        cx.notify();
    }

    // Every change to the update state lands here; only a new status or new notes cost work,
    // and that work runs in the background.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let status = updates::status(cx);
        let notes = updates::notes(cx);
        let unchanged = self
            .seen
            .as_ref()
            .is_some_and(|(seen, was)| Arc::ptr_eq(seen, &notes) && *was == status);
        if unchanged {
            return;
        }
        let Some(installed) = Version::installed() else {
            tracing::debug!("the What's new tab needs the installed version");
            return;
        };
        self.seen = Some((notes.clone(), status));
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let plan = cx
                .background_executor()
                .spawn(async move { plan::plan(status, &notes, installed) })
                .await;
            if let Err(error) = this.update(cx, |this, cx| this.show(plan, cx)) {
                tracing::debug!(%error, "the What's new tab closed before its notes were ready");
            }
        }));
    }

    fn show(&mut self, plan: Plan, cx: &mut Context<Self>) {
        let bodies = match &plan.body {
            Body::Sections { sections, .. } => sections
                .iter()
                .map(|section| {
                    section.text.as_ref().map(|text| Drawn {
                        state: cx.new(|cx| TextViewState::markdown("", cx)),
                        text: SharedString::from(text.clone()),
                        loaded: false,
                    })
                })
                .collect(),
            Body::UpToDate | Body::NoNotes | Body::Nothing => Vec::new(),
        };
        self.shown = Some(Shown { plan, bodies });
        cx.notify();
    }

    // A text view parses with the extensions it has when it is given text, and it takes them
    // from the element that draws it. So each body starts empty and gets its text only after a
    // frame has drawn it, and no note is ever parsed without the restricted extensions.
    fn load_drawn_bodies(&mut self, window: &mut Window) {
        let Some(shown) = &mut self.shown else {
            return;
        };
        let pending: Vec<(Entity<TextViewState>, SharedString)> = shown
            .bodies
            .iter_mut()
            .flatten()
            .filter(|body| !body.loaded)
            .map(|body| {
                body.loaded = true;
                (body.state.clone(), body.text.clone())
            })
            .collect();
        if pending.is_empty() {
            return;
        }
        window.on_next_frame(move |_, cx| {
            for (state, text) in pending {
                state.update(cx, |state, cx| state.set_text(&text, cx));
            }
        });
    }

    // The arrow, page and home keys scroll the notes wherever the focus is inside the tab.
    fn scroll_by_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift {
            return;
        }
        let offset = self.scroll.offset();
        let furthest = self.scroll.max_offset().y;
        let page = self.scroll.bounds().size.height;
        let line = px(SCROLL_LINE);
        let target = match event.keystroke.key.as_str() {
            "down" => offset.y - line,
            "up" => offset.y + line,
            "pagedown" => offset.y - page,
            "pageup" => offset.y + page,
            "home" => px(0.),
            "end" => -furthest,
            _ => return,
        };
        self.scroll
            .set_offset(point(offset.x, target.max(-furthest).min(px(0.))));
        cx.stop_propagation();
        cx.notify();
    }
}
