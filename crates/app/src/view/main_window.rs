use gpui_kit::*;

#[cfg(windows)]
use super::Workspace;
use crate::close_prompt::CloseGoal;

// The window's close button leaves Zenkai running in the tray when there is one.
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

    // Nothing is closed or reloaded: the same window comes back with the same workbooks.
    pub(super) fn hide_to_tray(&mut self, cx: &mut Context<Self>) {
        self.persist_session(cx);
        crate::tray::hide_main_window(cx);
    }
}
