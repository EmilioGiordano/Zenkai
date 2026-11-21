use std::fs;

use super::{Plan, Settled, plan, settle};
use crate::disk::{Presence, Running};
use crate::message::Message;
use crate::notice::Notice;
use crate::start::TEST_TIMING;
use crate::test_install::Install;

const PRESENCES: [Presence; 3] = [Presence::Present, Presence::Absent, Presence::Unknown];
const RUNNING: [Running; 3] = [Running::Yes, Running::No, Running::Unknown];

fn not_read<T>() -> T {
    panic!("read a fact this row does not need")
}

#[test]
fn the_rows_of_the_state_table() {
    use Presence::{Absent, Present};
    assert_eq!(plan(Present, Absent, not_read, not_read), Plan::Normal);
    assert_eq!(plan(Absent, Present, not_read, not_read), Plan::Interrupted);
    assert_eq!(
        plan(Present, Present, || Present, not_read),
        Plan::Unfinished
    );
    assert_eq!(
        plan(Present, Present, || Absent, || Running::Yes),
        Plan::Starting
    );
    assert_eq!(
        plan(Present, Present, || Absent, || Running::No),
        Plan::NotStarted
    );
    assert_eq!(plan(Absent, Absent, not_read, not_read), Plan::Missing);
}

// Every combination: a plan that moves or deletes files never rests on a fact that is unknown.
#[test]
fn nothing_moves_on_an_unknown_fact() {
    for app in PRESENCES {
        for old in PRESENCES {
            for marker in PRESENCES {
                for running in RUNNING {
                    let chosen = plan(app, old, || marker, || running);
                    let moves = matches!(
                        chosen,
                        Plan::Interrupted | Plan::Unfinished | Plan::NotStarted
                    );
                    let both = app == Presence::Present && old == Presence::Present;
                    let unknown = app == Presence::Unknown
                        || old == Presence::Unknown
                        || (both && marker == Presence::Unknown)
                        || (both && marker == Presence::Absent && running == Running::Unknown);
                    let case = format!("{app:?} {old:?} {marker:?} {running:?} -> {chosen:?}");
                    if moves {
                        assert!(!unknown, "{case}");
                    }
                    assert_eq!(unknown, chosen == Plan::Unknown, "{case}");
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Marker {
    None,
    Valid,
    Garbage,
}

// Every state a crash can leave on disk, with the app running or not: no version is ever lost,
// `app` always ends as one whole version, and a version is set aside only if it did not start.
#[test]
fn every_state_on_disk_settles_without_losing_a_version() {
    for app in [None, Some("new"), Some("old")] {
        for old in [None, Some("old")] {
            for marker in [Marker::None, Marker::Valid, Marker::Garbage] {
                for running in [false, true] {
                    if running && app.is_none() {
                        continue;
                    }
                    check_state(app, old, marker, running);
                }
            }
        }
    }
}

fn check_state(app: Option<&str>, old: Option<&str>, marker: Marker, running: bool) {
    let case = format!("app {app:?}, old {old:?}, {marker:?}, running {running}");
    let install = Install::new();
    if let Some(name) = app {
        install.put_version("app", name);
    }
    if let Some(name) = old {
        install.put_version("app.old", name);
    }
    match marker {
        Marker::None => {}
        Marker::Valid => install.write(".update/confirmed", "0.2.0"),
        Marker::Garbage => install.write(".update/confirmed", "yes"),
    }
    let held = running.then(|| install.hold("app/zenkai.exe"));
    let settled = settle(&install.folders(), &TEST_TIMING);
    drop(held);
    let marked = matches!(marker, Marker::Valid);
    let in_app = install.version_in("app");
    match (app, old) {
        (Some(app), None) => {
            assert!(
                matches!(settled, Settled::Clean(None)),
                "{case}: {settled:?}"
            );
            assert_eq!(in_app.as_deref(), Some(app), "{case}");
        }
        (None, Some(old)) => {
            assert!(
                matches!(settled, Settled::Clean(Some(Notice::Interrupted))),
                "{case}: {settled:?}"
            );
            assert_eq!(in_app.as_deref(), Some(old), "{case}");
            assert!(!install.exists("app.old"), "{case}");
        }
        (None, None) => {
            assert!(
                matches!(settled, Settled::NoApp(Message::Reinstall)),
                "{case}: {settled:?}"
            );
            assert!(!install.exists("app"), "{case}");
        }
        (Some(app), Some(_)) if marked => {
            assert!(
                matches!(settled, Settled::Clean(None)),
                "{case}: {settled:?}"
            );
            assert_eq!(in_app.as_deref(), Some(app), "{case}");
            assert!(!install.exists("app.old"), "{case}");
            assert!(!install.exists(".update/confirmed"), "{case}");
        }
        (Some(app), Some(_)) if running => {
            assert!(matches!(settled, Settled::AsIs), "{case}: {settled:?}");
            assert_eq!(in_app.as_deref(), Some(app), "{case}");
            assert!(install.exists("app.old"), "{case}");
        }
        (Some(app), Some(old)) => {
            assert!(
                matches!(settled, Settled::Clean(Some(Notice::Restored(None)))),
                "{case}: {settled:?}"
            );
            assert_eq!(in_app.as_deref(), Some(old), "{case}");
            assert!(!install.exists("app.old"), "{case}");
            let rejected = fs::read_to_string(install.rejected().join("zenkai.exe"));
            assert_eq!(rejected.ok().as_deref(), Some(app), "{case}");
        }
    }
}

// The crash points of an update, right after each rename, each followed by a normal start.
#[test]
fn a_crash_between_the_two_renames_restores_the_previous_version() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version(".update/new-0.2.0", "new");
    let settled = settle(&install.folders(), &TEST_TIMING);
    assert!(matches!(settled, Settled::Clean(Some(Notice::Interrupted))));
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
}

#[test]
fn a_crash_after_the_swap_before_the_new_version_confirmed_restores_the_previous_one() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version("app", "new");
    let settled = settle(&install.folders(), &TEST_TIMING);
    assert!(matches!(
        settled,
        Settled::Clean(Some(Notice::Restored(None)))
    ));
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
}

#[test]
fn a_crash_after_the_new_version_confirmed_keeps_it_and_only_drops_the_old_one() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version("app", "new");
    install.write(".update/confirmed", "0.2.0");
    install.write(".update/launcher.log", "kept");
    install.write(".update/notice", "restored");
    install.put_version(".update/new-0.3.0", "newer");
    install.put_version(".update/rejected-7", "bad");
    let settled = settle(&install.folders(), &TEST_TIMING);
    assert!(matches!(settled, Settled::Clean(None)));
    assert_eq!(install.version_in("app").as_deref(), Some("new"));
    assert!(!install.exists("app.old"));
    assert!(!install.exists(".update/confirmed"));
    for kept in ["launcher.log", "notice", "new-0.3.0", "rejected-7"] {
        assert!(install.exists(&format!(".update/{kept}")), "{kept}");
    }
}

#[test]
fn a_crash_after_app_old_was_removed_is_a_normal_start() {
    let install = Install::new();
    install.put_version("app", "new");
    install.write(".update/confirmed", "0.2.0");
    assert!(matches!(
        settle(&install.folders(), &TEST_TIMING),
        Settled::Clean(None)
    ));
    assert_eq!(install.version_in("app").as_deref(), Some("new"));
}

#[test]
fn a_crash_while_restoring_restores_on_the_next_start() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version(".update/rejected-7", "new");
    let settled = settle(&install.folders(), &TEST_TIMING);
    assert!(matches!(settled, Settled::Clean(Some(Notice::Interrupted))));
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
}

#[test]
fn a_version_that_cannot_be_moved_aside_is_opened_as_it_is() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version("app", "new");
    let held = install.hold("app/pdfium.dll");
    let settled = settle(&install.folders(), &TEST_TIMING);
    drop(held);
    assert!(matches!(settled, Settled::AsIs));
    assert_eq!(install.version_in("app").as_deref(), Some("new"));
    assert_eq!(install.version_in("app.old").as_deref(), Some("old"));
}

