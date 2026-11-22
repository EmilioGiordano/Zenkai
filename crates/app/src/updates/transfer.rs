use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::download::DownloadError;
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Ready(Version),
    NotVerified(Version),
    NotPublished(Version),
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
        if matches!(self.phase, Phase::Downloading { .. } | Phase::Stopping) {
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

    use super::super::download::DownloadError;
    use super::super::prepare::PrepareError;
    use super::super::version::Version;
    use super::{Event, Outcome, Phase, Transfer};

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
}
