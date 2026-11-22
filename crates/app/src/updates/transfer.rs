use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::download::DownloadError;
use super::handoff::Blocked;
use super::prepare::PrepareError;
use super::version::Version;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Downloading {
        version: Version,
        received: u64,
        total: Option<u64>,
    },
    Stopping,
    Ready(Version),
    Staging(Version),
    // Checked and extracted again for the close in progress: the only state the launcher is
    // started from.
    Staged(Version),
}

// How a "Restart now" check ended, for the update it checked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StageEnd {
    Staged,
    Retry,
    Gone,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NextStart {
    Launcher,
    Itself,
}

impl Phase {
    pub fn percent(self) -> Option<u8> {
        let Phase::Downloading {
            received,
            total: Some(total),
            ..
        } = self
        else {
            return None;
        };
        if total == 0 {
            return None;
        }
        let percent = received.min(total).saturating_mul(100) / total;
        u8::try_from(percent).ok()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Ready(Version),
    NotVerified(Version),
    NotPublished(Version),
    Blocked(Version, Blocked),
    InstallCheckFailed(String),
    Failed,
}

pub enum Event {
    Trying(Version),
    Received { received: u64, total: Option<u64> },
    Finished(Result<Version, PrepareError>),
}

// "Verified" lives only here, in memory, for this session: the swap verifies the zip again.
#[derive(Default)]
pub struct Transfer {
    phase: Phase,
    cancel: Option<Arc<AtomicBool>>,
    asked: Option<Version>,
    pub outcome: Option<Outcome>,
}

impl Transfer {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    // None while a download runs or winds down: a second request never starts a second one
    // writing the same files.
    pub fn begin(&mut self, version: Version) -> Option<Arc<AtomicBool>> {
        if matches!(
            self.phase,
            Phase::Downloading { .. } | Phase::Stopping | Phase::Staging(_) | Phase::Staged(_)
        ) {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.asked = Some(version);
        self.phase = Phase::Downloading {
            version,
            received: 0,
            total: None,
        };
        self.outcome = None;
        Some(cancel)
    }

    // The thread sees the flag between two reads; until it ends the phase is Stopping.
    pub fn cancel(&mut self) -> bool {
        if !matches!(self.phase, Phase::Downloading { .. }) {
            return false;
        }
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.phase = Phase::Stopping;
        true
    }

    // A version comes back when its download finished after a cancel: its files are still on
    // disk, the phase stays Stopping, and `cleaned_up` ends it once they are deleted.
    pub fn receive(&mut self, event: Event) -> Option<Version> {
        match event {
            Event::Trying(version) => {
                if matches!(self.phase, Phase::Downloading { .. }) {
                    self.phase = Phase::Downloading {
                        version,
                        received: 0,
                        total: None,
                    };
                }
            }
            Event::Received { received, total } => {
                if let Phase::Downloading { version, .. } = self.phase {
                    self.phase = Phase::Downloading {
                        version,
                        received,
                        total,
                    };
                }
            }
            Event::Finished(result) => return self.finish(result),
        }
        None
    }

    // A ready update stands for every version up to the one asked for: when that one did not
    // verify, an older one took its place, and asking again would only delete it and repeat the
    // fallback. A version newer than the one asked for downloads anew.
    pub fn ready_for(&self, wanted: Version) -> Option<Version> {
        let Phase::Ready(ready) = self.phase else {
            return None;
        };
        let covered = self.asked.map_or(ready, |asked| asked.max(ready));
        (wanted <= covered).then_some(ready)
    }

    // The checks before the handoff run in the background; meanwhile no download may start, as
    // it would delete what they extract.
    pub fn begin_staging(&mut self) -> Option<Version> {
        let Phase::Ready(version) = self.phase else {
            return None;
        };
        self.phase = Phase::Staging(version);
        Some(version)
    }

    // Retry keeps the update ready, as after a passing disk error; Gone ends it, as when it did
    // not verify again, its download is gone or the install refuses it.
    pub fn end_staging(&mut self, version: Version, end: StageEnd) {
        if self.phase != Phase::Staging(version) {
            return;
        }
        self.phase = match end {
            StageEnd::Staged => Phase::Staged(version),
            StageEnd::Retry => Phase::Ready(version),
            StageEnd::Gone => Phase::Idle,
        };
    }

    // The close was called off: the update waits, ready, and is checked again next time.
    pub fn unstage(&mut self) {
        if let Phase::Staged(version) = self.phase {
            self.phase = Phase::Ready(version);
        }
    }

    pub fn next_start(&self, version: Version) -> NextStart {
        if self.phase == Phase::Staged(version) {
            NextStart::Launcher
        } else {
            NextStart::Itself
        }
    }

    pub fn cleaned_up(&mut self) {
        if self.phase == Phase::Stopping {
            self.phase = Phase::Idle;
        }
    }

    // The download thread ended without a last word, which only a panic does. Only the download
    // that `cancel` belongs to is ended, so a late signal never touches a newer one.
    pub fn lose(&mut self, cancel: &Arc<AtomicBool>) {
        let ours = self
            .cancel
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, cancel));
        if ours {
            self.finish(Err(PrepareError::Lost));
        }
    }

