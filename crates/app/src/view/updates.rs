use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::notices::Severity;
use crate::updates::{self, Report, Version};

impl Workspace {
    pub(super) fn show_update_report(&mut self, cx: &mut Context<Self>) {
        let Some(report) = updates::take_report(cx) else {
            return;
        };
        match report {
            Report::Available(version) => self.notify_update_available(version, cx),
            Report::UpToDate => self.notify(Severity::Info, t!("update.up_to_date"), cx),
            Report::Failed => self.notify(Severity::Warning, t!("update.check_failed"), cx),
        }
    }

    fn notify_update_available(&mut self, version: Version, cx: &mut Context<Self>) {
        let text = t!(
            "update.available",
            version = version,
            installed = env!("CARGO_PKG_VERSION")
        );
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        self.notices
            .post_update_available(version, text.into(), time.into());
        self.announce_shown_update(cx);
        cx.notify();
    }

    // A version counts as announced when its card was on screen, not when it was found.
    pub(super) fn announce_shown_update(&mut self, cx: &mut Context<Self>) {
        if let Some(version) = self.notices.take_shown_update() {
            updates::mark_announced(cx, version);
        }
    }

    pub(super) fn update_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .notices
            .card()
            .is_some_and(|card| card.is_update_available())
        {
            cx.propagate();
            return;
        }
        cx.open_url(updates::RELEASES_PAGE);
        self.dismiss_notice(window, cx);
    }
}
