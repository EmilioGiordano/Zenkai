use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_channel::{Receiver, Sender};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use zenkai_i18n::t;

use self::finding::Finder;
use super::bitmap::to_image;
use super::cache::{BITMAP_CAP_BYTES, BitmapCache, BitmapKey};
use super::document::PdfWorker;
use super::find::FindMatch;
use super::layout::{
    Layout, PageIdx, PageSize, PdfPosition, Viewport, ZoomMode, percent_of, render_size, scale_for,
    step_zoom,
};
use super::protocol::{
    Bitmap, DocId, Job, LinkTarget, OpenError, OutlineItem, PageContent, Reply, Request,
};
use super::selection::Selection;

mod finding;
mod paint;
mod pointer;
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

    fn on_reply(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.focus.contains_focused(window, cx);
        match reply {
            Reply::Opened(Ok(opened)) => {
                self.phase = Phase::Ready;
                self.page_count = opened.page_count;
                self.sizes = vec![opened.first_page];
                self.outline = opened.outline;
                self.password_input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.send(Job::Sizes);
                self.relayout();
                self.request_visible();
                if had_focus {
                    window.focus(&self.focus, cx);
                }
            }
            Reply::Opened(Err(
                error @ (OpenError::PasswordRequired | OpenError::WrongPassword),
            )) => {
                let wrong = matches!(error, OpenError::WrongPassword);
                self.phase = Phase::Password { wrong };
                self.password_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    if had_focus {
                        input.focus(window, cx);
                    }
                });
            }
            Reply::Opened(Err(error)) => self.phase = Phase::Failed(error),
            Reply::Sizes(sizes) => {
                self.sizes = sizes;
                self.relayout();
                self.request_visible();
            }
            Reply::Rendered {
                page,
                pixel_width,
                thumbnail,
                bitmap,
            } => self.on_rendered(page, pixel_width, thumbnail, bitmap, window),
            Reply::Content { page, content } => {
                self.pending_contents.remove(&page);
                self.contents.insert(page, Arc::new(content));
                self.scroll_to_reveal(cx);
                if self.copy_waiting {
                    self.copy_selection(cx);
                }
            }
            Reply::Found {
                search,
                matches,
                next,
            } => self.on_found(search, matches, next, cx),
        }
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

    // Rebuilds the page positions for the zoom and sizes now known, keeping the page that is
    // on screen (or the one asked for) where it was.
    fn relayout(&mut self) {
        let viewport = self.viewport();
        if self.page_count == 0 || viewport.width <= 0.0 || viewport.height <= 0.0 {
            return;
        }
        let anchor = self
            .wanted_page
            .take()
            .unwrap_or_else(|| self.current_page());
        let scale = scale_for(self.zoom, self.size_of(anchor), viewport);
        self.layout = Layout::new(&self.sizes, self.page_count, scale);
        self.scroll_top = self.layout.scroll_to_page(anchor, viewport.height);
        self.clamp_scroll();
        self.drop_stale_widths();
    }

    fn clamp_scroll(&mut self) {
        let viewport = self.viewport();
        self.scroll_top = self
            .scroll_top
            .clamp(0.0, self.layout.max_scroll_top(viewport.height));
        self.scroll_left = self
            .scroll_left
            .clamp(0.0, self.layout.max_scroll_left(viewport.width));
    }

    fn drop_stale_widths(&mut self) {
        self.pending.clear();
        self.token.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn pixel_size(&self, page: PageIdx) -> (u32, u32) {
        render_size(
            self.layout.width_of(page),
            self.layout.height_of(page),
            self.device_scale,
        )
    }

    // Sends the renders and text reads the pages on screen still lack, the ones in view first.
    pub(super) fn request_visible(&mut self) {
        if !self.is_ready() {
            return;
        }
        let viewport = self.viewport();
        let near = self.layout.visible(self.scroll_top, viewport.height, 0);
        let wide = self
            .layout
            .visible(self.scroll_top, viewport.height, NEIGHBOURS);
        let wanted: Vec<PageIdx> = near
            .clone()
            .chain(wide.filter(|page| !near.contains(page)))
            .map(PageIdx)
            .collect();
        let keys: HashSet<BitmapKey> = wanted
            .iter()
            .map(|page| BitmapKey {
                page: *page,
                pixel_width: self.pixel_size(*page).0,
            })
            .collect();
        if self.pending.iter().any(|key| !keys.contains(key)) {
            self.drop_stale_widths();
        }
        for page in &wanted {
            let (pixel_width, pixel_height) = self.pixel_size(*page);
            let key = BitmapKey {
                page: *page,
                pixel_width,
            };
            if self.pages.contains(key) || self.pending.contains(&key) {
                continue;
            }
            self.pending.insert(key);
            self.send(Job::Render {
                page: *page,
                pixel_width,
                pixel_height,
                thumbnail: false,
                current: self.token.clone(),
                generation: self.token.load(Ordering::Relaxed),
            });
        }
        for index in near {
            let page = PageIdx(index);
            if !self.contents.contains_key(&page) && self.pending_contents.insert(page) {
                self.send(Job::Content { page });
            }
        }
        self.forget_far_contents(&wanted);
    }

    fn forget_far_contents(&mut self, near: &[PageIdx]) {
        let selected = self.selection.map(|selection| selection.pages());
        let found = self.finder.current_page();
        self.contents.retain(|page, _| {
            near.contains(page)
                || found == Some(*page)
                || selected
                    .as_ref()
                    .is_some_and(|pages| pages.contains(&page.0))
        });
    }

    fn on_rendered(
        &mut self,
        page: PageIdx,
        pixel_width: u32,
        thumbnail: bool,
        bitmap: Option<Bitmap>,
        window: &mut Window,
    ) {
        let key = BitmapKey { page, pixel_width };
        let (cache, pending) = if thumbnail {
            (&mut self.thumbnails, &mut self.pending_thumbnails)
        } else {
            (&mut self.pages, &mut self.pending)
        };
        pending.remove(&key);
        let Some(bitmap) = bitmap else {
            return;
        };
        let Some(image) = to_image(bitmap) else {
            return;
        };
        let viewport =
            self.layout
                .visible(self.scroll_top, self.bounds.size.height.into(), NEIGHBOURS);
        let bytes = image.bytes;
        let dropped = cache.insert(key, image.image, bytes, |page| {
            thumbnail || viewport.contains(&page.0)
        });
        for old in dropped {
            if let Err(error) = window.drop_image(old) {
                tracing::debug!(%error, "could not release a page texture");
            }
        }
    }

    // A tab that is not on screen holds no bitmaps; they render again when it comes back.
    pub fn release_bitmaps(&mut self, window: &mut Window) {
        if self.pages.used_bytes() == 0
            && self.thumbnails.used_bytes() == 0
            && self.pending.is_empty()
            && self.pending_thumbnails.is_empty()
        {
            return;
        }
        for old in self
            .pages
            .clear()
            .into_iter()
            .chain(self.thumbnails.clear())
        {
            if let Err(error) = window.drop_image(old) {
                tracing::debug!(%error, "could not release a page texture");
            }
        }
        self.pending.clear();
        self.pending_thumbnails.clear();
        self.token.fetch_add(1, Ordering::Relaxed);
        self.thumbnail_token.fetch_add(1, Ordering::Relaxed);
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.clamp_scroll();
        self.request_visible();
        let position = PdfPosition {
            page: u32::try_from(self.current_page().0).unwrap_or(0),
            zoom: self.zoom,
        };
        if position != self.reported {
            self.reported = position;
            cx.emit(PdfEvent::Moved(position));
            self.thumbnail_scroll
                .scroll_to_item(position.page as usize, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub fn go_to_page(&mut self, page: PageIdx, cx: &mut Context<Self>) {
        if !self.is_ready() || self.page_count == 0 {
            return;
        }
        let page = PageIdx(page.0.min(self.page_count - 1));
        self.scroll_top = self.layout.scroll_to_page(page, self.viewport().height);
        self.moved(cx);
    }

    pub fn step_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        let current = self.current_page().0;
        let target = if forward {
            current + 1
        } else {
            current.saturating_sub(1)
        };
        self.go_to_page(PageIdx(target), cx);
    }

    pub fn last_page(&mut self, cx: &mut Context<Self>) {
        self.go_to_page(PageIdx(self.page_count.saturating_sub(1)), cx);
    }

    fn go_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.page_input.read(cx).value().to_string();
        if let Ok(number) = typed.trim().parse::<usize>()
            && number >= 1
        {
            self.go_to_page(PageIdx(number - 1), cx);
        }
        self.sync_page_input(window, cx);
        window.focus(&self.focus, cx);
    }

    pub(super) fn sync_page_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = (self.current_page().0 + 1).to_string();
        self.page_input
            .update(cx, |input, cx| input.set_value(shown, window, cx));
    }

    pub fn focus_page_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_page_input(window, cx);
        self.page_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn set_zoom(&mut self, zoom: ZoomMode, cx: &mut Context<Self>) {
        if self.zoom == zoom {
            return;
        }
        self.zoom = zoom;
        self.relayout();
        self.moved(cx);
    }

    pub fn step_zoom(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        let percent = percent_of(self.layout.scale());
        self.set_zoom(step_zoom(percent, zoom_in), cx);
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.scroll_top += dy;
        self.scroll_left += dx;
        self.moved(cx);
    }

    pub(super) fn on_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(WHEEL_LINE));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control {
            self.step_zoom(dy > 0.0, cx);
        } else if event.modifiers.shift {
            self.scroll_by(-dy, 0.0, cx);
        } else {
            self.scroll_by(-dx, -dy, cx);
        }
    }

    pub(super) fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift {
            return;
        }
        let (dx, dy) = match keystroke.key.as_str() {
            "up" => (0.0, -KEY_SCROLL),
            "down" => (0.0, KEY_SCROLL),
            "left" => (-KEY_SCROLL, 0.0),
            "right" => (KEY_SCROLL, 0.0),
            _ => return,
        };
        self.scroll_by(dx, dy, cx);
        cx.stop_propagation();
    }
}

impl Drop for PdfView {
    fn drop(&mut self) {
        self.send(Job::Close);
    }
}
