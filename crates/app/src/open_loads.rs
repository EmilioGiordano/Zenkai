use crate::text_document::DocumentKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenId(u64);

struct OpenLoad {
    id: OpenId,
    label: String,
}

#[derive(Default)]
pub struct OpenLoads {
    next: u64,
    running: Vec<OpenLoad>,
}

impl OpenLoads {
    pub fn start(&mut self, label: String) -> OpenId {
        let id = OpenId(self.next);
        self.next += 1;
        self.running.push(OpenLoad { id, label });
        id
    }

    // A PDF opens in its own view, which shows its progress, so it has no pill to clear.
    pub fn start_for(&mut self, kind: DocumentKind, label: String) -> Option<OpenId> {
        (!kind.is_pdf()).then(|| self.start(label))
    }

    pub fn finish(&mut self, id: OpenId) {
        self.running.retain(|load| load.id != id);
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