    // A download the user cancelled ends quietly, even if it got to the end first.
    fn finish(&mut self, result: Result<Version, PrepareError>) -> Option<Version> {
        let cancelled = self.phase == Phase::Stopping;
        self.cancel = None;
        if let (true, Ok(version)) = (cancelled, &result) {
            return Some(*version);
        }
        self.phase = Phase::Idle;
        let outcome = match result {
            Ok(version) => {
                self.phase = Phase::Ready(version);
                Some(Outcome::Ready(version))
            }
            Err(PrepareError::Download(DownloadError::Cancelled)) => None,
            Err(error) if cancelled => {
                tracing::info!(%error, "the cancelled update download ended");
                None
            }
            Err(PrepareError::NotVerified(version)) => Some(Outcome::NotVerified(version)),
            Err(PrepareError::NotPublished(version)) => Some(Outcome::NotPublished(version)),
            Err(PrepareError::Blocked(version, blocked)) => {
                Some(Outcome::Blocked(version, blocked))
            }
            Err(PrepareError::InstallCheck(error)) => {
                tracing::warn!(%error, "could not check whether this install can update itself");
                Some(Outcome::InstallCheckFailed(error.to_string()))
            }
            Err(error) => {
                tracing::warn!(%error, "the update download failed");
                Some(Outcome::Failed)
            }
        };
        self.outcome = outcome;
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::Ordering;

    use proptest::prelude::*;

    use super::super::download::DownloadError;
    use super::super::handoff::Blocked;
    use super::super::prepare::PrepareError;
    use super::super::version::Version;
    use super::{Event, NextStart, Outcome, Phase, StageEnd, Transfer};

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    #[test]
    fn a_download_reports_progress_and_ends_ready() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Received {
            received: 40,
            total: Some(100),
        });
        assert_eq!(transfer.phase().percent(), Some(40));
        transfer.receive(Event::Finished(Ok(version("0.3.0"))));
        assert_eq!(transfer.phase(), Phase::Ready(version("0.3.0")));
        assert_eq!(transfer.outcome, Some(Outcome::Ready(version("0.3.0"))));
    }

    #[test]
    fn trying_an_older_version_restarts_the_progress() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Received {
            received: 90,
            total: Some(100),
        });
        transfer.receive(Event::Trying(version("0.2.0")));
        assert_eq!(
            transfer.phase(),
            Phase::Downloading {
                version: version("0.2.0"),
                received: 0,
                total: None
            }
        );
    }

    #[test]
    fn a_second_request_does_not_start_a_second_download() {
        let mut transfer = Transfer::default();
        assert!(transfer.begin(version("0.3.0")).is_some());
        assert!(transfer.begin(version("0.3.0")).is_none());
        assert!(transfer.cancel());
        assert!(transfer.begin(version("0.3.0")).is_none());
    }

    #[test]
    fn cancelling_raises_the_flag_and_ends_quietly() {
        let mut transfer = Transfer::default();
        let flag = transfer.begin(version("0.3.0")).unwrap();
        assert!(transfer.cancel());
        assert!(flag.load(Ordering::Relaxed));
        assert_eq!(transfer.phase(), Phase::Stopping);
        assert!(!transfer.cancel());
        transfer.receive(Event::Finished(Err(PrepareError::Download(
            DownloadError::Cancelled,
        ))));
        assert_eq!(transfer.phase(), Phase::Idle);
        assert_eq!(transfer.outcome, None);
        assert!(transfer.begin(version("0.3.0")).is_some());
    }

    #[test]
    fn a_download_that_finishes_after_cancel_is_not_offered_and_is_handed_back_to_delete() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.cancel();
        let late = transfer.receive(Event::Finished(Ok(version("0.3.0"))));
        assert_eq!(late, Some(version("0.3.0")));
        assert_eq!(transfer.phase(), Phase::Stopping);
        assert!(transfer.begin(version("0.3.0")).is_none());
        assert_eq!(transfer.outcome, None);
        transfer.cleaned_up();
        assert_eq!(transfer.phase(), Phase::Idle);
        assert_eq!(transfer.outcome, None);
        assert!(transfer.begin(version("0.3.0")).is_some());
    }

    #[test]
    fn only_a_download_that_finished_after_cancel_is_handed_back() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        assert_eq!(
            transfer.receive(Event::Finished(Ok(version("0.3.0")))),
            None
        );
        transfer.begin(version("0.3.1")).unwrap();
        transfer.cancel();
        let ended = Event::Finished(Err(PrepareError::Download(DownloadError::Cancelled)));
        assert_eq!(transfer.receive(ended), None);
        assert_eq!(transfer.phase(), Phase::Idle);
        transfer.cleaned_up();
        assert_eq!(transfer.phase(), Phase::Idle);
    }

    #[test]
    fn failures_are_reported_by_kind() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Finished(Err(PrepareError::NotVerified(version(
            "0.3.0",
        )))));
        assert_eq!(
            transfer.outcome,
            Some(Outcome::NotVerified(version("0.3.0")))
        );
        assert_eq!(transfer.phase(), Phase::Idle);

        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Finished(Err(PrepareError::Disk(io::Error::other(
            "full",
        )))));
        assert_eq!(transfer.outcome, Some(Outcome::Failed));
    }

    #[test]
    fn progress_without_a_known_size_has_no_percent() {
        let unknown = Phase::Downloading {
            version: version("0.3.0"),
            received: 10,
            total: None,
        };
        assert_eq!(unknown.percent(), None);
        let empty = Phase::Downloading {
            version: version("0.3.0"),
            received: 0,
            total: Some(0),
        };
        assert_eq!(empty.percent(), None);
        let over = Phase::Downloading {
            version: version("0.3.0"),
            received: 500,
            total: Some(100),
        };
        assert_eq!(over.percent(), Some(100));
    }

    #[test]
    fn a_ready_update_can_be_downloaded_again() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Finished(Ok(version("0.3.0"))));
        assert!(transfer.begin(version("0.3.1")).is_some());
        assert_eq!(transfer.outcome, None);
    }

    #[test]
    fn a_download_thread_that_dies_counts_as_failed() {
        let mut transfer = Transfer::default();
        let flag = transfer.begin(version("0.3.0")).unwrap();
        transfer.lose(&flag);
        assert_eq!(transfer.phase(), Phase::Idle);
        assert_eq!(transfer.outcome, Some(Outcome::Failed));
        assert!(transfer.begin(version("0.3.0")).is_some());
    }

    #[test]
    fn a_late_loss_leaves_a_finished_or_newer_download_alone() {
        let mut transfer = Transfer::default();
        let first = transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Finished(Ok(version("0.3.0"))));
        transfer.lose(&first);
        assert_eq!(transfer.phase(), Phase::Ready(version("0.3.0")));
        assert_eq!(transfer.outcome, Some(Outcome::Ready(version("0.3.0"))));

        let second = transfer.begin(version("0.3.1")).unwrap();
        transfer.lose(&first);
        assert!(matches!(transfer.phase(), Phase::Downloading { .. }));
        transfer.lose(&second);
        assert_eq!(transfer.phase(), Phase::Idle);
    }

    #[test]
    fn a_cancelled_download_whose_thread_dies_ends_quietly() {
        let mut transfer = Transfer::default();
        let flag = transfer.begin(version("0.3.0")).unwrap();
        transfer.cancel();
        transfer.lose(&flag);
        assert_eq!(transfer.phase(), Phase::Idle);
        assert_eq!(transfer.outcome, None);
    }

    fn ready(asked: &str, got: &str) -> Transfer {
        let mut transfer = Transfer::default();
        transfer.begin(version(asked)).unwrap();
        transfer.receive(Event::Finished(Ok(version(got))));
        transfer
    }

    #[test]
    fn an_older_version_ready_after_a_fallback_stands_for_the_one_asked_for() {
        let transfer = ready("0.3.0", "0.2.0");
        assert_eq!(transfer.phase(), Phase::Ready(version("0.2.0")));
        assert_eq!(transfer.ready_for(version("0.3.0")), Some(version("0.2.0")));
        assert_eq!(transfer.ready_for(version("0.2.0")), Some(version("0.2.0")));
        assert_eq!(transfer.ready_for(version("0.3.1")), None);
    }

    #[test]
    fn nothing_is_ready_for_anyone_while_not_ready() {
        let mut transfer = Transfer::default();
        assert_eq!(transfer.ready_for(version("0.2.0")), None);
        transfer.begin(version("0.2.0")).unwrap();
        assert_eq!(transfer.ready_for(version("0.2.0")), None);
    }

    #[test]
    fn staging_holds_off_downloads_until_it_ends() {
        let mut transfer = ready("0.3.0", "0.3.0");
        assert_eq!(transfer.begin_staging(), Some(version("0.3.0")));
        assert_eq!(transfer.phase(), Phase::Staging(version("0.3.0")));
        assert!(transfer.begin(version("0.4.0")).is_none());
        assert!(!transfer.cancel());
        assert_eq!(transfer.begin_staging(), None);
        assert_eq!(transfer.ready_for(version("0.3.0")), None);
        transfer.end_staging(version("0.3.0"), StageEnd::Retry);
        assert_eq!(transfer.phase(), Phase::Ready(version("0.3.0")));
        assert_eq!(transfer.begin_staging(), Some(version("0.3.0")));
        transfer.end_staging(version("0.3.0"), StageEnd::Gone);
        assert_eq!(transfer.phase(), Phase::Idle);
        assert!(transfer.begin(version("0.4.0")).is_some());
    }

    #[test]
    fn only_a_ready_update_is_staged_and_only_its_own_end_counts() {
        let mut transfer = Transfer::default();
        assert_eq!(transfer.begin_staging(), None);
        transfer.begin(version("0.3.0")).unwrap();
        assert_eq!(transfer.begin_staging(), None);
        transfer.receive(Event::Finished(Ok(version("0.3.0"))));
        transfer.begin_staging().unwrap();
        transfer.end_staging(version("0.2.0"), StageEnd::Gone);
        transfer.end_staging(version("0.2.0"), StageEnd::Staged);
        assert_eq!(transfer.phase(), Phase::Staging(version("0.3.0")));
    }

    #[test]
    fn a_staged_update_holds_until_its_close_is_called_off() {
        let mut transfer = ready("0.3.0", "0.3.0");
        transfer.begin_staging().unwrap();
        transfer.end_staging(version("0.3.0"), StageEnd::Staged);
        assert_eq!(transfer.phase(), Phase::Staged(version("0.3.0")));
        assert!(transfer.begin(version("0.4.0")).is_none());
        assert_eq!(transfer.begin_staging(), None);
        assert!(!transfer.cancel());
        assert_eq!(transfer.ready_for(version("0.3.0")), None);
        transfer.end_staging(version("0.3.0"), StageEnd::Gone);
        assert_eq!(transfer.phase(), Phase::Staged(version("0.3.0")));
        transfer.unstage();
        assert_eq!(transfer.phase(), Phase::Ready(version("0.3.0")));
        transfer.unstage();
        assert_eq!(transfer.phase(), Phase::Ready(version("0.3.0")));
    }

    #[test]
    fn only_the_staged_version_starts_the_launcher() {
        let mut transfer = ready("0.3.0", "0.3.0");
        assert_eq!(transfer.next_start(version("0.3.0")), NextStart::Itself);
        transfer.begin_staging().unwrap();
        assert_eq!(transfer.next_start(version("0.3.0")), NextStart::Itself);
        transfer.end_staging(version("0.3.0"), StageEnd::Staged);
        assert_eq!(transfer.next_start(version("0.3.0")), NextStart::Launcher);
        assert_eq!(transfer.next_start(version("0.2.0")), NextStart::Itself);
        transfer.unstage();
        assert_eq!(transfer.next_start(version("0.3.0")), NextStart::Itself);
        assert_eq!(
            Transfer::default().next_start(version("0.3.0")),
            NextStart::Itself
        );
    }

    #[test]
    fn an_install_that_cannot_be_checked_is_reported_with_the_error() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Finished(Err(PrepareError::InstallCheck(
            io::Error::other("denied"),
        ))));
        assert_eq!(
            transfer.outcome,
            Some(Outcome::InstallCheckFailed("denied".to_owned()))
        );
    }

    #[test]
    fn an_install_that_refuses_updates_is_reported_with_the_version() {
        let mut transfer = Transfer::default();
        transfer.begin(version("0.3.0")).unwrap();
        transfer.receive(Event::Finished(Err(PrepareError::Blocked(
            version("0.3.0"),
            Blocked::NoWriteAccess,
        ))));
        assert_eq!(transfer.phase(), Phase::Idle);
        assert_eq!(
            transfer.outcome,
            Some(Outcome::Blocked(version("0.3.0"), Blocked::NoWriteAccess))
        );
    }

    #[derive(Clone, Debug)]
    enum Step {
        Begin(u8),
        Cancel,
        Ready(u8),
        Failed,
        CleanedUp,
        Stage,
        End(u8, StageEnd),
        Unstage,
    }

    fn step() -> impl Strategy<Value = Step> {
        let end = prop_oneof![
            Just(StageEnd::Staged),
            Just(StageEnd::Retry),
            Just(StageEnd::Gone)
        ];
        prop_oneof![
            (0u8..4).prop_map(Step::Begin),
            Just(Step::Cancel),
            (0u8..4).prop_map(Step::Ready),
            Just(Step::Failed),
            Just(Step::CleanedUp),
            Just(Step::Stage),
            (0u8..4, end).prop_map(|(patch, end)| Step::End(patch, end)),
            Just(Step::Unstage),
        ]
    }

    fn numbered(patch: u8) -> Version {
        version(&format!("0.2.{patch}"))
    }

    proptest! {
        // The files staged for the launcher are deleted only by a new download; none may start
        // while they are checked or staged, a check is released only by its own end, and a
        // staged update only when its close is called off.
        #[test]
        fn a_staged_update_is_released_only_by_its_own_end(
            steps in proptest::collection::vec(step(), 1..40)
        ) {
            let mut transfer = Transfer::default();
            // Only a download thread that runs sends its last word.
            let mut running = false;
            for step in steps {
                let before = transfer.phase();
                let releasing = match (&step, before) {
                    (Step::End(patch, _), Phase::Staging(staged)) => numbered(*patch) == staged,
                    (Step::Unstage, Phase::Staged(_)) => true,
                    _ => false,
                };
                match step {
                    Step::Begin(patch) => {
                        let began = transfer.begin(numbered(patch)).is_some();
                        if matches!(before, Phase::Staging(_) | Phase::Staged(_)) {
                            prop_assert!(!began);
                        }
                        running |= began;
                    }
                    Step::Cancel => {
                        transfer.cancel();
                    }
                    Step::Ready(patch) if running => {
                        running = false;
                        transfer.receive(Event::Finished(Ok(numbered(patch))));
                    }
                    Step::Failed if running => {
                        running = false;
                        transfer.receive(Event::Finished(Err(PrepareError::Lost)));
                    }
                    Step::Ready(_) | Step::Failed => {}
                    Step::CleanedUp => transfer.cleaned_up(),
                    Step::Stage => {
                        let staged = transfer.begin_staging();
                        if let Phase::Ready(version) = before {
                            prop_assert_eq!(staged, Some(version));
                        } else {
                            prop_assert_eq!(staged, None);
                        }
                    }
                    Step::End(patch, end) => transfer.end_staging(numbered(patch), end),
                    Step::Unstage => transfer.unstage(),
                }
                if matches!(before, Phase::Staging(_) | Phase::Staged(_)) && !releasing {
                    prop_assert_eq!(transfer.phase(), before);
                }
                if let Phase::Staged(version) = transfer.phase() {
                    prop_assert_eq!(transfer.next_start(version), NextStart::Launcher);
                }
            }
        }
    }
}
