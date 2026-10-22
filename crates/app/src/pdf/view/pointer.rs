use gpui_kit::*;

use super::{PdfEvent, PdfView};
use crate::pdf::layout::PageIdx;
use crate::pdf::protocol::{Job, LinkTarget};
use crate::pdf::selection::{Selection, TextPos, char_at, selected_text};

// A point of the canvas in page terms: which page, and where on it in PDF points.
struct PagePoint {
    page: PageIdx,
    x: f32,
    y: f32,
}

impl PdfView {
    fn locate(&self, position: Point<Pixels>) -> Option<PagePoint> {
        if self.layout.page_count() == 0 {
            return None;
        }
        let local_x = f32::from(position.x - self.bounds.origin.x);
        let local_y = f32::from(position.y - self.bounds.origin.y);
        let content_y = local_y + self.scroll_top;
        let page = self.layout.page_at(content_y);
        let left = self
            .layout
            .left_of(page, self.viewport().width, self.scroll_left);
        let scale = self.layout.scale();
        Some(PagePoint {
            page,
            x: (local_x - left) / scale,
            y: (content_y - self.layout.top_of(page)) / scale,
        })
    }

    fn text_pos_at(&self, point: &PagePoint) -> Option<TextPos> {
        let content = self.contents.get(&point.page)?;
        let index = char_at(&content.chars, point.x, point.y)?;
        Some(TextPos {
            page: point.page,
            index,
        })
    }

    fn link_at(&self, point: &PagePoint) -> Option<LinkTarget> {
        let content = self.contents.get(&point.page)?;
        content
            .links
            .iter()
            .find(|link| link.rect.contains(point.x, point.y))
            .map(|link| link.target.clone())
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(point) = self.locate(event.position) else {
            return;
        };
        if let Some(target) = self.link_at(&point) {
            self.follow(target, cx);
            return;
        }
        self.selection = self.text_pos_at(&point).map(|pos| Selection {
            anchor: pos,
            head: pos,
        });
        self.dragging = self.selection.is_some();
        cx.notify();
    }

    pub(super) fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(point) = self.locate(event.position) else {
            return;
        };
        if self.dragging && event.dragging() {
            let head = self.text_pos_at(&point);
            if let (Some(selection), Some(head)) = (&mut self.selection, head) {
                selection.head = head;
                cx.notify();
            }
            return;
        }
        let hover = self.link_at(&point);
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    pub(super) fn on_mouse_up(&mut self, cx: &mut Context<Self>) {
        self.dragging = false;
        if self
            .selection
            .is_some_and(|selection| selection.anchor == selection.head)
        {
            self.selection = None;
            cx.notify();
        }
    }

    fn follow(&mut self, target: LinkTarget, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Page(page) => self.go_to_page(page, cx),
            LinkTarget::Web(url) => cx.emit(PdfEvent::OpenWeb(url)),
        }
    }

    pub fn select_page(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        let page = self.current_page();
        self.selection = Some(Selection {
            anchor: TextPos { page, index: 0 },
            head: TextPos {
                page,
                index: usize::MAX,
            },
        });
        cx.notify();
    }

    // The text of a selection that spans pages not read yet is asked for, and the copy
    // completes when those pages arrive.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        let missing: Vec<PageIdx> = selection
            .pages()
            .map(PageIdx)
            .filter(|page| !self.contents.contains_key(page))
            .collect();
        if !missing.is_empty() {
            self.copy_waiting = true;
            for page in missing {
                if self.pending_contents.insert(page) {
                    self.send(Job::Content { page });
                }
            }
            return;
        }
        self.copy_waiting = false;
        let text = selected_text(&selection, &self.contents);
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}
