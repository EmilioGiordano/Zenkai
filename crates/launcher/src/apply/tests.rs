use std::ffi::OsString;
use std::fs;
use std::process::Command;
use std::time::Duration;

use super::apply;
use crate::arguments::Version;
use crate::fake_app::{self, ARGUMENTS, Behavior};
use crate::message::Message;
use crate::notice::Notice;
use crate::start::{Finish, TEST_TIMING, Timing};
use crate::test_install::Install;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn passed() -> Vec<OsString> {
    ARGUMENTS.iter().map(OsString::from).collect()
}

fn run(install: &Install, timing: &Timing) -> Finish {
    apply(&install.folders(), &version("0.2.0"), &passed(), timing)
}

fn with_new_version(behavior: Behavior) -> Install {
    let install = Install::new();
    install.put_version("app", "old");
    fake_app::place(&install.path(".update/new-0.2.0"), behavior);
    install
}

fn assert_restored(install: &Install, finish: &Finish) {
    assert!(
        matches!(finish, Finish::Open(Some(Notice::Restored(Some(v)))) if *v == version("0.2.0")),
        "{finish:?}"
    );
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
    assert!(!install.exists("app.old"));
    assert!(install.rejected().join("zenkai.exe").is_file());
}

fn assert_untouched(install: &Install, finish: &Finish) {
    assert!(
        matches!(finish, Finish::Open(Some(Notice::NotApplied(v))) if *v == version("0.2.0")),
        "{finish:?}"
    );
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
    assert!(!install.exists("app.old"));
    assert!(install.exists(".update/new-0.2.0/zenkai.exe"));
}

#[test]
fn a_new_version_that_confirms_takes_the_place_of_the_old_one() {
    let install = with_new_version(Behavior::Confirm("0.2.0"));
    install.put_version(".update/new-0.3.0", "next");
    let finish = run(&install, &TEST_TIMING);
    assert!(matches!(finish, Finish::Running), "{finish:?}");
    assert!(install.exists("app/fake-behavior.txt"));
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert!(!install.exists("app.old"));
    assert!(!install.exists(".update/new-0.2.0"));
    assert!(install.exists(".update/new-0.3.0"));
    assert!(!install.exists(".update/confirmed"));
}

#[test]
fn a_version_that_never_starts_is_set_aside_and_the_previous_restored() {
    let install = Install::new();
    install.put_version("app", "old");
    install.put_version(".update/new-0.2.0", "not a program");
    let finish = run(&install, &TEST_TIMING);
    assert_restored(&install, &finish);
}

#[test]
fn a_version_that_ends_without_confirming_is_set_aside() {
    let install = with_new_version(Behavior::Exit);
    let finish = run(&install, &TEST_TIMING);
    assert_restored(&install, &finish);
}

#[test]
fn a_version_that_hangs_is_ended_and_set_aside() {
    let install = with_new_version(Behavior::Hang);
    let timing = Timing {
        confirmation: Duration::from_secs(2),
        ..TEST_TIMING
    };
    let finish = run(&install, &timing);
    assert_restored(&install, &finish);
}

#[test]
fn a_marker_for_another_version_does_not_count() {
    let install = with_new_version(Behavior::Confirm("0.2.1"));
    let finish = run(&install, &TEST_TIMING);
    assert_restored(&install, &finish);
}

#[test]
fn a_marker_left_from_before_is_removed_before_the_swap() {
    let install = with_new_version(Behavior::Exit);
    install.write(".update/confirmed", "0.2.0");
    let finish = run(&install, &TEST_TIMING);
    assert_restored(&install, &finish);
}

#[test]
fn a_previous_version_that_does_not_close_is_left_alone() {
    let install = Install::new();
    fake_app::place(&install.path("app"), Behavior::Hang);
    install.put_version(".update/new-0.2.0", "new");
    let mut previous = Command::new(install.path("app/zenkai.exe"))
        .args(ARGUMENTS)
        .spawn()
        .unwrap();
    fake_app::started_with(install.root.path());
    let finish = run(&install, &TEST_TIMING);
    previous.kill().unwrap();
    previous.wait().unwrap();
    assert!(
        matches!(finish, Finish::OpenAndTell(Message::NotClosed)),
        "{finish:?}"
    );
    assert!(install.exists("app/fake-behavior.txt"));
    assert_eq!(
        install.version_in(".update/new-0.2.0").as_deref(),
        Some("new")
    );
}

