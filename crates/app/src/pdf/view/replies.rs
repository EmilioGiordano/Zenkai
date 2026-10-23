use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::*;

use super::side::thumbnail_kept;
use super::{NEIGHBOURS, PdfView, Phase};
use crate::pdf::bitmap::to_image;
use crate::pdf::cache::BitmapKey;
use crate::pdf::layout::PageIdx;
use zenkai_pdf_host::wire::{Bitmap, Job, OpenError, Reply};

impl PdfView {
    pub(super) fn on_reply(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
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
            Reply::Stopped => self.phase = Phase::Failed(OpenError::HostStopped),
            Reply::Sizes(sizes) => {
                self.sizes = sizes;
                self.relayout();
                self.request_visible();
            }
            Reply::Rendered {
                page,
                pixel_width,
                thumbnail,
                generation,
                bitmap,
            } => self.on_rendered(page, pixel_width, thumbnail, generation, bitmap, window),
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

    pub(super) fn on_rendered(
        &mut self,
        page: PageIdx,
        pixel_width: u32,
        thumbnail: bool,
        generation: u64,
        bitmap: Option<Bitmap>,
        window: &mut Window,
    ) {
        let token = if thumbnail {
            &self.thumbnail_token
        } else {
            &self.token
        };
        // A render requested before the tab rested or the zoom changed must not come back to life.
        if token.load(Ordering::Relaxed) != generation {
            return;
        }
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
        let visible = &self.thumbnail_visible;
        let dropped = cache.insert(key, image.image, bytes, |page| {
            if thumbnail {
                thumbnail_kept(visible, page)
            } else {
                viewport.contains(&page.0)
            }
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
}