#[test]
fn an_old_folder_that_cannot_be_removed_keeps_the_marker_for_the_next_start() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version("app", "new");
    install.write(".update/confirmed", "0.2.0");
    let held = install.hold("app.old/pdfium.dll");
    let settled = settle(&install.folders(), &TEST_TIMING);
    drop(held);
    assert!(matches!(settled, Settled::AsIs));
    assert!(install.exists(".update/confirmed"));
    assert_eq!(install.version_in("app").as_deref(), Some("new"));
    assert!(matches!(
        settle(&install.folders(), &TEST_TIMING),
        Settled::Clean(None)
    ));
    assert!(!install.exists("app.old"));
}

#[test]
fn a_version_left_in_flight_is_named_when_it_is_set_aside() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version("app", "new");
    install.write(".update/applying", "0.2.0");
    let settled = settle(&install.folders(), &TEST_TIMING);
    let expected = Notice::Restored(Some(crate::arguments::Version::parse("0.2.0").unwrap()));
    assert!(
        matches!(&settled, Settled::Clean(Some(notice)) if *notice == expected),
        "{settled:?}"
    );
    assert!(!install.exists(".update/applying"));
}

#[test]
fn an_unreadable_record_of_the_version_in_flight_is_not_a_name() {
    let install = Install::new();
    install.put_version("app.old", "old");
    install.put_version("app", "new");
    install.write(".update/applying", "../../x");
    let settled = settle(&install.folders(), &TEST_TIMING);
    assert!(matches!(
        settled,
        Settled::Clean(Some(Notice::Restored(None)))
    ));
}
