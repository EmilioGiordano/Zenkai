use super::layout::PageIdx;

pub use zenkai_pdf_host::wire::{FindMatch, MAX_MATCHES};

// The state of one search: the matches found so far, in page order, and the one in focus.
#[derive(Clone, Debug, Default)]
pub struct FindResults {
    query: String,
    matches: Vec<FindMatch>,
    current: Option<usize>,
    complete: bool,
}

impl FindResults {
    pub fn new(query: &str) -> FindResults {
        FindResults {
            query: query.to_string(),
            matches: Vec::new(),
            current: None,
            complete: false,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn finish(&mut self) {
        self.complete = true;
    }

    pub fn is_full(&self) -> bool {
        self.matches.len() >= MAX_MATCHES
    }

    // Adds matches found in a chunk of pages. The first match at or after `from_page` takes
    // focus if nothing has it yet.
    pub fn extend(&mut self, found: impl IntoIterator<Item = FindMatch>, from_page: PageIdx) {
        let room = MAX_MATCHES.saturating_sub(self.matches.len());
        self.matches.extend(found.into_iter().take(room));
        if self.current.is_none() {
            self.current = self
                .matches
                .iter()
                .position(|found| found.page >= from_page)
                .or(if self.complete || self.is_full() {
                    Some(0).filter(|_| !self.matches.is_empty())
                } else {
                    None
                });
        }
    }

    pub fn current(&self) -> Option<FindMatch> {
        self.matches.get(self.current?).copied()
    }

    // "k of n": one-based position of the focused match and the total.
    pub fn position(&self) -> (usize, usize) {
        (
            self.current.map_or(0, |index| index + 1),
            self.matches.len(),
        )
    }

    pub fn step(&mut self, forward: bool) -> Option<FindMatch> {
        let count = self.matches.len();
        if count == 0 {
            return None;
        }
        let next = match (self.current, forward) {
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        self.current = Some(next);
        self.current()
    }

    pub fn on_page(&self, page: PageIdx) -> impl Iterator<Item = (bool, FindMatch)> + '_ {
        let current = self.current;
        let start = self.matches.partition_point(|found| found.page < page);
        self.matches[start..]
            .iter()
            .enumerate()
            .take_while(move |(_, found)| found.page == page)
            .map(move |(offset, found)| (current == Some(start + offset), *found))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_pdf_host::wire::CharSpan;

    fn found(page: usize, start: usize) -> FindMatch {
        FindMatch {
            page: PageIdx(page),
            span: CharSpan { start, len: 1 },
        }
    }

    #[test]
    fn the_first_match_from_the_current_page_takes_focus() {
        let mut results = FindResults::new("x");
        results.extend([found(0, 1), found(2, 4), found(3, 0)], PageIdx(1));
        assert_eq!(results.current(), Some(found(2, 4)));
        assert_eq!(results.position(), (2, 3));
    }

    #[test]
    fn with_nothing_after_the_page_focus_waits_until_the_search_ends_then_wraps() {
        let mut results = FindResults::new("x");
        results.extend([found(0, 1)], PageIdx(2));
        assert_eq!(results.current(), None);
        results.finish();
        results.extend([], PageIdx(2));
        assert_eq!(results.current(), Some(found(0, 1)));
    }

    #[test]
    fn stepping_wraps_both_ways() {
        let mut results = FindResults::new("x");
        results.extend([found(0, 0), found(1, 0), found(2, 0)], PageIdx(0));
        assert_eq!(results.step(true), Some(found(1, 0)));
        assert_eq!(results.step(true), Some(found(2, 0)));
        assert_eq!(results.step(true), Some(found(0, 0)));
        assert_eq!(results.step(false), Some(found(2, 0)));
    }

    #[test]
    fn stepping_without_matches_does_nothing() {
        let mut results = FindResults::new("x");
        assert_eq!(results.step(true), None);
        assert_eq!(results.position(), (0, 0));
    }

    #[test]
    fn the_matches_of_a_page_say_which_one_has_focus() {
        let mut results = FindResults::new("x");
        results.extend(
            [found(0, 0), found(1, 3), found(1, 8), found(2, 0)],
            PageIdx(1),
        );
        let page_one: Vec<(bool, FindMatch)> = results.on_page(PageIdx(1)).collect();
        assert_eq!(page_one, [(true, found(1, 3)), (false, found(1, 8))]);
        assert_eq!(results.on_page(PageIdx(5)).count(), 0);
    }

    #[test]
    fn the_matches_kept_are_capped() {
        let mut results = FindResults::new("x");
        results.extend((0..MAX_MATCHES + 10).map(|n| found(n, 0)), PageIdx(0));
        assert_eq!(results.position().1, MAX_MATCHES);
        assert!(results.is_full());
    }
}
