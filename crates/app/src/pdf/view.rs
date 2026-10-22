use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use async_channel::{Receiver, Sender};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use zenkai_i18n::t;

use self::finding::Finder;
use super::cache::{BITMAP_CAP_BYTES, BitmapCache, BitmapKey};
use super::document::PdfWorker;
use super::find::FindMatch;
use super::layout::{Layout, PageIdx, PageSize, PdfPosition, Viewport, ZoomMode};
use super::protocol::{
    DocId, Job, LinkTarget, OpenError, OutlineItem, PageContent, Reply, Request,
};
use super::selection::Selection;

mod finding;
mod navigation;
mod paint;
mod pointer;
mod replies;
mod requests;
mod screens;
mod side;
mod toolbar;

pub const CONTEXT: &str = "PdfDocument";
const NEIGHBOURS: usize = 1;
const THUMBNAIL_CAP_BYTES: usize = 32 * 1024 * 1024;
const KEY_SCROLL: f32 = 48.0;
const WHEEL_LINE: f32 = 20.0;

pub enum PdfEvent {
    Moved(PdfPosition),
    OpenWeb(String),
}

#[derive(Clone)]
enum Phase {
    Opening,
    Password { wrong: bool },
    Ready,
    Failed(OpenError),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SideList {
    Thumbnails,
    Outline,
}

pub struct PdfView {
    doc: DocId,
    path: PathBuf,
    worker: Result<PdfWorker, OpenError>,
    replies: Sender<Reply>,
    phase: Phase,
    zoom: ZoomMode,
    reported: PdfPosition,
    wanted_page: Option<PageIdx>,
    page_count: usize,
    sizes: Vec<PageSize>,
    outline: Vec<OutlineItem>,
    layout: Layout,
    bounds: Bounds<Pixels>,
    scroll_top: f32,
    scroll_left: f32,
    device_scale: f32,
    pages: BitmapCache,
    thumbnails: BitmapCache,
    pending: HashSet<BitmapKey>,
    pending_thumbnails: HashSet<BitmapKey>,
    token: Arc<AtomicU64>,
    thumbnail_token: Arc<AtomicU64>,
    contents: HashMap<PageIdx, Arc<PageContent>>,
    pending_contents: HashSet<PageIdx>,
    selection: Option<Selection>,
    dragging: bool,
    copy_waiting: bool,
    hover: Option<LinkTarget>,
    finder: Finder,
    reveal: Option<FindMatch>,
    page_input: Entity<InputState>,
    password_input: Entity<InputState>,
    side: SideList,
    outline_focus: FocusHandle,
    outline_cursor: usize,
    outline_scroll: UniformListScrollHandle,
    thumbnail_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _listener: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PdfEvent> for PdfView {}

impl PdfView {
    pub fn new(
        doc: DocId,
        path: PathBuf,
        position: PdfPosition,
        worker: Result<PdfWorker, OpenError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PdfView {
        let (replies, inbox) = async_channel::unbounded();
        let page_input = cx.new(|cx| InputState::new(window, cx));
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t!("pdf.password.placeholder"))
        });
        let go = cx.subscribe_in(
            &page_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.go_from_input(window, cx);
                }
            },
        );
        let unlock = cx.subscribe_in(
            &password_input,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_password(cx);
                }
            },
        );
        let listener = Self::listen(inbox, window, cx);
        let mut view = PdfView {
            doc,
            path,
            worker,
            replies,
            phase: Phase::Opening,
            zoom: position.zoom,
            reported: position,
            wanted_page: Some(PageIdx(position.page as usize)),
            page_count: 0,
            sizes: Vec::new(),
            outline: Vec::new(),
            layout: Layout::new(&[], 0, 1.0),
            bounds: Bounds::default(),
            scroll_top: 0.0,
            scroll_left: 0.0,
            device_scale: 1.0,
            pages: BitmapCache::new(BITMAP_CAP_BYTES),
            thumbnails: BitmapCache::new(THUMBNAIL_CAP_BYTES),
            pending: HashSet::new(),
            pending_thumbnails: HashSet::new(),
            token: Arc::new(AtomicU64::new(0)),
            thumbnail_token: Arc::new(AtomicU64::new(0)),
            contents: HashMap::new(),
            pending_contents: HashSet::new(),
            selection: None,
            dragging: false,
            copy_waiting: false,
            hover: None,
            finder: Finder::new(window, cx),
            reveal: None,
            page_input,
            password_input,
            side: SideList::Thumbnails,
            outline_focus: cx.focus_handle(),
            outline_cursor: 0,
            outline_scroll: UniformListScrollHandle::new(),
            thumbnail_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _listener: listener,
            _subscriptions: vec![go, unlock],
        };
        view.open(None);
        view
    }

    fn listen(inbox: Receiver<Reply>, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(reply) = inbox.recv().await {
                let update =
                    this.update_in(cx, |view, window, cx| view.on_reply(reply, window, cx));
                if update.is_err() {
                    break;
                }
            }
        })
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Password { .. } => self
                .password_input
                .update(cx, |input, cx| input.focus(window, cx)),
            _ => window.focus(&self.focus, cx),
        }
    }

    fn send(&self, job: Job) -> bool {
        let Ok(worker) = &self.worker else {
            return false;
        };
        worker.submit(Request {
            doc: self.doc,
            reply: self.replies.clone(),
            job,
        })
    }

    fn open(&mut self, password: Option<String>) {
        self.phase = Phase::Opening;
        let job = Job::Open {
            path: self.path.clone(),
            password,
        };
        if !self.send(job) {
            self.phase = Phase::Failed(match &self.worker {
                Err(error) => error.clone(),
                Ok(_) => OpenError::LibraryFailed(t!("pdf.worker_stopped").to_string()),
            });
        }
    }

    fn submit_password(&mut self, cx: &mut Context<Self>) {
        let password = self.password_input.read(cx).value().to_string();
        if password.is_empty() {
            return;
        }
        self.open(Some(password));
        cx.notify();
    }

    pub(super) fn viewport(&self) -> Viewport {
        Viewport {
            width: f32::from(self.bounds.size.width),
            height: f32::from(self.bounds.size.height),
        }
    }

    fn size_of(&self, page: PageIdx) -> PageSize {
        self.sizes
            .get(page.0)
            .or_else(|| self.sizes.first())
            .copied()
            .unwrap_or(PageSize::LETTER)
    }

    pub fn current_page(&self) -> PageIdx {
        self.layout
            .current_page(self.scroll_top, self.viewport().height)
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn zoom_label(&self) -> String {
        match self.zoom {
            ZoomMode::FitPage => t!("pdf.zoom.fit_page").to_string(),
            ZoomMode::FitWidth => t!("pdf.zoom.fit_width").to_string(),
            ZoomMode::Percent(percent) => t!("pdf.zoom.percent", percent = percent),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }

    pub(super) fn set_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.bounds == bounds {
            return;
        }
        let resized = self.bounds.size != bounds.size;
        self.bounds = bounds;
        if resized && self.is_ready() {
            self.relayout();
            self.request_visible();
            cx.notify();
        }
    }
}

impl Drop for PdfView {
    fn drop(&mut self) {
        self.send(Job::Close);
    }
}
