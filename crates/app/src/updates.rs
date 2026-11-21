mod fetch;
mod notes;
mod record;
mod releases;
mod state;
mod store;
mod version;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;

use crate::agent_settings::AgentConfig;
use fetch::FetchError;
#[cfg(test)]
pub use notes::MAX_NOTES;
pub use notes::{Day, Note, Notes};
use record::Record;
use releases::Releases;
pub use state::{Report, Source, Status, Updates, known_update};
use state::{SettingsReading, Snapshot, StartupCheck};
pub use version::Version;

pub const RELEASES_PAGE: &str = "https://github.com/EmilioGiordano/Zenkai/releases";

impl Global for Updates {}

pub fn init(cx: &mut App) {
    cx.set_global(Updates::default());
    cx.observe_global::<AgentConfig>(start_if_ready).detach();
    cx.spawn(async move |cx| {
        let (record, notes) = cx
            .background_executor()
            .spawn(async { (Record::load(), Notes::load()) })
            .await;
        cx.update(|cx| {
            let Some(installed) = Version::installed() else {
                tracing::error!("the installed version is not a plain X.Y.Z");
                return;
            };
            cx.update_global::<Updates, _>(|updates, _| {
                updates.load_record(record, installed);
                updates.load_notes(notes);
            });
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

pub fn update_known(cx: &App) -> bool {
    Version::installed()
        .and_then(|installed| known_update(status(cx), &notes(cx), installed))
        .is_some()
}

// Called from an observer of `Updates`: updating the global when there is nothing to take would
// notify that observer again, forever, and freeze the app at startup.
pub fn take_report(cx: &mut App) -> Option<Report> {
    let report = cx.global::<Updates>().report?;
    cx.update_global::<Updates, _>(|updates, _| updates.report = None);
    Some(report)
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
