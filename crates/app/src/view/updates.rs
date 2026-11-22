use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::notices::{Lifetime, NoticeKind, Severity};
use crate::close_prompt::CloseGoal;
use crate::updates::{self, LauncherNotice, Report, Version};
#[cfg(windows)]
use crate::updates::{DownloadOutcome, DownloadPhase};

impl Workspace {
    pub(super) fn show_update_report(&mut self, cx: &mut Context<Self>) {
        for notice in updates::take_launcher_notices(cx) {
            self.show_launcher_notice(notice, cx);
        }
        #[cfg(windows)]
        self.show_download_outcome(cx);
        let Some(report) = updates::take_report(cx) else {
            return;
        };
        match report {
            Report::Available(version) => self.notify_update_available(version, cx),
            Report::UpToDate => self.notify(Severity::Info, t!("update.up_to_date"), cx),
            Report::Failed => self.notify(Severity::Warning, t!("update.check_failed"), cx),
        }
    }

    // Unlike other warnings it does not expire: it explains why the version running is not the
    // one installed last.
    fn show_launcher_notice(&mut self, notice: LauncherNotice, cx: &mut Context<Self>) {
        let text: SharedString = match notice {
            LauncherNotice::Restored(Some(version)) => {
                t!("update.restored", version = version).into()
            }
            LauncherNotice::Restored(None) => t!("update.restored_unnamed").into(),
            LauncherNotice::Interrupted => t!("update.interrupted").into(),
            LauncherNotice::NotApplied(version) => {
                t!("update.not_applied", version = version).into()
            }
        };
        self.post(Severity::Warning, text, None, Lifetime::UntilReplaced);
        cx.notify();
    }

    pub(crate) fn close_unconfirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!(
            "this version could not confirm its start; closing so the launcher restores the previous one"
        );
        self.post(
            Severity::Error,
            t!("update.unconfirmed").into(),
            None,
            Lifetime::UntilReplaced,
        );
        self.request_close(CloseGoal::Quit, window, cx);
    }

    fn notify_update_available(&mut self, version: Version, cx: &mut Context<Self>) {
        let text = t!(
            "update.available",
            version = version,
            installed = env!("CARGO_PKG_VERSION")
        );
        self.post_update_card(NoticeKind::UpdateAvailable(version), text, cx);
        self.announce_shown_update(cx);
    }

    fn post_update_card(
        &mut self,
        kind: NoticeKind,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        self.notices.post_update(kind, text.into(), time.into());
        cx.notify();
    }

    // A version counts as announced when its card was on screen, not when it was found.
    pub(super) fn announce_shown_update(&mut self, cx: &mut Context<Self>) {
        if let Some(version) = self.notices.take_shown_update() {
            updates::mark_announced(cx, version);
        }
    }

    pub(super) fn update_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match updates::update_known(cx) {
            None => self.notify(Severity::Info, t!("update.none_available"), cx),
            #[cfg(windows)]
            Some(version) if updates::can_download() => self.download_update(version, cx),
            Some(_) => {
                let on_card = self
                    .notices
                    .card()
                    .is_some_and(|card| card.is_update_available());
                self.confirm_vetted_link(updates::RELEASES_PAGE, window, cx);
                if on_card {
                    self.dismiss_notice(window, cx);
                }
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn cancel_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !updates::cancel_download(cx) {
            self.notify(Severity::Info, t!("update.none_downloading"), cx);
            return;
        }
        if self.notices.card().is_some_and(is_download_card) {
            self.dismiss_notice(window, cx);
        }
        self.notices.drop_download_card();
        cx.notify();
    }

    // Only Windows downloads updates; elsewhere "Update now" opens the Releases page.
    #[cfg(not(windows))]
    pub(super) fn cancel_update(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.notify(Severity::Info, t!("update.none_downloading"), cx);
    }

    #[cfg(windows)]
    fn download_update(&mut self, version: Version, cx: &mut Context<Self>) {
        match updates::download_phase(cx) {
            DownloadPhase::Ready(ready) if ready >= version => self.post_update_card(
                NoticeKind::UpdateReady(ready),
                t!("update.ready", version = ready),
                cx,
            ),
            DownloadPhase::Downloading { version, .. } => self.post_update_card(
                NoticeKind::UpdateDownloading(version),
                t!("update.downloading"),
                cx,
            ),
            DownloadPhase::Stopping => self.notify(Severity::Info, t!("update.still_stopping"), cx),
            DownloadPhase::Idle | DownloadPhase::Ready(_) => {
                updates::start_download(cx, version);
                self.post_update_card(
                    NoticeKind::UpdateDownloading(version),
                    t!("update.downloading"),
                    cx,
                );
            }
        }
    }

    // Runs on every change to the update state, so the progress on screen follows the download.
    #[cfg(windows)]
    fn show_download_outcome(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(outcome) = updates::take_download_outcome(cx) else {
            return;
        };
        match outcome {
            DownloadOutcome::Ready(version) => self.post_update_card(
                NoticeKind::UpdateReady(version),
                t!("update.ready", version = version),
                cx,
            ),
            DownloadOutcome::NotVerified(version) => self.post_update_card(
                NoticeKind::UpdateNotVerified(version),
                t!("update.not_verified", version = version),
                cx,
            ),
            DownloadOutcome::NotPublished(version) => {
                self.notices.drop_download_card();
                self.notify(
                    Severity::Warning,
                    t!("update.not_published", version = version),
                    cx,
                );
            }
            DownloadOutcome::Failed => {
                self.notices.drop_download_card();
                self.notify(Severity::Warning, t!("update.download_failed"), cx);
            }
        }
    }
}

#[cfg(windows)]
fn is_download_card(card: &super::notices::Notice) -> bool {
    matches!(card.kind, NoticeKind::UpdateDownloading(_))
}
