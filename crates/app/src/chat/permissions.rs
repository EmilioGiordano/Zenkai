use gpui_kit::*;
use zenkai_agent::chat::session::{ChoiceKind, PermissionChoice};
use zenkai_agent::chat::thread::ToolCallId;

use super::{ChatPanel, View};

fn shortcut_choice(choices: &[PermissionChoice], allow: bool) -> Option<usize> {
    let find = |kind: ChoiceKind| choices.iter().position(|choice| choice.kind == kind);
    if allow {
        find(ChoiceKind::AllowOnce)
    } else {
        find(ChoiceKind::RejectOnce).or_else(|| find(ChoiceKind::RejectAlways))
    }
}

impl ChatPanel {
    pub(super) fn answer(&mut self, tool: &ToolCallId, choice: usize, cx: &mut Context<Self>) {
        let Some(position) = self.permissions.iter().position(|ask| &ask.tool == tool) else {
            return;
        };
        let ask = self.permissions.remove(position);
        if let Some(chosen) = ask.choices.get(choice).cloned() {
            self.thread
                .permission_answered(&ask.tool, chosen.kind.allows());
            ask.choose(&chosen);
        } else {
            ask.cancel();
        }
        cx.notify();
    }

    // The keyboard path is deliberately narrow: only a one-time allow, so a shortcut can never
    // grant a standing permission. Denying prefers a one-time reject, then the standing one.
    pub(super) fn answer_first(&mut self, allow: bool, cx: &mut Context<Self>) {
        let Some(ask) = self.permissions.first() else {
            return;
        };
        let choice = shortcut_choice(&ask.choices, allow);
        let tool = ask.tool.clone();
        match choice {
            Some(choice) => self.answer(&tool, choice, cx),
            None if !allow => {
                let ask = self.permissions.remove(0);
                self.thread.permission_answered(&ask.tool, false);
                ask.cancel();
                cx.notify();
            }
            None => {}
        }
    }

    // Whether the buttons of a pending question are on screen.
    pub(crate) fn permission_visible(&self) -> bool {
        self.view == View::Chat && !self.permissions.is_empty()
    }

    pub(crate) fn has_pending_permission(&self) -> bool {
        !self.permissions.is_empty()
    }

    pub(crate) fn allow_permission(&mut self, cx: &mut Context<Self>) {
        self.answer_first(true, cx);
    }

    pub(crate) fn deny_permission(&mut self, cx: &mut Context<Self>) {
        self.answer_first(false, cx);
    }
}

#[cfg(test)]
mod tests {
    use zenkai_agent::chat::session::{ChoiceKind, PermissionChoice};

    use super::shortcut_choice;

    fn choices(kinds: &[ChoiceKind]) -> Vec<PermissionChoice> {
        kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| PermissionChoice {
                id: index.to_string(),
                label: String::new(),
                kind: *kind,
            })
            .collect()
    }

    #[test]
    fn the_allow_shortcut_never_picks_a_standing_permission() {
        let offered = choices(&[ChoiceKind::AllowAlways, ChoiceKind::AllowOnce]);
        assert_eq!(shortcut_choice(&offered, true), Some(1));
        let only_always = choices(&[ChoiceKind::AllowAlways, ChoiceKind::RejectOnce]);
        assert_eq!(shortcut_choice(&only_always, true), None);
    }

    #[test]
    fn the_deny_shortcut_prefers_a_one_time_reject() {
        let offered = choices(&[ChoiceKind::RejectAlways, ChoiceKind::RejectOnce]);
        assert_eq!(shortcut_choice(&offered, false), Some(1));
        let only_always = choices(&[ChoiceKind::AllowOnce, ChoiceKind::RejectAlways]);
        assert_eq!(shortcut_choice(&only_always, false), Some(1));
        assert_eq!(
            shortcut_choice(&choices(&[ChoiceKind::AllowOnce]), false),
            None
        );
    }
}
