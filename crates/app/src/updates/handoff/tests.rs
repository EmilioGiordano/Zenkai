use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::prepare::{asset_name, new_app, signature_name};
use super::super::version::Version;
use super::{Blocked, Drive, StageError, apply_arguments, blocked, drive_of, stage};

fn version(text: &str) -> Version {
    Version::parse_bare(text).unwrap()
}

fn installed() -> Version {
    version("0.1.0")
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

// An install root whose `.update` holds the verified 0.2.0 download, as branch 4 leaves it.
fn downloaded() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let update = root.path().join(".update");
    fs::create_dir_all(&update).unwrap();
    let wanted = version("0.2.0");
    fs::copy(
        fixture("zenkai-v0.2.0-windows-x64.zip"),
        update.join(asset_name(wanted)),
    )
    .unwrap();
    fs::copy(fixture("good.minisig"), update.join(signature_name(wanted))).unwrap();
    root
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
fn the_launcher_gets_the_version_as_its_grammar_spells_it_and_the_restart_flag() {
    assert_eq!(
        apply_arguments(version("0.2.0")),
        ["--apply=0.2.0".to_owned(), "--restarted".to_owned()]
    );
    assert_eq!(
        apply_arguments(version("10.0.99999"))[0],
        "--apply=10.0.99999"
    );
}

#[test]
fn shares_and_unplaced_paths_are_told_apart_from_drive_letters() {
    assert_eq!(drive_of(Path::new(r"C:\Zenkai")), Drive::Letter(b'C'));
    assert_eq!(drive_of(Path::new(r"\\?\D:\Zenkai")), Drive::Letter(b'D'));
    assert_eq!(drive_of(Path::new(r"\\server\share\Zenkai")), Drive::Share);
    assert_eq!(
        drive_of(Path::new(r"\\?\UNC\server\share\Zenkai")),
        Drive::Share
    );
    assert_eq!(drive_of(Path::new(r"\\.\pipe\Zenkai")), Drive::Unplaced);
    assert_eq!(drive_of(Path::new(r"Zenkai\app")), Drive::Unplaced);
}

#[test]
fn a_share_is_never_updated() {
    assert_eq!(
        blocked(Path::new(r"\\server\share\Zenkai")).unwrap(),
        Some(Blocked::NetworkPath)
    );
}

#[test]
fn a_local_folder_this_user_writes_is_not_blocked_and_the_probe_leaves_nothing() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(blocked(root.path()).unwrap(), None);
    assert!(listed(root.path()).is_empty());
}

#[test]
fn a_root_that_cannot_be_probed_is_a_disk_error() {
    let root = tempfile::tempdir().unwrap();
    assert!(blocked(&root.path().join("missing")).is_err());
}

#[test]
fn staging_extracts_the_verified_update_from_scratch() {
    let root = downloaded();
    let folder = new_app(&root.path().join(".update"), version("0.2.0"));
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("planted.dll"), b"not from the zip").unwrap();
    stage(root.path(), version("0.2.0"), installed()).unwrap();
    assert!(folder.join("zenkai.exe").is_file());
    assert!(!folder.join("planted.dll").exists());
}

#[test]
fn a_zip_changed_since_the_download_is_refused_and_deleted() {
    let root = downloaded();
    let update = root.path().join(".update");
    stage(root.path(), version("0.2.0"), installed()).unwrap();
    let zip = update.join(asset_name(version("0.2.0")));
    let mut bytes = fs::read(&zip).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    fs::write(&zip, bytes).unwrap();
    let result = stage(root.path(), version("0.2.0"), installed());
    assert!(
        matches!(result, Err(StageError::NotVerified(_))),
        "{result:?}"
    );
    assert!(listed(&update).is_empty());
}

#[test]
fn a_download_that_is_gone_is_a_disk_error_and_nothing_else_is_touched() {
    let root = downloaded();
    let update = root.path().join(".update");
    fs::remove_file(update.join(asset_name(version("0.2.0")))).unwrap();
    fs::write(update.join("confirmed"), b"0.1.0").unwrap();
    let result = stage(root.path(), version("0.2.0"), installed());
    assert!(matches!(result, Err(StageError::Disk(_))), "{result:?}");
    assert_eq!(
        listed(&update),
        ["confirmed", "zenkai-v0.2.0-windows-x64.zip.minisig"]
    );
}

#[test]
fn an_update_folder_that_is_a_junction_is_not_used() {
    let root = downloaded();
    let elsewhere = root.path().join("elsewhere");
    fs::rename(root.path().join(".update"), &elsewhere).unwrap();
    let linked = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(root.path().join(".update"))
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(linked.status.success(), "{linked:?}");
    let result = stage(root.path(), version("0.2.0"), installed());
    assert!(matches!(result, Err(StageError::Disk(_))), "{result:?}");
    assert!(!new_app(&elsewhere, version("0.2.0")).exists());
}
