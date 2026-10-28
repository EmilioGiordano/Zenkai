use std::time::Duration;

use gpui_kit::*;
use zenkai_agent::chat::thread::{NoticeKind, TurnEnd, TurnState};
use zenkai_i18n::t;

use super::{ChatPanel, closed};

const STOP_GRACE: Duration = Duration::from_secs(5);

fn stop_overdue(turn: TurnState, stopped_epoch: u64, epoch: u64) -> bool {
    turn == TurnState::Cancelling && stopped_epoch == epoch
}

impl ChatPanel {
    pub(super) fn watch_stop(&self, cx: &mut Context<Self>) {
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STOP_GRACE).await;
            closed(this.update(cx, |this, cx| this.end_overdue_stop(epoch, cx)));
        })
        .detach();
    }

    fn end_overdue_stop(&mut self, stopped_epoch: u64, cx: &mut Context<Self>) {
        if !stop_overdue(self.thread.turn(), stopped_epoch, self.epoch) {
            return;
        }
        if let Some(since) = self.turn_began.take() {
            self.list_files_written(since, cx);
        }
        self.epoch += 1;
        self.end_session(cx);
        self.finish_turn(TurnEnd::Cancelled, cx);
        self.thread
            .notice(NoticeKind::Info, t!("chat.stop_timeout"));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::stop_overdue;
    use zenkai_agent::chat::thread::TurnState;

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
}
