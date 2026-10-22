use pdfium_render::prelude::*;

use crate::wire::{
    MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_TITLE_CHARS, OutlineItem, PageIdx, page_within,
};

const MAX_VISITED: usize = 20_000;

// Bookmarks that point outside the document are left out, so the list always passes the app's
// checks.
pub fn read_outline(document: &PdfDocument<'_>, page_count: usize) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut visited = 0;
    if let Some(first) = document.bookmarks().root() {
        walk(first, 0, page_count, &mut items, &mut visited);
    }
    items
}

// The bookmark tree comes from the file, so its depth and the nodes visited are capped; a loop
// in it, or a long chain of bookmarks that are skipped, ends at the visit limit.
fn walk(
    first: PdfBookmark<'_>,
    depth: u8,
    page_count: usize,
    items: &mut Vec<OutlineItem>,
    visited: &mut usize,
) {
    let mut next = Some(first);
    while let Some(bookmark) = next {
        *visited += 1;
        if items.len() >= MAX_OUTLINE_ITEMS || *visited > MAX_VISITED {
            return;
        }
        if let (Some(title), Some(page)) = (bookmark.title(), page_of(&bookmark, page_count)) {
            items.push(OutlineItem {
                title: title.chars().take(MAX_TITLE_CHARS).collect(),
                depth,
                page,
            });
        }
        if depth < MAX_OUTLINE_DEPTH
            && let Some(child) = bookmark.first_child()
        {
            walk(child, depth + 1, page_count, items, visited);
        }
        next = bookmark.next_sibling();
    }
}

fn page_of(bookmark: &PdfBookmark<'_>, page_count: usize) -> Option<PageIdx> {
    let index = match bookmark.destination() {
        Some(destination) => destination.page_index().ok()?,
        None => match bookmark.action()? {
            PdfAction::LocalDestination(action) => action.destination().ok()?.page_index().ok()?,
            _ => return None,
        },
    };
    page_within(i64::from(index), page_count)
}
