const MAX_KEPT: usize = 512;

// The workbook that was active when each message of this session was sent. Trimming of the
// transcript drops its oldest messages, so entries are matched from the newest backwards.
#[derive(Debug, Default)]
pub(super) struct SentBooks(Vec<Option<String>>);

impl SentBooks {
    pub(super) fn record(&mut self, book: Option<String>) {
        self.0.push(book);
        if self.0.len() > MAX_KEPT {
            self.0.remove(0);
        }
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    // `rank` counts user messages from the oldest one in the transcript; `total` is how many there are.
    pub(super) fn of(&self, rank: usize, total: usize) -> Option<&str> {
        let from_end = total.checked_sub(rank)?;
        let index = self.0.len().checked_sub(from_end)?;
        self.0.get(index)?.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_sent_from_two_workbooks_keeps_both_labels() {
        let mut books = SentBooks::default();
        books.record(Some("a.xlsx".to_string()));
        books.record(Some("b.xlsx".to_string()));
        assert_eq!(books.of(0, 2), Some("a.xlsx"));
        assert_eq!(books.of(1, 2), Some("b.xlsx"));
    }

    #[test]
    fn restored_messages_before_the_session_have_no_workbook() {
        let mut books = SentBooks::default();
        books.record(Some("a.xlsx".to_string()));
        assert_eq!(books.of(0, 2), None);
        assert_eq!(books.of(1, 2), Some("a.xlsx"));
    }

    #[test]
    fn a_send_without_a_workbook_keeps_later_ones_aligned() {
        let mut books = SentBooks::default();
        books.record(None);
        books.record(Some("b.xlsx".to_string()));
        assert_eq!(books.of(0, 2), None);
        assert_eq!(books.of(1, 2), Some("b.xlsx"));
    }

    #[test]
    fn a_trimmed_transcript_still_matches_from_the_newest() {
        let mut books = SentBooks::default();
        for name in ["a", "b", "c"] {
            books.record(Some(name.to_string()));
        }
        assert_eq!(books.of(0, 2), Some("b"));
        assert_eq!(books.of(1, 2), Some("c"));
    }

    #[test]
    fn reset_clears_and_the_list_stays_bounded() {
        let mut books = SentBooks::default();
        for index in 0..MAX_KEPT + 10 {
            books.record(Some(index.to_string()));
        }
        assert_eq!(books.0.len(), MAX_KEPT);
        books.clear();
        assert_eq!(books.of(0, 1), None);
    }
}
