mod fetch;
mod record;
mod version;

use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::*;

use fetch::FetchError;
use record::Record;
pub use version::Version;

pub const RELEASES_PAGE: &str = "https://github.com/EmilioGiordano/Zenkai/releases";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Startup,
    Palette,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Unchecked,
    Checking,
    UpToDate { at: u64 },
    Available(Version),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Report {
    Available(Version),
    UpToDate,
    Failed,
}

pub struct Updates {
    record: Record,
    status: Status,
    silenced_this_session: bool,
    running: bool,
    report: Option<Report>,
}

impl Global for Updates {}

pub fn init(cx: &mut App) {
    cx.set_global(Updates {
        record: Record::default(),
        status: Status::Unchecked,
        silenced_this_session: false,
        running: false,
        report: None,
    });
    cx.spawn(async move |cx| {
        let record = cx
            .background_executor()
            .spawn(async { Record::load() })
            .await;
        cx.update(|cx| {
            let due = now().is_some_and(|now| record.is_due(now));
            let run = record.check_on_open && due;
            cx.update_global::<Updates, _>(|updates, _| updates.record = record);
            if run {
                check(cx, Source::Startup);
            }
        });
    })
    .detach();
}

fn now() -> Option<u64> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(elapsed.as_secs())
}

pub fn status(cx: &App) -> Status {
    cx.global::<Updates>().status
}

pub fn check_on_open(cx: &App) -> bool {
    cx.global::<Updates>().record.check_on_open
}

pub fn set_check_on_open(cx: &mut App, on: bool) {
    let record = cx.update_global::<Updates, _>(|updates, _| {
        updates.record.check_on_open = on;
        updates.record.clone()
    });
    save_in_background(record, cx);
}

pub fn silence_for_session(cx: &mut App) {
    cx.update_global::<Updates, _>(|updates, _| updates.silenced_this_session = true);
}

pub fn take_report(cx: &mut App) -> Option<Report> {
    cx.update_global::<Updates, _>(|updates, _| updates.report.take())
}

pub fn check(cx: &mut App, source: Source) {
    let (Some(installed), Some(started_at)) = (Version::installed(), now()) else {
        tracing::error!("the update check needs the installed version and the clock");
        return;
    };
    let started = cx.update_global::<Updates, _>(|updates, _| {
        let idle = !updates.running;
        if idle {
            updates.running = true;
            updates.status = Status::Checking;
        }
        idle
    });
    if !started {
        return;
    }
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async { fetch::newest_release() })
            .await;
        cx.update(|cx| finish(cx, source, installed, started_at, result));
    })
    .detach();
}

fn finish(
    cx: &mut App,
    source: Source,
    installed: Version,
    checked_at: u64,
    result: Result<Option<Version>, FetchError>,
) {
    let record = cx.update_global::<Updates, _>(|updates, _| {
        updates.running = false;
        match result {
            Ok(newest) => {
                updates.record.last_check = Some(checked_at);
                match version::newer_than(newest, installed) {
                    Some(found) => found_update(updates, source, found),
                    None => {
                        updates.status = Status::UpToDate { at: checked_at };
                        if source == Source::Palette {
                            updates.report = Some(Report::UpToDate);
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "the update check failed");
                // An automatic check that fails waits for tomorrow and says nothing.
                if source == Source::Startup {
                    updates.record.last_check = Some(checked_at);
                    updates.status = Status::Unchecked;
                } else {
                    updates.status = Status::Failed;
                }
                if source == Source::Palette {
                    updates.report = Some(Report::Failed);
                }
            }
        }
        updates.record.clone()
    });
    save_in_background(record, cx);
}

// A check the user asked for always shows what it found; the automatic one tells once per
// version and not again after Later.
fn found_update(updates: &mut Updates, source: Source, found: Version) {
    updates.status = Status::Available(found);
    let announce = source != Source::Startup
        || updates
            .record
            .should_announce(found, updates.silenced_this_session);
    if announce {
        updates.record.announced = Some(found);
        updates.report = Some(Report::Available(found));
    }
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

#[cfg(test)]
mod tests {
    use super::{Record, Report, Source, Status, Updates, Version, found_update};

    fn updates() -> Updates {
        Updates {
            record: Record::default(),
            status: Status::Unchecked,
            silenced_this_session: false,
            running: false,
            report: None,
        }
    }

    fn found() -> Version {
        Version::parse_bare("0.1.1").unwrap()
    }

    #[test]
    fn the_automatic_check_announces_a_version_once() {
        let mut updates = updates();
        found_update(&mut updates, Source::Startup, found());
        assert_eq!(updates.report.take(), Some(Report::Available(found())));
        found_update(&mut updates, Source::Startup, found());
        assert_eq!(updates.report, None);
        assert_eq!(updates.status, Status::Available(found()));
    }

    #[test]
    fn a_newer_version_is_announced_after_an_older_one() {
        let mut updates = updates();
        found_update(&mut updates, Source::Startup, found());
        let newer = Version::parse_bare("0.1.2").unwrap();
        found_update(&mut updates, Source::Startup, newer);
        assert_eq!(updates.report, Some(Report::Available(newer)));
    }

    #[test]
    fn later_keeps_the_automatic_check_quiet_for_the_session() {
        let mut updates = updates();
        updates.silenced_this_session = true;
        found_update(&mut updates, Source::Startup, found());
        assert_eq!(updates.report, None);
        assert_eq!(updates.status, Status::Available(found()));
    }

    #[test]
    fn a_check_the_user_asked_for_always_reports() {
        let mut updates = updates();
        updates.silenced_this_session = true;
        updates.record.announced = Some(found());
        found_update(&mut updates, Source::Palette, found());
        assert_eq!(updates.report, Some(Report::Available(found())));
    }
}
