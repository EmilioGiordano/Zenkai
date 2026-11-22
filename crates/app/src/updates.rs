#[cfg(windows)]
mod archive;
#[cfg(windows)]
mod download;
#[cfg(windows)]
mod download_task;
mod fetch;
#[cfg(windows)]
mod handoff;
mod launcher_notice;
mod notes;
#[cfg(windows)]
mod prepare;
mod record;
mod releases;
#[cfg(windows)]
mod signature;
mod state;
mod store;
#[cfg(windows)]
mod transfer;
mod version;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;

use crate::agent_settings::AgentConfig;
#[cfg(windows)]
pub use download_task::{
    can_download, cancel as cancel_download, phase as download_phase, ready_for, restart_into,
    stage as stage_update, start as start_download, take_outcome as take_download_outcome, unstage,
};
use fetch::FetchError;
#[cfg(windows)]
pub use handoff::{Blocked, StageError};
pub use launcher_notice::LauncherNotice;
#[cfg(test)]
pub use notes::MAX_NOTES;
pub use notes::{Day, Note, Notes};
use record::Record;
use releases::Releases;
pub use state::{Report, Source, Status, Updates, known_update};
use state::{SettingsReading, Snapshot, StartupCheck};
#[cfg(windows)]
pub use transfer::{Outcome as DownloadOutcome, Phase as DownloadPhase};
pub use version::Version;

pub const RELEASES_PAGE: &str = "https://github.com/EmilioGiordano/Zenkai/releases";

impl Global for Updates {}

pub fn init(cx: &mut App) {
    cx.set_global(Updates::default());
    cx.observe_global::<AgentConfig>(start_if_ready).detach();
    cx.spawn(async move |cx| {
        let (record, notes, notices) = cx
            .background_executor()
            .spawn(async {
                let notices = crate::installation::update_folder()
                    .map(|folder| launcher_notice::take(&folder))
                    .unwrap_or_default();
                (Record::load(), Notes::load(), notices)
            })
            .await;
        cx.update(|cx| {
            let Some(installed) = Version::installed() else {
                tracing::error!("the installed version is not a plain X.Y.Z");
                return;
            };
            let restored = cx.update_global::<Updates, _>(|updates, _| {
                updates.load_record(record, installed);
                updates.load_notes(notes);
                let mut changed = false;
                for notice in notices {
                    changed |= updates.receive_launcher_notice(notice);
                }
                changed.then(|| updates.snapshot_for_save())
            });
            if let Some(snapshot) = restored {
                save_in_background(snapshot, cx);
            }
            start_if_ready(cx);
        });
    })
    .detach();
}

fn start_if_ready(cx: &mut App) {
    if !cx.global::<Updates>().startup_pending() {
        return;
    }
    let config = cx.global::<AgentConfig>();
    let reading = if config.failure.is_some() || config.state.problem.is_some() {
        SettingsReading::Broken
    } else if config.paths.is_some() {
        SettingsReading::Loaded {
            check_for_updates: config.settings().general.check_for_updates,
        }
    } else {
        SettingsReading::Loading
    };
    let verdict =
        cx.update_global::<Updates, _>(|updates, _| updates.startup_check(reading, now()));
    if verdict == StartupCheck::Run {
        check(cx, Source::Startup);
    }
}

fn now() -> Option<u64> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(elapsed.as_secs())
}

pub fn status(cx: &App) -> Status {
    cx.global::<Updates>().status()
}

pub fn notes(cx: &App) -> Arc<Notes> {
    cx.global::<Updates>().notes()
}

pub fn update_known(cx: &App) -> Option<Version> {
    Version::installed().and_then(|installed| known_update(status(cx), &notes(cx), installed))
}

// Called from an observer of `Updates`: updating the global when there is nothing to take would
// notify that observer again, forever, and freeze the app at startup.
pub fn take_report(cx: &mut App) -> Option<Report> {
    let report = cx.global::<Updates>().report?;
    cx.update_global::<Updates, _>(|updates, _| updates.report = None);
    Some(report)
}

// Called from the same observer as `take_report`, so it too updates the global only when there
// is something to take.
pub fn take_launcher_notices(cx: &mut App) -> Vec<LauncherNotice> {
    if cx.global::<Updates>().launcher_notices.is_empty() {
        return Vec::new();
    }
    cx.update_global::<Updates, _>(|updates, _| std::mem::take(&mut updates.launcher_notices))
}

// Called once the notice is on screen: a version counts as announced only when it was seen.
pub fn mark_announced(cx: &mut App, version: Version) {
    let snapshot = cx.update_global::<Updates, _>(|updates, _| {
        updates.mark_announced(version);
        updates.snapshot_for_save()
    });
    save_in_background(snapshot, cx);
}

pub fn check(cx: &mut App, source: Source) {
    let (Some(installed), Some(started_at)) = (Version::installed(), now()) else {
        tracing::error!("the update check needs the installed version and the clock");
        cx.update_global::<Updates, _>(|updates, _| updates.fail_to_start(source));
        return;
    };
    if !cx.update_global::<Updates, _>(|updates, _| updates.begin_check(source)) {
        return;
    }
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move { fetch::latest_releases(installed) })
            .await;
        cx.update(|cx| finish(cx, installed, started_at, result));
    })
    .detach();
}

fn finish(cx: &mut App, installed: Version, checked_at: u64, result: Result<Releases, FetchError>) {
    let snapshot = cx.update_global::<Updates, _>(|updates, _| {
        match result {
            Ok(releases) => updates.finish_ok(releases, installed, checked_at),
            Err(error) => {
                tracing::warn!(%error, "the update check failed");
                updates.finish_failed(checked_at);
            }
        }
        updates.snapshot_for_save()
    });
    save_in_background(snapshot, cx);
}

fn save_in_background(snapshot: Snapshot, cx: &mut App) {
    cx.spawn(async move |cx| {
        let written = cx
            .background_executor()
            .spawn(async move {
                if let Err(error) = snapshot.record.save(snapshot.order) {
                    tracing::warn!(%error, "could not save the update check record");
                }
                let notes = snapshot.notes?;
                match notes.save(snapshot.order) {
                    Ok(()) => Some(notes),
                    Err(error) => {
                        tracing::warn!(%error, "could not save the release notes cache");
                        None
                    }
                }
            })
            .await;
        if let Some(notes) = written {
            cx.update(|cx| {
                cx.update_global::<Updates, _>(|updates, _| updates.notes_saved(&notes))
            });
        }
    })
    .detach();
}
