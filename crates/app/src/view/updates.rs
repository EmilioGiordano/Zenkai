use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
#[cfg(windows)]
use super::lifecycle::Lifecycle;
use super::notices::{Lifetime, NoticeKind, Severity};
use crate::close_prompt::CloseGoal;
#[cfg(windows)]
use crate::close_prompt::CloseRoom;
use crate::updates::{self, CannotSelfUpdate, LauncherNotice, Report, SelfUpdate, Version};
#[cfg(windows)]
use crate::updates::{Blocked, DownloadOutcome, DownloadPhase, StageError};

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
        let Some(version) = updates::update_known(cx) else {
            self.notify(Severity::Info, t!("update.none_available"), cx);
            return;
        };
        #[cfg(not(windows))]
        let _ = version;
        match updates::self_update() {
            #[cfg(windows)]
            SelfUpdate::Available => self.download_update(version, cx),
            #[cfg(windows)]
            SelfUpdate::Unavailable {
                reason: CannotSelfUpdate::NotInstalled,
            } => self.post_update_card(
                NoticeKind::UpdateNotInstalled,
                t!("update.not_installed"),
                cx,
            ),
            SelfUpdate::Unavailable {
                reason: CannotSelfUpdate::NoTrustedKey | CannotSelfUpdate::NotWindows,
            } => self.open_releases(window, cx),
            #[cfg(not(windows))]
            _ => self.open_releases(window, cx),
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
        if let Some(ready) = updates::ready_for(cx, version) {
            self.post_update_card(
                NoticeKind::UpdateReady(ready),
                t!("update.ready", version = ready),
                cx,
            );
            return;
        }
        match updates::download_phase(cx) {
            DownloadPhase::Downloading { version, .. } => self.post_update_card(
                NoticeKind::UpdateDownloading(version),
                t!("update.downloading"),
                cx,
            ),
            DownloadPhase::Stopping => self.notify(Severity::Info, t!("update.still_stopping"), cx),
            DownloadPhase::Staging(_) | DownloadPhase::Staged(_) => {
                self.notify(Severity::Info, t!("update.still_preparing"), cx)
            }
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
            DownloadOutcome::Blocked(version, blocked) => self.post_update_card(
                NoticeKind::UpdateBlocked(version),
                blocked_text(blocked, version),
                cx,
            ),
            DownloadOutcome::InstallCheckFailed(error) => {
                self.notices.drop_download_card();
                self.notify(
                    Severity::Warning,
                    t!("update.prepare_failed", error = error),
                    cx,
                );
            }
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

    // "Restart now": the update is verified and extracted again in the background, then the app
    // goes through its normal close, unsaved work prompt included, and hands over to the launcher.
    #[cfg(windows)]
    pub(super) fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.close_room() {
            CloseRoom::Free => {}
            CloseRoom::AppClosing => {
                self.notify(Severity::Info, t!("update.already_closing"), cx);
                return;
            }
            CloseRoom::Blocked => {
                self.notify(Severity::Warning, t!("update.close_blocked"), cx);
                return;
            }
        }
        if matches!(
            updates::download_phase(cx),
            DownloadPhase::Staging(_) | DownloadPhase::Staged(_)
        ) {
            self.notify(Severity::Info, t!("update.still_preparing"), cx);
            return;
        }
        let Some((version, checked)) = updates::stage_update(cx) else {
            self.notify(Severity::Info, t!("update.none_ready"), cx);
            return;
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(staged) = checked.recv().await else {
                tracing::debug!("the update checks ended without a word for this window");
                return;
            };
            let shown = this.update_in(cx, |this, window, cx| {
                this.staged(version, staged, window, cx)
            });
            if let Err(error) = shown {
                tracing::debug!(%error, "workspace gone while the update was prepared");
            }
        })
        .detach();
    }

    #[cfg(windows)]
    fn close_room(&self) -> CloseRoom {
        CloseRoom::of(
            self.lifecycle == Lifecycle::Closing || self.pending_goal.is_some(),
            self.close_dialog_purpose().map(|(purpose, _)| purpose),
            self.departure.as_ref().map(|departure| departure.purpose),
            self.prompts.blocks_input(),
        )
    }

    // The goal the close of the app now carries, wherever it is held.
    #[cfg(windows)]
    fn close_goal_held(&self) -> Option<CloseGoal> {
        self.close_dialog_purpose()
            .map(|(_, goal)| goal)
            .or_else(|| self.departure.as_ref().map(|departure| departure.plan.goal))
            .or(self.pending_goal)
    }

    // Every way a close of the app is called off ends here, so a staged update goes back to
    // waiting, ready, and is checked again next time.
    pub(super) fn close_called_off(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        updates::unstage(cx);
        cx.notify();
    }

    // Only Windows installs updates; elsewhere nothing is ever ready.
    #[cfg(not(windows))]
    pub(super) fn restart_to_update(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.notify(Severity::Info, t!("update.none_ready"), cx);
    }

    // If the close is cancelled the app stays open and the update stays ready.
    #[cfg(windows)]
    fn staged(
        &mut self,
        version: Version,
        staged: Result<(), StageError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match staged {
            Ok(()) => self.close_to_update(version, window, cx),
            Err(StageError::Blocked(blocked)) => self.post_update_card(
                NoticeKind::UpdateBlocked(version),
                blocked_text(blocked, version),
                cx,
            ),
            Err(StageError::NotVerified(error)) => {
                tracing::warn!(%error, %version, "the update did not verify again and was deleted");
                self.post_update_card(
                    NoticeKind::UpdateNotVerified(version),
                    t!("update.not_verified", version = version),
                    cx,
                );
            }
            Err(StageError::Gone(error)) => {
                tracing::warn!(%error, %version, "the downloaded update is gone");
                self.notices.drop_ready_card(version);
                self.notify(Severity::Warning, t!("update.download_gone"), cx);
            }
            Err(error @ (StageError::Disk(_) | StageError::Lost)) => {
                tracing::warn!(%error, %version, "could not prepare the update");
                self.notify(
                    Severity::Warning,
                    t!("update.prepare_failed", error = error),
                    cx,
                );
            }
        }
        cx.notify();
    }

    // Like a restart for a new language: the window comes back from the tray for the unsaved
    // work prompt. A close that cannot start now says so instead of waiting silently, and a
    // close that does not carry this update leaves it ready, so no staged update is left behind.
    #[cfg(windows)]
    fn close_to_update(&mut self, version: Version, window: &mut Window, cx: &mut Context<Self>) {
        if updates::download_phase(cx) != DownloadPhase::Staged(version) {
            return;
        }
        let room = self.close_room();
        if room == CloseRoom::Blocked {
            updates::unstage(cx);
            self.notify(Severity::Warning, t!("update.close_blocked"), cx);
            return;
        }
        crate::tray::request_show(cx);
        self.request_close(CloseGoal::Update(version), window, cx);
        let started_here = room == CloseRoom::Free && self.lifecycle == Lifecycle::Closing;
        let carried = self.close_goal_held() == Some(CloseGoal::Update(version));
        if !started_here && !carried {
            updates::unstage(cx);
        }
    }

    pub(super) fn open_releases(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_card = self
            .notices
            .card()
            .is_some_and(|card| card.leads_to_releases());
        self.confirm_vetted_link(updates::RELEASES_PAGE, window, cx);
        if on_card {
            self.dismiss_notice(window, cx);
        }
    }

    pub(super) fn open_install_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_card = self
            .notices
            .card()
            .is_some_and(|card| card.leads_to_install_page());
        self.confirm_web_link(updates::INSTALL_PAGE.to_owned(), window, cx);
        if on_card {
            self.dismiss_notice(window, cx);
        }
    }

    // The first start of a new version, the one that confirmed it to the launcher.
    #[cfg(windows)]
    pub(crate) fn show_updated(&mut self, version: Version, cx: &mut Context<Self>) {
        self.post_update_card(NoticeKind::Updated(version), t!("update.updated"), cx);
    }
}

#[cfg(windows)]
fn blocked_text(blocked: Blocked, version: Version) -> String {
    match blocked {
        Blocked::NetworkPath => t!("update.blocked_network", version = version),
        Blocked::NoWriteAccess => t!("update.blocked_no_write", version = version),
    }
}

#[cfg(windows)]
fn is_download_card(card: &super::notices::Notice) -> bool {
    matches!(card.kind, NoticeKind::UpdateDownloading(_))
}
