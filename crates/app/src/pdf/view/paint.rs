use std::sync::Arc;

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use super::PdfView;
use crate::pdf::cache::BitmapKey;
use crate::pdf::layout::PageIdx;
use crate::pdf::selection::line_rects;
use zenkai_pdf_host::wire::PageRect;

const SHADOW_OFFSET: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkKind {
    Selection,
    Match,
    CurrentMatch,
}

struct Mark {
    bounds: Bounds<Pixels>,
    kind: MarkKind,
}

struct PagePaint {
    bounds: Bounds<Pixels>,
    image: Option<Arc<RenderImage>>,
    marks: Vec<Mark>,
}

// Everything the canvas draws, prepared in render so that paint only draws it.
pub(super) struct Frame {
    pages: Vec<PagePaint>,
    paper: Hsla,
    shadow: Hsla,
    selection: Hsla,
    found: Hsla,
    current: Hsla,
    ring: Hsla,
}

fn scaled(rect: &PageRect, scale: f32, origin: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(
            origin.x + px(rect.left * scale),
            origin.y + px(rect.top * scale),
        ),
        size(
            px((rect.right - rect.left) * scale),
            px((rect.bottom - rect.top) * scale),
        ),
    )
}

impl PdfView {
    pub(super) fn frame(&mut self, cx: &App) -> Frame {
        let theme = cx.theme();
        let viewport = self.viewport();
        let scale = self.layout.scale();
        let visible = self.layout.visible(self.scroll_top, viewport.height, 0);
        let mut pages = Vec::with_capacity(visible.len());
        for index in visible {
            let page = PageIdx(index);
            let origin = point(
                px(self.layout.left_of(page, viewport.width, self.scroll_left)),
                px(self.layout.top_of(page) - self.scroll_top),
            );
            let bounds = Bounds::new(
                origin,
                size(
                    px(self.layout.width_of(page)),
                    px(self.layout.height_of(page)),
                ),
            );
            let key = BitmapKey {
                page,
                pixel_width: self.pixel_size(page).0,
            };
            let image = self.pages.get(key).or_else(|| self.pages.any_width(page));
            pages.push(PagePaint {
                bounds,
                image,
                marks: self.marks(page, scale, origin),
            });
        }
        Frame {
            pages,
            paper: white(),
            shadow: black().opacity(0.18),
            selection: theme.primary.opacity(0.30),
            found: theme.warning.opacity(0.45),
            current: theme.primary.opacity(0.45),
            ring: theme.primary,
        }
    }

    fn marks(&self, page: PageIdx, scale: f32, origin: Point<Pixels>) -> Vec<Mark> {
        let Some(content) = self.contents.get(&page) else {
            return Vec::new();
        };
        let mut marks: Vec<Mark> = self
            .highlights(page)
            .into_iter()
            .map(|highlight| Mark {
                bounds: scaled(&highlight.rect, scale, origin),
                kind: if highlight.current {
                    MarkKind::CurrentMatch
                } else {
                    MarkKind::Match
                },
            })
            .collect();
        if let Some(span) = self
            .selection
            .and_then(|selection| selection.span_on(page, content.chars.len()))
        {
            marks.extend(line_rects(&content.chars, span).iter().map(|rect| Mark {
                bounds: scaled(rect, scale, origin),
                kind: MarkKind::Selection,
            }));
        }
        marks
    }
}

pub(super) fn paint(frame: &Frame, canvas: Bounds<Pixels>, window: &mut Window) {
    window.with_content_mask(Some(ContentMask { bounds: canvas }), |window| {
        for page in &frame.pages {
            let bounds = Bounds::new(canvas.origin + page.bounds.origin, page.bounds.size);
            let shadow = Bounds::new(
                bounds.origin + point(px(SHADOW_OFFSET), px(SHADOW_OFFSET)),
                bounds.size,
            );
            window.paint_quad(fill(shadow, frame.shadow));
            window.paint_quad(fill(bounds, frame.paper));
            if let Some(image) = &page.image
                && let Err(error) =
                    window.paint_image(bounds, bounds, Corners::default(), image.clone(), 0, false)
            {
                tracing::debug!(%error, "could not draw a page");
            }
            for mark in &page.marks {
                let mark_bounds = Bounds::new(canvas.origin + mark.bounds.origin, mark.bounds.size);
                let color = match mark.kind {
                    MarkKind::Selection => frame.selection,
                    MarkKind::Match => frame.found,
                    MarkKind::CurrentMatch => frame.current,
                };
                window.paint_quad(fill(mark_bounds, color));
                if mark.kind == MarkKind::CurrentMatch {
                    window.paint_quad(outline(mark_bounds, frame.ring, BorderStyle::Solid));
                }
            }
        }
    });
}
