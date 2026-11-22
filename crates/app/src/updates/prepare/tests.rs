use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::archive::APP_FILES;
use super::super::download::DownloadError;
use super::super::signature::SignatureError;
use super::super::version::Version;
use super::{
    MAX_TRIES, PendingError, PrepareError, asset_name, new_app, prepare, signature_name,
    verify_and_extract,
};

fn version(text: &str) -> Version {
    Version::parse_bare(text).unwrap()
}

fn installed() -> Version {
    version("0.1.0")
}

fn calm() -> AtomicBool {
    AtomicBool::new(false)
}

fn fixture(name: &str) -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "update",
        name,
    ]
    .iter()
    .collect()
}

const GOOD: (&str, &str) = ("zenkai-v0.2.0-windows-x64.zip", "good.minisig");
const STRANGER: (&str, &str) = ("zenkai-v0.2.0-windows-x64.zip", "wrong-key.minisig");
const HOSTILE: (&str, &str) = ("hostile.zip", "hostile.minisig");

fn serve(update: &Path, version: Version, (zip, signature): (&str, &str)) {
    fs::copy(fixture(zip), update.join(asset_name(version))).unwrap();
    fs::copy(fixture(signature), update.join(signature_name(version))).unwrap();
}

fn listed(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn the_names_match_the_release_and_the_launcher() {
    assert_eq!(
        asset_name(version("0.2.0")),
        "zenkai-v0.2.0-windows-x64.zip"
    );
    assert_eq!(
        signature_name(version("0.2.0")),
        "zenkai-v0.2.0-windows-x64.zip.minisig"
    );
    assert_eq!(
        new_app(Path::new(".update"), version("0.2.0")),
        Path::new(".update").join("new-0.2.0")
    );
}

#[test]
fn a_verified_release_is_extracted_and_its_zip_kept() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let found = prepare(
        &update,
        version("0.2.0"),
        &[],
        installed(),
        &calm(),
        |asked| {
            serve(&update, asked, GOOD);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(found, version("0.2.0"));
    let extracted = new_app(&update, found);
    assert_eq!(listed(&extracted).len(), APP_FILES.len());
    assert_eq!(
        fs::read(extracted.join("zenkai.exe")).unwrap(),
        b"app 0.2.0"
    );
    assert!(update.join(asset_name(found)).is_file());
    assert!(update.join(signature_name(found)).is_file());
}

#[test]
fn a_release_that_does_not_verify_is_deleted_and_the_previous_one_tried() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let asked = RefCell::new(Vec::new());
    // The zip of 0.2.0 served as 0.3.0: well signed, but its comment names another version.
    let found = prepare(
        &update,
        version("0.3.0"),
        &[version("0.2.0")],
        installed(),
        &calm(),
        |wanted| {
            asked.borrow_mut().push(wanted);
            serve(&update, wanted, GOOD);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(found, version("0.2.0"));
    assert_eq!(*asked.borrow(), [version("0.3.0"), version("0.2.0")]);
    assert!(!update.join(asset_name(version("0.3.0"))).exists());
    assert!(!update.join(signature_name(version("0.3.0"))).exists());
    assert!(!new_app(&update, version("0.3.0")).exists());
}

#[test]
fn no_more_than_three_versions_are_tried_per_request() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let asked = RefCell::new(Vec::new());
    let older = [version("0.4.0"), version("0.3.0"), version("0.2.0")];
    let result = prepare(
        &update,
        version("0.5.0"),
        &older,
        installed(),
        &calm(),
        |wanted| {
            asked.borrow_mut().push(wanted);
            serve(&update, wanted, STRANGER);
            Ok(())
        },
    );
    assert!(matches!(
        result,
        Err(PrepareError::NotVerified(newest)) if newest == version("0.5.0")
    ));
    assert_eq!(asked.borrow().len(), MAX_TRIES);
    assert!(listed(&update).is_empty());
}

#[test]
fn a_hostile_zip_that_verifies_is_refused_and_leaves_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let result = prepare(
        &update,
        version("0.2.0"),
        &[],
        installed(),
        &calm(),
        |wanted| {
            serve(&update, wanted, HOSTILE);
            Ok(())
        },
    );
    assert!(matches!(result, Err(PrepareError::NotVerified(_))));
    assert!(listed(&update).is_empty());
    assert!(!folder.path().join("escaped.txt").exists());
}

#[test]
fn a_release_without_the_file_hands_over_to_the_previous_one() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let found = prepare(
        &update,
        version("0.3.0"),
        &[version("0.2.0")],
        installed(),
        &calm(),
        |wanted| {
            if wanted == version("0.3.0") {
                return Err(DownloadError::Status(404));
            }
            serve(&update, wanted, GOOD);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(found, version("0.2.0"));
}

#[test]
fn a_failed_or_cancelled_download_stops_and_cleans_up() {
    for failure in [
        DownloadError::Cancelled,
        DownloadError::Transfer(io::Error::other("reset")),
        DownloadError::Status(500),
    ] {
        let folder = tempfile::tempdir().unwrap();
        let update = folder.path().join(".update");
        let asked = RefCell::new(0);
        let mut failure = Some(failure);
        let result = prepare(
            &update,
            version("0.3.0"),
            &[version("0.2.0")],
            installed(),
            &calm(),
            |wanted| {
                *asked.borrow_mut() += 1;
                fs::write(update.join(asset_name(wanted)), b"part").unwrap();
                Err(failure.take().unwrap())
            },
        );
        assert!(matches!(result, Err(PrepareError::Download(_))));
        assert_eq!(*asked.borrow(), 1);
        assert!(listed(&update).is_empty());
    }
}

#[test]
fn the_kept_zip_is_verified_again_from_disk() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    prepare(
        &update,
        version("0.2.0"),
        &[],
        installed(),
        &calm(),
        |wanted| {
            serve(&update, wanted, GOOD);
            Ok(())
        },
    )
    .unwrap();
    verify_and_extract(&update, version("0.2.0"), installed()).unwrap();

    let zip = update.join(asset_name(version("0.2.0")));
    let mut bytes = fs::read(&zip).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&zip, bytes).unwrap();
    let result = verify_and_extract(&update, version("0.2.0"), installed());
    assert!(matches!(
        result,
        Err(PendingError::Signature(SignatureError::Invalid))
    ));
    assert!(!new_app(&update, version("0.2.0")).exists());
}

