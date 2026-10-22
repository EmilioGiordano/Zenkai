use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use zenkai_i18n::t;

use super::PdfView;
use crate::pdf::find::{FindMatch, FindResults};
use crate::pdf::layout::PageIdx;
use crate::pdf::selection::line_rects;
use zenkai_pdf_host::wire::{Job, MAX_QUERY_CHARS, PageRect};

const PAGES_PER_CHUNK: usize = 24;
const REVEAL_MARGIN: f32 = 48.0;

pub(super) struct Finder {
    pub(super) input: Entity<InputState>,
    results: FindResults,
    search: u64,
    from: PageIdx,
    _subscription: Subscription,
}

impl Finder {
    pub(super) fn new(window: &mut Window, cx: &mut Context<PdfView>) -> Finder {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("pdf.find.placeholder")));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |view, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => view.start_search(cx),
                InputEvent::PressEnter { shift, .. } => view.step_find(!*shift, window, cx),
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
            },
        );
        Finder {
            input,
            results: FindResults::new(""),
            search: 0,
            from: PageIdx(0),
            _subscription: subscription,
        }
    }

    pub(super) fn current_page(&self) -> Option<PageIdx> {
        self.results.current().map(|found| found.page)
    }
}

// What a page shows of the matches: the char ranges merged into line bars, and whether the
// bar belongs to the match in focus.
pub(super) struct Highlight {
    pub rect: PageRect,
    pub current: bool,
}

impl PdfView {
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finder
            .input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finder
            .input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.start_search(cx);
        window.focus(&self.focus, cx);
    }

    pub(super) fn find_status(&self, cx: &App) -> String {
        let query = self.finder.input.read(cx).value();
        let results = &self.finder.results;
        let (current, total) = results.position();
        if query.is_empty() {
            String::new()
        } else if total == 0 && results.is_complete() {
            t!("find.status_none").to_string()
        } else if total == 0 {
            t!("pdf.find.searching").to_string()
        } else {
            t!("find.status_of", current = current.max(1), total = total)
        }
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        let query: String = self
            .finder
            .input
            .read(cx)
            .value()
            .chars()
            .take(MAX_QUERY_CHARS)
            .collect();
        self.finder.search += 1;
        self.finder.from = self.current_page();
        self.finder.results = FindResults::new(&query);
        if query.trim().is_empty() {
            self.finder.results.finish();
        } else {
            self.send_find_chunk(PageIdx(0));
        }
        cx.notify();
    }

    fn send_find_chunk(&self, first: PageIdx) {
        self.send(Job::Find {
            search: self.finder.search,
            query: self.finder.results.query().to_string(),
            first,
            count: PAGES_PER_CHUNK,
        });
    }

    pub(super) fn on_found(
        &mut self,
        search: u64,
        matches: Vec<FindMatch>,
        next: Option<PageIdx>,
        cx: &mut Context<Self>,
    ) {
        if search != self.finder.search {
            return;
        }
        let had_focus = self.finder.results.current();
        let from = self.finder.from;
        self.finder.results.extend(matches, from);
        match next.filter(|_| !self.finder.results.is_full()) {
            Some(next) => self.send_find_chunk(next),
            None => {
                self.finder.results.finish();
                self.finder.results.extend([], from);
            }
        }
        if had_focus.is_none()
            && let Some(found) = self.finder.results.current()
        {
            self.reveal(found, cx);
        }
    }

    pub fn step_find(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(found) = self.finder.results.step(forward) {
            self.reveal(found, cx);
        }
        cx.notify();
    }

    pub(super) fn reveal(&mut self, found: FindMatch, cx: &mut Context<Self>) {
        self.go_to_page(found.page, cx);
        self.reveal = Some(found);
        self.scroll_to_reveal(cx);
    }

    // The match is brought into view once the page's text is known, since only then is its
    // place on the page known.
    pub(super) fn scroll_to_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.reveal else {
            return;
        };
        let Some(content) = self.contents.get(&found.page) else {
            return;
        };
        self.reveal = None;
        let span = found.span.start..=found.span.start + found.span.len.saturating_sub(1);
        let Some(bar) = line_rects(&content.chars, span).first().copied() else {
            return;
        };
        let scale = self.layout.scale();
        let top = self.layout.top_of(found.page) + bar.top * scale;
        let bottom = self.layout.top_of(found.page) + bar.bottom * scale;
        let height = self.viewport().height;
        if top < self.scroll_top + REVEAL_MARGIN {
            self.scroll_top = top - REVEAL_MARGIN;
        } else if bottom > self.scroll_top + height - REVEAL_MARGIN {
            self.scroll_top = bottom - height + REVEAL_MARGIN;
        }
        let left = self
            .layout
            .left_of(found.page, self.viewport().width, self.scroll_left)
            + bar.left * scale;
        if left < 0.0 || left > self.viewport().width {
            self.scroll_left += left - REVEAL_MARGIN;
        }
        self.moved(cx);
    }

    pub(super) fn highlights(&self, page: PageIdx) -> Vec<Highlight> {
        let Some(content) = self.contents.get(&page) else {
            return Vec::new();
        };
        self.finder
            .results
            .on_page(page)
            .flat_map(|(current, found)| {
                let span = found.span.start..=found.span.start + found.span.len.saturating_sub(1);
                line_rects(&content.chars, span)
                    .into_iter()
                    .map(move |rect| Highlight { rect, current })
            })
            .collect()
    }
}
