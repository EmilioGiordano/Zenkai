use pdfium_render::prelude::*;

use crate::wire::{
    CharBox, LinkTarget, MAX_CHARS, MAX_LINKS, PageContent, PageLink, PageRect, PageSize, Turn,
    page_within, to_page_rect, web_url,
};

struct Frame {
    origin: (f32, f32),
    shown: PageSize,
    turn: Turn,
}

fn frame_of(page: &PdfPage<'_>) -> Frame {
    let origin = page
        .boundaries()
        .crop()
        .or_else(|_| page.boundaries().media())
        .map_or((0.0, 0.0), |boundary| {
            (boundary.bounds.left().value, boundary.bounds.bottom().value)
        });
    let turn = match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => Turn::Quarter,
        Ok(PdfPageRenderRotation::Degrees180) => Turn::Half,
        Ok(PdfPageRenderRotation::Degrees270) => Turn::ThreeQuarters,
        Ok(PdfPageRenderRotation::None) | Err(_) => Turn::None,
    };
    Frame {
        origin,
        shown: PageSize {
            width: page.width().value,
            height: page.height().value,
        }
        .sanitized(),
        turn,
    }
}

fn rect_of(rect: &PdfRect, frame: &Frame) -> PageRect {
    let found = to_page_rect(
        (
            rect.left().value,
            rect.bottom().value,
            rect.right().value,
            rect.top().value,
        ),
        frame.origin,
        frame.shown,
        frame.turn,
    );
    found.sanitized()
}

// The text of a page as chars, for searching. None when the page has no text layer.
pub fn search_text(page: &PdfPage<'_>) -> Option<Vec<char>> {
    let text = page.text().ok()?;
    let chars: Vec<char> = text
        .chars()
        .iter()
        .take(MAX_CHARS)
        .map(|found| found.unicode_char().unwrap_or(' '))
        .collect();
    (!chars.is_empty()).then_some(chars)
}

pub fn read_content(page: &PdfPage<'_>, page_count: usize) -> PageContent {
    let frame = frame_of(page);
    let mut chars = page.text().map_or_else(
        |_| Vec::new(),
        |text| {
            text.chars()
                .iter()
                .take(MAX_CHARS + 1)
                .map(|found| CharBox {
                    ch: found.unicode_char().unwrap_or(' '),
                    rect: found
                        .loose_bounds()
                        .map_or_else(|_| PageRect::default(), |rect| rect_of(&rect, &frame)),
                })
                .collect()
        },
    );
    let truncated = chars.len() > MAX_CHARS;
    chars.truncate(MAX_CHARS);
    PageContent {
        chars,
        links: read_links(page, &frame, page_count),
        truncated,
    }
}

fn read_links(page: &PdfPage<'_>, frame: &Frame, page_count: usize) -> Vec<PageLink> {
    page.links()
        .iter()
        .take(MAX_LINKS)
        .filter_map(|link| {
            let rect = rect_of(&link.rect().ok()?, frame);
            let target = link_target(&link, page_count)?;
            Some(PageLink { rect, target })
        })
        .collect()
}

// Only a jump inside the document or a web address is honoured. Launching a program, opening
// another file or running a script is dropped, whatever the file asks.
fn link_target(link: &PdfLink<'_>, page_count: usize) -> Option<LinkTarget> {
    match link.action() {
        Some(PdfAction::Uri(action)) => action
            .uri()
            .ok()
            .and_then(|uri| web_url(&uri))
            .map(LinkTarget::Web),
        Some(PdfAction::LocalDestination(action)) => {
            page_target(&action.destination().ok()?, page_count)
        }
        Some(_) => None,
        None => page_target(&link.destination()?, page_count),
    }
}

fn page_target(destination: &PdfDestination<'_>, page_count: usize) -> Option<LinkTarget> {
    let page = page_within(i64::from(destination.page_index().ok()?), page_count)?;
    Some(LinkTarget::Page(page))
}
