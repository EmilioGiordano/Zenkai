use std::time::Duration;

use gpui_kit::*;
use zenkai_agent::chat::thread::{Entry, NoticeKind, TurnEnd, TurnState};
use zenkai_i18n::t;

use super::{ChatPanel, Link, Start, closed};

const STOP_GRACE: Duration = Duration::from_secs(5);

fn stop_overdue(turn: TurnState, stopped_epoch: u64, epoch: u64) -> bool {
    turn == TurnState::Cancelling && stopped_epoch == epoch
}

pub(super) fn last_prompt(entries: &[Entry]) -> Option<String> {
    entries.iter().rev().find_map(|entry| match entry {
        Entry::User(text) => Some(text.clone()),
        _ => None,
    })
}

impl ChatPanel {
    pub(crate) fn retry_turn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.failed_prompt.clone() else {
            return;
        };
        if !self.thread.submit(text.clone()) {
            return;
        }
        let book = self.active_workbook(cx).map(|(_, name)| name);
        self.sent_books.record(book);
        let context = self.prompt_context(cx);
        self.start_turn(text, context, window, cx);
    }

    pub(super) fn watch_stop(&self, cx: &mut Context<Self>) {
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STOP_GRACE).await;
            closed(this.update(cx, |this, cx| this.end_overdue_stop(epoch, cx)));
        })
        .detach();
    }

    pub(super) fn reloadable_session(&self) -> Option<String> {
        self.live_session
            .clone()
            .filter(|_| self.state.can_resume())
    }

    pub(crate) fn can_restart_agent(&self) -> bool {
        self.live.is_none()
            && self.link == Link::Idle
            && (self.failed_prompt.is_some() || self.restart_session.is_some())
    }

    pub(crate) fn restart_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_restart_agent() {
            return;
        }
        self.problem = None;
        self.begin(Start::Switch, window, cx);
        cx.notify();
    }

    fn end_overdue_stop(&mut self, stopped_epoch: u64, cx: &mut Context<Self>) {
        if !stop_overdue(self.thread.turn(), stopped_epoch, self.epoch) {
            return;
        }
        if let Some(since) = self.turn_began.take() {
            self.list_files_written(since, cx);
        }
        self.epoch += 1;
        self.restart_session = self.reloadable_session();
        self.end_session(cx);
        self.finish_turn(TurnEnd::Cancelled, cx);
        self.thread
            .notice(NoticeKind::Info, t!("chat.stop_timeout"));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{last_prompt, stop_overdue};
    use zenkai_agent::chat::thread::TurnState;
    use zenkai_agent::chat::thread::{Entry, NoticeKind};

    #[test]
    fn a_stop_the_agent_never_answered_is_overdue() {
        assert!(stop_overdue(TurnState::Cancelling, 3, 3));
    }

    #[test]
    fn a_stop_the_agent_answered_is_not_overdue() {
        assert!(!stop_overdue(TurnState::Idle, 3, 3));
    }

    #[test]
    fn a_stop_from_an_earlier_conversation_is_not_overdue() {
        assert!(!stop_overdue(TurnState::Cancelling, 3, 4));
    }

    #[test]
    fn the_prompt_to_retry_is_the_last_one_the_user_sent() {
        let entries = [
            Entry::User("first".to_string()),
            Entry::User("second".to_string()),
            Entry::Notice {
                kind: NoticeKind::Error,
                text: "failed".to_string(),
            },
        ];
        assert_eq!(last_prompt(&entries), Some("second".to_string()));
    }

    #[test]
    fn without_a_user_message_there_is_nothing_to_retry() {
        assert_eq!(last_prompt(&[]), None);
    }
}