#[test]
fn an_extracted_folder_from_before_is_replaced_whole() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    fs::create_dir_all(new_app(&update, version("0.2.0"))).unwrap();
    serve(&update, version("0.2.0"), GOOD);
    fs::write(new_app(&update, version("0.2.0")).join("stale.dll"), b"old").unwrap();
    verify_and_extract(&update, version("0.2.0"), installed()).unwrap();
    assert!(
        !new_app(&update, version("0.2.0"))
            .join("stale.dll")
            .exists()
    );
}

#[test]
fn earlier_downloads_go_and_the_launcher_files_stay() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    fs::create_dir_all(new_app(&update, version("0.1.5"))).unwrap();
    fs::create_dir_all(update.join("rejected-42")).unwrap();
    for file in [
        "zenkai-v0.1.5-windows-x64.zip",
        "zenkai-v0.1.5-windows-x64.zip.minisig",
        "lock",
        "confirmed",
        "applying",
        "notice",
        "launcher.log",
    ] {
        fs::write(update.join(file), b"x").unwrap();
    }
    prepare(
        &update,
        version("0.2.0"),
        &[],
        installed(),
        &calm(),
        |wanted| {
            serve(&update, wanted, GOOD);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        listed(&update),
        [
            "applying",
            "confirmed",
            "launcher.log",
            "lock",
            "new-0.2.0",
            "notice",
            "rejected-42",
            "zenkai-v0.2.0-windows-x64.zip",
            "zenkai-v0.2.0-windows-x64.zip.minisig",
        ]
    );
}

#[test]
fn an_update_folder_that_is_not_a_folder_stops_everything() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    fs::write(&update, b"not a folder").unwrap();
    let result = prepare(&update, version("0.2.0"), &[], installed(), &calm(), |_| {
        panic!("nothing is downloaded")
    });
    assert!(matches!(result, Err(PrepareError::Disk(_))));
}

#[test]
fn a_cancel_before_a_try_downloads_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let result = prepare(
        &update,
        version("0.2.0"),
        &[],
        installed(),
        &AtomicBool::new(true),
        |_| panic!("nothing is downloaded after a cancel"),
    );
    assert!(matches!(
        result,
        Err(PrepareError::Download(DownloadError::Cancelled))
    ));
}

#[test]
fn a_cancel_during_the_checks_removes_what_the_try_left() {
    let folder = tempfile::tempdir().unwrap();
    let update = folder.path().join(".update");
    let cancel = AtomicBool::new(false);
    let result = prepare(
        &update,
        version("0.2.0"),
        &[],
        installed(),
        &cancel,
        |wanted| {
            serve(&update, wanted, GOOD);
            cancel.store(true, Ordering::Relaxed);
            Ok(())
        },
    );
    assert!(matches!(
        result,
        Err(PrepareError::Download(DownloadError::Cancelled))
    ));
    assert!(listed(&update).is_empty());
}
