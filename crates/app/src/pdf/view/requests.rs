use std::collections::HashSet;
use std::sync::atomic::Ordering;

use super::{NEIGHBOURS, PdfView};
use crate::pdf::cache::BitmapKey;
use crate::pdf::layout::{Layout, PageIdx, render_size, scale_for};
use zenkai_pdf_host::wire::Job;

impl PdfView {
    // Rebuilds the page positions for the zoom and sizes now known, keeping the place that is
    // on screen (or the page asked for) where it was.
    pub(super) fn relayout(&mut self) {
        let viewport = self.viewport();
        if self.page_count == 0 || viewport.width <= 0.0 || viewport.height <= 0.0 {
            return;
        }
        self.laid_out_for = Some(viewport);
        let wanted = self.wanted_page.take();
        let anchor = wanted.unwrap_or_else(|| self.current_page());
        let scale = scale_for(self.zoom, self.size_of(anchor), viewport);
        let previous = std::mem::replace(
            &mut self.layout,
            Layout::new(&self.sizes, self.page_count, scale),
        );
        self.scroll_top = match wanted {
            Some(page) => self.layout.scroll_to_page(page, viewport.height),
            None => self.layout.scroll_after_rescale(&previous, self.scroll_top),
        };
        self.clamp_scroll();
        self.drop_stale_widths();
    }

    // The layout follows the reader's width after a side resize whatever order the size
    // reports and the drag end arrive in.
    pub(super) fn refit_if_resized(&mut self) {
        if self.is_ready() && self.side_drag.is_none() && self.laid_out_for != Some(self.viewport())
        {
            self.relayout();
        }
    }

    pub(super) fn clamp_scroll(&mut self) {
        let viewport = self.viewport();
        self.scroll_top = self
            .scroll_top
            .clamp(0.0, self.layout.max_scroll_top(viewport.height));
        self.scroll_left = self
            .scroll_left
            .clamp(0.0, self.layout.max_scroll_left(viewport.width));
    }

    pub(super) fn drop_stale_widths(&mut self) {
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
            self.send_with(
                Job::Render {
                    page: *page,
                    pixel_width,
                    pixel_height,
                    thumbnail: false,
                    generation: self.token.load(Ordering::Relaxed),
                },
                Some(self.token.clone()),
            );
        }
        for index in near {
            let page = PageIdx(index);
            if !self.contents.contains_key(&page) && self.pending_contents.insert(page) {
                self.send(Job::Content { page });
            }
        }
        self.forget_far_contents(&wanted);
    }

    pub(super) fn forget_far_contents(&mut self, near: &[PageIdx]) {
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
}