#[test]
fn an_app_folder_still_in_use_is_not_replaced() {
    let install = Install::new();
    install.put_version("app", "old");
    install.put_version(".update/new-0.2.0", "new");
    let held = install.hold("app/pdfium.dll");
    let finish = run(&install, &TEST_TIMING);
    drop(held);
    assert_untouched(&install, &finish);
}

#[test]
fn a_new_version_that_cannot_move_puts_the_previous_one_back() {
    let install = Install::new();
    install.put_version("app", "old");
    install.put_version(".update/new-0.2.0", "new");
    let held = install.hold(".update/new-0.2.0/pdfium.dll");
    let finish = run(&install, &TEST_TIMING);
    drop(held);
    assert_untouched(&install, &finish);
}

#[test]
fn a_missing_new_version_is_not_applied() {
    let install = Install::new();
    install.put_version("app", "old");
    let finish = run(&install, &TEST_TIMING);
    assert!(matches!(finish, Finish::Open(Some(Notice::NotApplied(_)))));
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
}

#[test]
fn a_new_version_without_a_plain_program_is_not_applied() {
    let install = Install::new();
    install.put_version("app", "old");
    fs::create_dir_all(install.path(".update/new-0.2.0/zenkai.exe")).unwrap();
    let finish = run(&install, &TEST_TIMING);
    assert!(matches!(finish, Finish::Open(Some(Notice::NotApplied(_)))));
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
    assert!(!install.exists("app.old"));
}

#[test]
fn a_new_version_that_is_a_link_elsewhere_is_not_applied() {
    let install = Install::new();
    install.put_version("app", "old");
    install.put_version("elsewhere", "planted");
    let linked = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(install.path(".update\\new-0.2.0"))
        .arg(install.path("elsewhere"))
        .output()
        .unwrap();
    assert!(linked.status.success(), "{linked:?}");
    let finish = run(&install, &TEST_TIMING);
    assert!(matches!(finish, Finish::Open(Some(Notice::NotApplied(_)))));
    assert_eq!(install.version_in("app").as_deref(), Some("old"));
    assert_eq!(install.version_in("elsewhere").as_deref(), Some("planted"));
}

#[test]
fn an_update_cut_off_between_the_renames_before_does_not_block_this_one() {
    let install = Install::new();
    install.put_version("app.old", "old");
    fake_app::place(
        &install.path(".update/new-0.2.0"),
        Behavior::Confirm("0.2.0"),
    );
    let finish = run(&install, &TEST_TIMING);
    assert!(matches!(finish, Finish::Running), "{finish:?}");
    assert!(install.exists("app/fake-behavior.txt"));
    assert!(!install.exists("app.old"));
}

#[test]
fn a_missing_install_is_not_patched_up_by_an_update() {
    let install = Install::new();
    fake_app::place(
        &install.path(".update/new-0.2.0"),
        Behavior::Confirm("0.2.0"),
    );
    let finish = run(&install, &TEST_TIMING);
    assert!(
        matches!(finish, Finish::Tell(Message::Reinstall)),
        "{finish:?}"
    );
    assert!(!install.exists("app"));
}

#[test]
fn an_unfinished_cleanup_before_does_not_take_the_new_version_with_it() {
    let install = with_new_version(Behavior::Confirm("0.2.0"));
    install.put_version("app.old", "older");
    install.write(".update/confirmed", "0.1.9");
    let finish = run(&install, &TEST_TIMING);
    assert!(matches!(finish, Finish::Running), "{finish:?}");
    assert!(install.exists("app/fake-behavior.txt"));
    assert!(!install.exists("app.old"));
}
