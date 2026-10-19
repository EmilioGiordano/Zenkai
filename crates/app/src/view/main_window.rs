use gpui_kit::*;

use crate::close_prompt::CloseGoal;
#[cfg(windows)]
use {
    super::{Lifecycle, Severity, Workspace},
    crate::close_prompt::ClosePlan,
    zenkai_i18n::t,
};

#[cfg(windows)]
pub(super) fn close_goal(cx: &App) -> CloseGoal {
    if crate::tray::installed(cx) {
        CloseGoal::Hide
    } else {
        CloseGoal::Quit
    }
}

#[cfg(not(windows))]
pub(super) fn close_goal(_: &App) -> CloseGoal {
    CloseGoal::Quit
}

#[cfg(windows)]
impl Workspace {
    pub(crate) fn tray_summary(&self) -> (usize, u64) {
        (self.documents.len(), self.memory_mb)
    }

    // Quitting can raise a prompt even when the setting does not ask first (a busy
    // workbook, autosave off), so any unsaved work or open prompt needs a visible window.
    pub(crate) fn close_needs_window(&self) -> bool {
        !self.documents.unsaved().is_empty()
            || self.close_dialog.is_some()
            || self.departure.is_some()
            || self.prompts.blocks_input()
            || self.lifecycle == Lifecycle::Closing
    }

    pub(super) fn close_discarded(
        &mut self,
        plan: &ClosePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for id in &plan.discard {
            if self.documents.entry(*id).is_some() {
                self.finish_close(*id, window, cx);
            }
        }
    }

    pub(super) fn hide_to_tray(&mut self, cx: &mut Context<Self>) {
        self.lifecycle = Lifecycle::Running;
        self.clear_busy(t!("busy.closing"), cx);
        self.persist_session(cx);
        crate::tray::hide_main_window(cx);
    }

    pub(super) fn abandon_hide(&mut self, reason: &str, cx: &mut Context<Self>) {
        self.lifecycle = Lifecycle::Running;
        self.clear_busy(t!("busy.closing"), cx);
        self.notify(Severity::Error, t!("close.not_hidden", reason = reason), cx);
    }
}
