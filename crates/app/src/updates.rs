mod fetch;
mod record;
mod state;
mod version;

use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;

use crate::agent_settings::AgentConfig;
use fetch::FetchError;
use record::Record;
pub use state::{Report, Source, Status, Updates};
use state::{SettingsReading, StartupCheck};
pub use version::Version;

pub const RELEASES_PAGE: &str = "https://github.com/EmilioGiordano/Zenkai/releases";

impl Global for Updates {}

pub fn init(cx: &mut App) {
    cx.set_global(Updates::new());
    cx.observe_global::<AgentConfig>(start_if_ready).detach();
    cx.spawn(async move |cx| {
        let record = cx
            .background_executor()
            .spawn(async { Record::load() })
            .await;
        cx.update(|cx| {
            let Some(installed) = Version::installed() else {
                tracing::error!("the installed version is not a plain X.Y.Z");
                return;
            };
            cx.update_global::<Updates, _>(|updates, _| updates.load_record(record, installed));
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
        return;
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

// Called from an observer of `Updates`: updating the global when there is nothing to take would
// notify that observer again, forever, and freeze the app at startup.
pub fn take_report(cx: &mut App) -> Option<Report> {
    let report = cx.global::<Updates>().report?;
    cx.update_global::<Updates, _>(|updates, _| updates.report = None);
    Some(report)
}

// Called once the notice is on screen: a version counts as announced only when it was seen.
pub fn mark_announced(cx: &mut App, version: Version) {
    let record = cx.update_global::<Updates, _>(|updates, _| {
        updates.mark_announced(version);
        updates.record.clone()
    });
    save_in_background(record, cx);
}

pub fn check(cx: &mut App, source: Source) {
    let (Some(installed), Some(started_at)) = (Version::installed(), now()) else {
        tracing::error!("the update check needs the installed version and the clock");
        return;
    };
    if !cx.update_global::<Updates, _>(|updates, _| updates.begin_check(source)) {
        return;
    }
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async { fetch::newest_release() })
            .await;
        cx.update(|cx| finish(cx, installed, started_at, result));
    })
    .detach();
}

fn finish(
    cx: &mut App,
    installed: Version,
    checked_at: u64,
    result: Result<Option<Version>, FetchError>,
) {
    let record = cx.update_global::<Updates, _>(|updates, _| {
        match result {
            Ok(newest) => updates.finish_ok(newest, installed, checked_at),
            Err(error) => {
                tracing::warn!(%error, "the update check failed");
                updates.finish_failed(checked_at);
            }
        }
        updates.record.clone()
    });
    save_in_background(record, cx);
}

fn save_in_background(record: Record, cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            if let Err(error) = record.save() {
                tracing::warn!(%error, "could not save the update check record");
            }
        })
        .detach();
}
