use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::text_document::DocumentKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenId(u64);

enum Stop {
    Unavailable,
    Flag(Arc<AtomicBool>),
}

struct OpenLoad {
    id: OpenId,
    label: String,
    stop: Stop,
}

#[derive(Default)]
pub struct OpenLoads {
    next: u64,
    running: Vec<OpenLoad>,
}

impl OpenLoads {
    pub fn start(&mut self, label: String) -> OpenId {
        self.push(label, Stop::Unavailable)
    }

    pub fn start_cancellable(&mut self, label: String) -> (OpenId, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (self.push(label, Stop::Flag(flag.clone())), flag)
    }

    fn push(&mut self, label: String, stop: Stop) -> OpenId {
        let id = OpenId(self.next);
        self.next += 1;
        self.running.push(OpenLoad { id, label, stop });
        id
    }

    // A PDF opens in its own view, which shows its progress, so it has no pill to clear.
    pub fn start_for(&mut self, kind: DocumentKind, label: String) -> Option<OpenId> {
        (!kind.is_pdf()).then(|| self.start(label))
    }

    // False when the load was cancelled meanwhile, so its late result must be dropped.
    pub fn finish(&mut self, id: OpenId) -> bool {
        let before = self.running.len();
        self.running.retain(|load| load.id != id);
        self.running.len() != before
    }

    pub fn newest_cancellable(&self) -> bool {
        self.running
            .last()
            .is_some_and(|load| matches!(load.stop, Stop::Flag(_)))
    }

    // Only the load the indicator shows can be cancelled; the pill clears at once.
    pub fn cancel_newest(&mut self) -> bool {
        let Some(OpenLoad {
            stop: Stop::Flag(flag),
            ..
        }) = self.running.last()
        else {
            return false;
        };
        flag.store(true, Ordering::Relaxed);
        self.running.pop();
        true
    }

    pub fn newest_label(&self) -> Option<&str> {
        self.running.last().map(|load| load.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_indicator_stays_until_every_overlapping_load_ends() {
        let mut loads = OpenLoads::default();
        let large = loads.start("large".into());
        let heavy = loads.start("heavy".into());
        loads.finish(large);
        assert_eq!(loads.newest_label(), Some("heavy"));
        loads.finish(heavy);
        assert_eq!(loads.newest_label(), None);
    }

    #[test]
    fn finishing_in_reverse_order_keeps_the_older_load_visible() {
        let mut loads = OpenLoads::default();
        let first = loads.start("first".into());
        let second = loads.start("second".into());
        loads.finish(second);
        assert_eq!(loads.newest_label(), Some("first"));
        loads.finish(first);
        assert_eq!(loads.newest_label(), None);
    }

    #[test]
    fn a_failed_load_removes_only_its_own_entry() {
        let mut loads = OpenLoads::default();
        let failing = loads.start("failing".into());
        let running = loads.start("running".into());
        let newest = loads.start("newest".into());
        loads.finish(failing);
        assert_eq!(loads.newest_label(), Some("newest"));
        loads.finish(newest);
        assert_eq!(loads.newest_label(), Some("running"));
        loads.finish(running);
        assert_eq!(loads.newest_label(), None);
    }

    #[test]
    fn finishing_twice_does_not_touch_other_loads() {
        let mut loads = OpenLoads::default();
        let done = loads.start("done".into());
        let other = loads.start("other".into());
        loads.finish(done);
        loads.finish(done);
        assert_eq!(loads.newest_label(), Some("other"));
        loads.finish(other);
    }

    #[test]
    fn a_cancelled_load_is_discarded_when_its_result_arrives() {
        let mut loads = OpenLoads::default();
        let (id, flag) = loads.start_cancellable("big".into());
        assert!(loads.cancel_newest());
        assert!(flag.load(Ordering::Relaxed));
        assert_eq!(loads.newest_label(), None);
        assert!(!loads.finish(id));
    }

    #[test]
    fn a_load_that_was_not_cancelled_is_delivered() {
        let mut loads = OpenLoads::default();
        let (id, flag) = loads.start_cancellable("big".into());
        assert!(loads.finish(id));
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelling_leaves_an_older_load_running_and_untouched() {
        let mut loads = OpenLoads::default();
        let (older, older_flag) = loads.start_cancellable("older".into());
        let (_, newer_flag) = loads.start_cancellable("newer".into());
        assert!(loads.cancel_newest());
        assert!(newer_flag.load(Ordering::Relaxed));
        assert!(!older_flag.load(Ordering::Relaxed));
        assert_eq!(loads.newest_label(), Some("older"));
        assert!(loads.finish(older));
    }

    #[test]
    fn a_load_without_a_stop_cannot_be_cancelled() {
        let mut loads = OpenLoads::default();
        let (_, flag) = loads.start_cancellable("big".into());
        let reading = loads.start("reading".into());
        assert!(!loads.newest_cancellable());
        assert!(!loads.cancel_newest());
        assert!(!flag.load(Ordering::Relaxed));
        assert_eq!(loads.newest_label(), Some("reading"));
        assert!(loads.finish(reading));
        assert!(loads.newest_cancellable());
    }

    #[test]
    fn nothing_running_means_nothing_to_cancel() {
        let mut loads = OpenLoads::default();
        assert!(!loads.newest_cancellable());
        assert!(!loads.cancel_newest());
    }

    #[test]
    fn a_pdf_never_starts_a_pill() {
        let mut loads = OpenLoads::default();
        assert_eq!(loads.start_for(DocumentKind::Pdf, "a.pdf".into()), None);
        assert_eq!(loads.newest_label(), None);
    }

    #[test]
    fn a_text_pill_ends_when_its_open_ends_whether_it_loaded_or_failed() {
        let mut loads = OpenLoads::default();
        let loaded = loads.start_for(DocumentKind::Markdown, "a.md".into());
        assert_eq!(loads.newest_label(), Some("a.md"));
        loads.finish(loaded.unwrap());
        assert_eq!(loads.newest_label(), None);
        let failed = loads.start_for(DocumentKind::Markdown, "b.md".into());
        loads.finish(failed.unwrap());
        assert_eq!(loads.newest_label(), None);
    }
}
