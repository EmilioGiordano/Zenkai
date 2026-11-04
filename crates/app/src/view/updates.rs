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
        self.notices.post_update_available(text.into(), time.into());
        cx.notify();
    }

    pub(super) fn update_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.open_url(updates::RELEASES_PAGE);
        self.dismiss_notice(window, cx);
    }
}
