use pdfium_render::prelude::*;

use super::layout::PageIdx;
use super::protocol::OutlineItem;

const MAX_ITEMS: usize = 2000;
const MAX_DEPTH: u8 = 8;
const MAX_TITLE_CHARS: usize = 200;

pub fn read_outline(document: &PdfDocument<'_>) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    if let Some(first) = document.bookmarks().root() {
        walk(first, 0, &mut items);
    }
    items
}

// The bookmark tree comes from the file, so its size and depth are capped; a loop in it ends
// at the item limit.
fn walk(first: PdfBookmark<'_>, depth: u8, items: &mut Vec<OutlineItem>) {
    let mut next = Some(first);
    while let Some(bookmark) = next {
        if items.len() >= MAX_ITEMS {
            return;
        }
        if let (Some(title), Some(page)) = (bookmark.title(), page_of(&bookmark)) {
            items.push(OutlineItem {
                title: title.chars().take(MAX_TITLE_CHARS).collect(),
                depth,
                page,
            });
        }
        if depth < MAX_DEPTH
            && let Some(child) = bookmark.first_child()
        {
            walk(child, depth + 1, items);
        }
        next = bookmark.next_sibling();
    }
}

fn page_of(bookmark: &PdfBookmark<'_>) -> Option<PageIdx> {
    let index = match bookmark.destination() {
        Some(destination) => destination.page_index().ok()?,
        None => match bookmark.action()? {
            PdfAction::LocalDestination(action) => action.destination().ok()?.page_index().ok()?,
            _ => return None,
        },
    };
    Some(PageIdx(usize::try_from(index).ok()?))
}
