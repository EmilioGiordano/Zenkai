use gpui_kit::*;
use zenkai_agent::chat::session::ChoiceKind;
use zenkai_agent::chat::thread::ToolCallId;

use super::ChatPanel;

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

    // The keyboard path to the buttons on a card: the first allow or deny the agent offered.
    pub(super) fn answer_first(&mut self, allow: bool, cx: &mut Context<Self>) {
        let Some(ask) = self.permissions.first() else {
            return;
        };
        let wanted = |kind: ChoiceKind| kind.allows() == allow;
        let choice = ask.choices.iter().position(|choice| wanted(choice.kind));
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
