use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use super::{TEST_TIMING, run};
use crate::arguments::{self, Request, Version};
use crate::fake_app::{self, ARGUMENTS, Behavior};
use crate::lock::{self, Taken};
use crate::message::Message;
use crate::test_install::Install;

fn passed() -> Vec<OsString> {
    ARGUMENTS.iter().map(OsString::from).collect()
}

#[test]
fn a_normal_start_opens_the_app_with_the_arguments_unchanged() {
    let install = Install::new();
    fake_app::place(&install.path("app"), Behavior::Exit);
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(message.is_none(), "{message:?}");
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert!(!install.exists(".update/lock"));
}

#[test]
fn the_app_keeps_the_working_directory_it_was_started_from() {
    let install = Install::new();
    fake_app::place(&install.path("app"), Behavior::Exit);
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(message.is_none(), "{message:?}");
    let started_in = fake_app::started_in(install.root.path());
    assert_eq!(
        fs::canonicalize(started_in).unwrap(),
        fs::canonicalize(std::env::current_dir().unwrap()).unwrap()
    );
}

#[test]
fn the_apply_argument_never_reaches_the_app() {
    let install = Install::new();
    install.put_version("app", "old");
    fake_app::place(
        &install.path(".update/new-0.2.0"),
        Behavior::Confirm("0.2.0"),
    );
    let mut arguments = vec![OsString::from("--apply=0.2.0")];
    arguments.extend(passed());
    let launch = arguments::parse(arguments);
    let message = run(
        &install.folders(),
        launch.request,
        &launch.passed,
        &TEST_TIMING,
    );
    assert!(message.is_none(), "{message:?}");
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert!(!install.exists("app.old"));
}

#[test]
fn a_start_while_another_launcher_works_says_zenkai_is_busy() {
    let install = Install::new();
    install.put_version("app", "old");
    let Taken::Held(held) = lock::take(
        &install.folders().lock(),
        Duration::ZERO,
        Duration::from_millis(10),
    ) else {
        panic!("no lock");
    };
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    drop(held);
    assert!(matches!(message, Some(Message::Busy)), "{message:?}");
}

#[test]
fn a_restored_version_opens_with_its_notice_for_the_app() {
    let install = Install::new();
    install.put_version("app", "new");
    fake_app::place(&install.path("app.old"), Behavior::Exit);
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(message.is_none(), "{message:?}");
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert_eq!(
        fs::read_to_string(install.path(".update/notice")).unwrap(),
        "restored"
    );
}

#[test]
fn an_update_that_did_not_start_leaves_its_version_in_the_notice() {
    let install = Install::new();
    fake_app::place(&install.path("app"), Behavior::Exit);
    install.put_version(".update/new-0.2.0", "not a program");
    let request = Request::Apply(Version::parse("0.2.0").unwrap());
    let message = run(&install.folders(), request, &passed(), &TEST_TIMING);
    assert!(message.is_none(), "{message:?}");
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert_eq!(
        fs::read_to_string(install.path(".update/notice")).unwrap(),
        "restored 0.2.0"
    );
}

#[test]
fn a_missing_install_says_to_reinstall() {
    let install = Install::new();
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(matches!(message, Some(Message::Reinstall)), "{message:?}");
}

#[test]
fn an_app_that_cannot_start_is_reported() {
    let install = Install::new();
    install.put_version("app", "not a program");
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(
        matches!(message, Some(Message::CouldNotStart(_))),
        "{message:?}"
    );
}

#[test]
fn without_a_place_for_the_lock_the_app_still_opens_untouched() {
    let install = Install::new();
    fake_app::place(&install.path("app"), Behavior::Exit);
    install.put_version("app.old", "old");
    fs::remove_dir_all(install.path(".update")).unwrap();
    install.write(".update", "a file where the folder goes");
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(message.is_none(), "{message:?}");
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert!(install.exists("app.old"));
}

#[test]
fn an_update_folder_that_leads_elsewhere_is_never_used() {
    let install = Install::new();
    fake_app::place(&install.path("app"), Behavior::Exit);
    install.put_version("app.old", "old");
    fs::remove_dir_all(install.path(".update")).unwrap();
    fs::create_dir(install.path("elsewhere")).unwrap();
    install.link(".update", "elsewhere");
    let message = run(&install.folders(), Request::Open, &passed(), &TEST_TIMING);
    assert!(message.is_none(), "{message:?}");
    assert_eq!(fake_app::started_with(install.root.path()), ARGUMENTS);
    assert!(install.exists("app.old"));
    assert_eq!(fs::read_dir(install.path("elsewhere")).unwrap().count(), 0);
}
