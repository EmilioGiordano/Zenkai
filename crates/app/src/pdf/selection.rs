use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use super::layout::PageIdx;
use super::protocol::{CharBox, PageContent, PageRect};

// Characters pdfium inserts to mark soft hyphens and line breaks it generated.
const SOFT_HYPHEN_MARK: char = '\u{2}';
const LINE_HEIGHT_SHARE: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub page: PageIdx,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextPos,
    pub head: TextPos,
}

impl Selection {
    pub fn ordered(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn pages(&self) -> RangeInclusive<usize> {
        let (start, end) = self.ordered();
        start.page.0..=end.page.0
    }

    // The chars of `page` the selection covers, both ends included.
    pub fn span_on(&self, page: PageIdx, char_count: usize) -> Option<RangeInclusive<usize>> {
        let (start, end) = self.ordered();
        if char_count == 0 || page < start.page || page > end.page {
            return None;
        }
        let first = if page == start.page { start.index } else { 0 };
        let last = if page == end.page {
            end.index
        } else {
            char_count - 1
        };
        let last = last.min(char_count - 1);
        (first <= last).then_some(first..=last)
    }
}

// The char nearest to a point in page points, so a drag past the end of a line still lands
// on that line's last char. None only for a page with no chars.
pub fn char_at(chars: &[CharBox], x: f32, y: f32) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .filter(|(_, found)| !found.rect.is_empty())
        .map(|(index, found)| (index, distance_to(&found.rect, x, y)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

fn distance_to(rect: &PageRect, x: f32, y: f32) -> f32 {
    let dx = (rect.left - x).max(x - rect.right).max(0.0);
    let dy = (rect.top - y).max(y - rect.bottom).max(0.0);
    dx * dx + dy * dy * 4.0
}

// One rectangle per line of the chars in `span`, so a highlight is a few bars and not a box
// per letter.
pub fn line_rects(chars: &[CharBox], span: RangeInclusive<usize>) -> Vec<PageRect> {
    let mut lines: Vec<PageRect> = Vec::new();
    let end = (*span.end()).min(chars.len().saturating_sub(1));
    for found in chars.iter().take(end + 1).skip(*span.start()) {
        if found.rect.is_empty() {
            continue;
        }
        match lines.last_mut() {
            Some(line) if same_line(line, &found.rect) => {
                line.left = line.left.min(found.rect.left);
                line.right = line.right.max(found.rect.right);
                line.top = line.top.min(found.rect.top);
                line.bottom = line.bottom.max(found.rect.bottom);
            }
            _ => lines.push(found.rect),
        }
    }
    lines
}

fn same_line(line: &PageRect, next: &PageRect) -> bool {
    let overlap = line.bottom.min(next.bottom) - line.top.max(next.top);
    let height = (line.bottom - line.top).min(next.bottom - next.top);
    overlap >= height * LINE_HEIGHT_SHARE
}

pub fn selected_text(
    selection: &Selection,
    contents: &HashMap<PageIdx, Arc<PageContent>>,
) -> String {
    let mut text = String::new();
    for page in selection.pages() {
        let Some(content) = contents.get(&PageIdx(page)) else {
            continue;
        };
        let Some(span) = selection.span_on(PageIdx(page), content.chars.len()) else {
            continue;
        };
        if !text.is_empty() {
            text.push_str("\r\n");
        }
        text.extend(
            content.chars[span]
                .iter()
                .map(|found| found.ch)
                .filter(|ch| *ch != SOFT_HYPHEN_MARK),
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(ch: char, column: f32, line: f32) -> CharBox {
        CharBox {
            ch,
            rect: PageRect {
                left: column * 10.0,
                right: column * 10.0 + 10.0,
                top: line * 20.0,
                bottom: line * 20.0 + 14.0,
            },
        }
    }

    fn word(text: &str, line: f32) -> Vec<CharBox> {
        text.chars()
            .enumerate()
            .map(|(column, ch)| letter(ch, column as f32, line))
            .collect()
    }

    fn pos(page: usize, index: usize) -> TextPos {
        TextPos {
            page: PageIdx(page),
            index,
        }
    }

    #[test]
    fn a_point_inside_a_char_picks_it() {
        let chars = word("abc", 0.0);
        assert_eq!(char_at(&chars, 15.0, 5.0), Some(1));
    }

    #[test]
    fn a_point_past_the_end_of_a_line_picks_its_last_char() {
        let mut chars = word("abc", 0.0);
        chars.extend(word("def", 1.0));
        assert_eq!(char_at(&chars, 400.0, 5.0), Some(2));
        assert_eq!(char_at(&chars, 400.0, 25.0), Some(5));
    }

    #[test]
    fn a_page_without_chars_has_no_char_to_pick() {
        assert_eq!(char_at(&[], 1.0, 1.0), None);
        let empty = [CharBox {
            ch: '\n',
            rect: PageRect::default(),
        }];
        assert_eq!(char_at(&empty, 1.0, 1.0), None);
    }

    #[test]
    fn the_chars_of_a_line_merge_into_one_bar() {
        let mut chars = word("abc", 0.0);
        chars.extend(word("def", 1.0));
        let bars = line_rects(&chars, 1..=4);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].left, 10.0);
        assert_eq!(bars[0].right, 30.0);
        assert_eq!(bars[1].left, 0.0);
        assert_eq!(bars[1].right, 20.0);
    }

    #[test]
    fn zero_sized_chars_are_skipped_in_a_highlight() {
        let mut chars = word("ab", 0.0);
        chars.push(CharBox {
            ch: '\n',
            rect: PageRect::default(),
        });
        chars.extend(word("cd", 1.0));
        assert_eq!(line_rects(&chars, 0..=4).len(), 2);
    }

    #[test]
    fn a_selection_is_the_same_whichever_way_it_was_dragged() {
        let forward = Selection {
            anchor: pos(0, 2),
            head: pos(1, 1),
        };
        let backward = Selection {
            anchor: pos(1, 1),
            head: pos(0, 2),
        };
        assert_eq!(forward.ordered(), backward.ordered());
        assert_eq!(forward.pages(), 0..=1);
    }

    #[test]
    fn the_span_on_each_page_follows_the_ends() {
        let selection = Selection {
            anchor: pos(0, 3),
            head: pos(2, 1),
        };
        assert_eq!(selection.span_on(PageIdx(0), 10), Some(3..=9));
        assert_eq!(selection.span_on(PageIdx(1), 10), Some(0..=9));
        assert_eq!(selection.span_on(PageIdx(2), 10), Some(0..=1));
        assert_eq!(selection.span_on(PageIdx(3), 10), None);
        assert_eq!(selection.span_on(PageIdx(1), 0), None);
    }

    #[test]
    fn selected_text_joins_pages_and_drops_hyphen_marks() {
        let mut contents = HashMap::new();
        contents.insert(
            PageIdx(0),
            Arc::new(PageContent {
                chars: word("ab\u{2}c", 0.0),
                links: Vec::new(),
            }),
        );
        contents.insert(
            PageIdx(1),
            Arc::new(PageContent {
                chars: word("xyz", 0.0),
                links: Vec::new(),
            }),
        );
        let selection = Selection {
            anchor: pos(0, 1),
            head: pos(1, 1),
        };
        assert_eq!(selected_text(&selection, &contents), "bc\r\nxy");
    }
}
