use gpui_kit::*;

use super::Workspace;
use crate::close_prompt::{CloseGoal, ClosePlan};
#[cfg(windows)]
use {
    super::{Lifecycle, Severity},
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

impl Workspace {
    #[cfg(windows)]
    pub(super) fn quit_or_hide(
        &mut self,
        plan: ClosePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if plan.goal == CloseGoal::Hide {
            self.close_discarded(&plan, window, cx);
            self.hide_to_tray(cx);
        } else {
            self.write_session_and_quit(plan, cx);
        }
    }

    #[cfg(not(windows))]
    pub(super) fn quit_or_hide(
        &mut self,
        plan: ClosePlan,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write_session_and_quit(plan, cx);
    }
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

    pub(crate) fn warn_about_launch(&mut self, text: String, cx: &mut Context<Self>) {
        self.notify(Severity::Warning, text, cx);
    }

    fn close_discarded(&mut self, plan: &ClosePlan, window: &mut Window, cx: &mut Context<Self>) {
        for id in &plan.discard {
            if self.documents.entry(*id).is_some() {
                self.finish_close(*id, window, cx);
            }
        }
    }

    // Reached once the recovery copies are written and nothing is busy, so closing the
    // discarded workbooks now cannot race an autosave into recreating their copies.
    pub(super) fn finish_hide(
        &mut self,
        plan: ClosePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unprotected = self.recovery_dir.is_none()
            && self
                .documents
                .iter()
                .any(|document| document.needs_recovery() && plan.keeps(document.id));
        if unprotected {
            self.ask_to_quit_anyway(t!("quit.autosave_off_reason"), plan, window, cx);
            return;
        }
        self.quit_or_hide(plan, window, cx);
    }

    fn hide_to_tray(&mut self, cx: &mut Context<Self>) {
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
