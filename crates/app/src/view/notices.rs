use std::collections::VecDeque;
use std::time::Duration;

use gpui_kit::SharedString;

const HISTORY_LIMIT: usize = 20;
pub(super) const WARNING_LIFETIME: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Expiry {
    Closed,
    Held,
    Gone,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Lifetime {
    UntilReplaced,
    UntilOpen,
}

#[derive(Clone, Debug)]
pub(super) struct Notice {
    id: u64,
    pub severity: Severity,
    pub text: SharedString,
    detail: Option<SharedString>,
    pub lifetime: Lifetime,
    pub time: SharedString,
}

impl Notice {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn detail(&self) -> Option<&SharedString> {
        self.detail.as_ref()
    }

    pub fn copy_text(&self) -> String {
        match &self.detail {
            Some(detail) => format!(
                "{}
{detail}",
                self.text
            ),
            None => self.text.to_string(),
        }
    }
}

// Info goes to the status bar only. A warning or an error also gets a card that stays
// until it is dismissed or replaced, so an unread problem survives a later "Saved".
#[derive(Default)]
pub(super) struct Notices {
    status: Option<Notice>,
    card: Option<Notice>,
    history: VecDeque<Notice>,
    next_id: u64,
}

impl Notices {
    pub fn post(
        &mut self,
        severity: Severity,
        text: SharedString,
        detail: Option<SharedString>,
        lifetime: Lifetime,
        time: SharedString,
    ) -> u64 {
        let id = self.next_id;
        let notice = Notice {
            id,
            severity,
            text,
            detail,
            lifetime,
            time,
        };
        self.next_id += 1;
        if self.history.len() == HISTORY_LIMIT {
            self.history.pop_front();
        }
        self.history.push_back(notice.clone());
        if severity != Severity::Info {
            self.card = Some(notice.clone());
        }
        self.status = Some(notice);
        id
    }

    pub fn status(&self) -> Option<&Notice> {
        self.status.as_ref()
    }

    pub fn card(&self) -> Option<&Notice> {
        self.card.as_ref()
    }

    pub fn newest_first(&self) -> impl Iterator<Item = &Notice> {
        self.history.iter().rev()
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn index_of(&self, id: Option<u64>) -> usize {
        id.and_then(|id| self.newest_first().position(|notice| notice.id == id))
            .unwrap_or(0)
    }

    pub fn id_at(&self, index: usize) -> Option<u64> {
        self.newest_first().nth(index).map(Notice::id)
    }

    pub fn dismiss_card(&mut self) {
        let Some(card) = self.card.take() else {
            return;
        };
        if self
            .status
            .as_ref()
            .is_some_and(|status| status.id == card.id)
        {
            self.status = None;
        }
    }

    // Only a warning times out, and never while the user is reading it.
    pub fn expire_warning(&mut self, id: u64, held: bool) -> Expiry {
        let is_that_warning = self
            .card
            .as_ref()
            .is_some_and(|card| card.id == id && card.severity == Severity::Warning);
        if !is_that_warning {
            return Expiry::Gone;
        }
        if held {
            return Expiry::Held;
        }
        self.dismiss_card();
        Expiry::Closed
    }

    // The history is kept: it is what the Messages panel shows.
    pub fn clear_on_open(&mut self) {
        let until_open = |notice: &mut Option<Notice>| {
            if notice
                .as_ref()
                .is_some_and(|notice| notice.lifetime == Lifetime::UntilOpen)
            {
                *notice = None;
            }
        };
        until_open(&mut self.status);
        until_open(&mut self.card);
    }
}

#[cfg(test)]
mod tests {
    use super::{Expiry, HISTORY_LIMIT, Lifetime, Notices, Severity};

    fn post(notices: &mut Notices, severity: Severity, text: &str, lifetime: Lifetime) -> u64 {
        notices.post(
            severity,
            text.to_string().into(),
            None,
            lifetime,
            "12:00:00".into(),
        )
    }

    fn texts(notices: &Notices) -> Vec<String> {
        notices.newest_first().map(|n| n.text.to_string()).collect()
    }

    #[test]
    fn a_notice_copies_its_detail_when_it_has_one_and_its_text_otherwise() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Error,
            "Short",
            Lifetime::UntilReplaced,
        );
        notices.post(
            Severity::Error,
            "Could not save a.xlsx".into(),
            Some("a.xlsx: Access denied (os error 5)".into()),
            Lifetime::UntilReplaced,
            "12:00:00".into(),
        );
        let copied: Vec<_> = notices.newest_first().map(|n| n.copy_text()).collect();
        assert_eq!(
            copied,
            [
                "Could not save a.xlsx
a.xlsx: Access denied (os error 5)",
                "Short"
            ]
        );
    }

    #[test]
    fn a_warning_card_closes_when_its_time_is_up() {
        let mut notices = Notices::default();
        let id = post(
            &mut notices,
            Severity::Warning,
            "Careful",
            Lifetime::UntilReplaced,
        );
        assert_eq!(notices.expire_warning(id, false), Expiry::Closed);
        assert!(notices.card().is_none());
        assert!(notices.status().is_none());
        assert_eq!(notices.len(), 1);
    }

    #[test]
    fn a_warning_card_that_is_hovered_or_focused_stays() {
        let mut notices = Notices::default();
        let id = post(
            &mut notices,
            Severity::Warning,
            "Careful",
            Lifetime::UntilReplaced,
        );
        assert_eq!(notices.expire_warning(id, true), Expiry::Held);
        assert!(notices.card().is_some());
    }

    #[test]
    fn an_error_card_never_times_out() {
        let mut notices = Notices::default();
        let id = post(
            &mut notices,
            Severity::Error,
            "Disk full",
            Lifetime::UntilReplaced,
        );
        assert_eq!(notices.expire_warning(id, false), Expiry::Gone);
        assert!(notices.card().is_some());
    }

    #[test]
    fn the_timer_of_a_replaced_warning_leaves_the_newer_card_alone() {
        let mut notices = Notices::default();
        let old = post(
            &mut notices,
            Severity::Warning,
            "First",
            Lifetime::UntilReplaced,
        );
        post(
            &mut notices,
            Severity::Warning,
            "Second",
            Lifetime::UntilReplaced,
        );
        assert_eq!(notices.expire_warning(old, false), Expiry::Gone);
        assert_eq!(
            notices.card().map(|n| n.text.to_string()).as_deref(),
            Some("Second")
        );
    }

    #[test]
    fn the_warning_lifetime_is_eight_seconds() {
        assert_eq!(super::WARNING_LIFETIME.as_secs(), 8);
    }

    #[test]
    fn the_history_lists_the_newest_first() {
        let mut notices = Notices::default();
        for text in ["one", "two", "three"] {
            post(&mut notices, Severity::Info, text, Lifetime::UntilReplaced);
        }
        assert_eq!(texts(&notices), ["three", "two", "one"]);
    }

    #[test]
    fn the_history_keeps_only_the_last_twenty() {
        let mut notices = Notices::default();
        for index in 0..HISTORY_LIMIT + 5 {
            post(
                &mut notices,
                Severity::Info,
                &index.to_string(),
                Lifetime::UntilReplaced,
            );
        }
        let kept = texts(&notices);
        assert_eq!(kept.len(), HISTORY_LIMIT);
        assert_eq!(kept.first().map(String::as_str), Some("24"));
        assert_eq!(kept.last().map(String::as_str), Some("5"));
    }

    #[test]
    fn info_goes_to_the_status_bar_without_a_card() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Info,
            "Saved",
            Lifetime::UntilReplaced,
        );
        assert!(notices.card().is_none());
        assert_eq!(
            notices.status().map(|n| n.text.to_string()).as_deref(),
            Some("Saved")
        );
    }

    #[test]
    fn info_does_not_close_an_open_card() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Error,
            "Disk full",
            Lifetime::UntilReplaced,
        );
        post(
            &mut notices,
            Severity::Info,
            "Saved",
            Lifetime::UntilReplaced,
        );
        assert_eq!(
            notices.card().map(|n| n.text.to_string()).as_deref(),
            Some("Disk full")
        );
        assert_eq!(
            notices.status().map(|n| n.text.to_string()).as_deref(),
            Some("Saved")
        );
    }

    #[test]
    fn a_warning_replaces_an_error_card() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Error,
            "Disk full",
            Lifetime::UntilReplaced,
        );
        post(
            &mut notices,
            Severity::Warning,
            "Read-only",
            Lifetime::UntilReplaced,
        );
        let card = notices.card().unwrap();
        assert_eq!(card.severity, Severity::Warning);
        assert_eq!(card.text.as_ref(), "Read-only");
    }

    #[test]
    fn dismissing_the_card_clears_its_status_line_but_not_the_history() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Error,
            "Disk full",
            Lifetime::UntilReplaced,
        );
        notices.dismiss_card();
        assert!(notices.card().is_none());
        assert!(notices.status().is_none());
        assert_eq!(notices.len(), 1);
    }

    #[test]
    fn dismissing_the_card_keeps_a_newer_status_line() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Error,
            "Disk full",
            Lifetime::UntilReplaced,
        );
        post(
            &mut notices,
            Severity::Info,
            "Saved",
            Lifetime::UntilReplaced,
        );
        notices.dismiss_card();
        assert_eq!(
            notices.status().map(|n| n.text.to_string()).as_deref(),
            Some("Saved")
        );
    }

    #[test]
    fn opening_a_file_clears_what_lasts_until_then_and_keeps_the_history() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Warning,
            "Unsupported",
            Lifetime::UntilOpen,
        );
        notices.clear_on_open();
        assert!(notices.card().is_none());
        assert!(notices.status().is_none());
        assert_eq!(notices.len(), 1);
    }

    #[test]
    fn opening_a_file_keeps_what_lasts_until_replaced() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Error,
            "Disk full",
            Lifetime::UntilReplaced,
        );
        notices.clear_on_open();
        assert!(notices.card().is_some());
        assert!(notices.status().is_some());
    }

    #[test]
    fn the_selection_follows_its_notice_when_newer_ones_arrive() {
        let mut notices = Notices::default();
        post(&mut notices, Severity::Info, "a", Lifetime::UntilReplaced);
        post(&mut notices, Severity::Info, "b", Lifetime::UntilReplaced);
        let selected = notices.id_at(1);
        post(&mut notices, Severity::Info, "c", Lifetime::UntilReplaced);
        assert_eq!(notices.index_of(selected), 2);
    }

    #[test]
    fn a_selection_dropped_by_the_cap_falls_back_to_the_newest() {
        let mut notices = Notices::default();
        post(
            &mut notices,
            Severity::Info,
            "first",
            Lifetime::UntilReplaced,
        );
        let selected = notices.id_at(0);
        for index in 0..HISTORY_LIMIT {
            post(
                &mut notices,
                Severity::Info,
                &index.to_string(),
                Lifetime::UntilReplaced,
            );
        }
        assert_eq!(notices.index_of(selected), 0);
    }
}
