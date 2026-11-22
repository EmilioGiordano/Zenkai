use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, System, ZipWriter};

use super::{
    APP_FILES, ArchiveError, Entry, Kind, LIMITS, Limits, MAX_ENTRIES, extract, kind_of, select,
};

const TOP: &str = "zenkai-v0.2.0-windows-x64/";

fn stored() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
}

fn deflated() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}

fn release_entries() -> Vec<(String, Vec<u8>)> {
    let mut entries = vec![
        (format!("{TOP}zenkai.exe"), b"launcher".to_vec()),
        (format!("{TOP}zenkai-mcp.exe"), b"relay".to_vec()),
        (format!("{TOP}LICENSE"), b"license".to_vec()),
    ];
    for file in APP_FILES {
        entries.push((
            format!("{TOP}app/{file}"),
            format!("new {file}").into_bytes(),
        ));
    }
    entries
}

fn zip_of(entries: &[(String, Vec<u8>)], options: SimpleFileOptions) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        if name.ends_with('/') {
            writer.add_directory(name.as_str(), options).unwrap();
        } else {
            writer.start_file(name.as_str(), options).unwrap();
            writer.write_all(bytes).unwrap();
        }
    }
    writer.finish().unwrap().into_inner()
}

fn release_with(extra: &[(&str, &[u8])]) -> Vec<u8> {
    let mut entries = release_entries();
    for (name, bytes) in extra {
        entries.push(((*name).to_owned(), bytes.to_vec()));
    }
    zip_of(&entries, deflated())
}

fn extract_into(zip: Vec<u8>, folder: &Path) -> Result<(), ArchiveError> {
    extract(Cursor::new(zip), &folder.join("new-0.2.0"), LIMITS)
}

fn names(entries: &[&str]) -> Result<Vec<&'static str>, ArchiveError> {
    let entries: Vec<Entry<'_>> = entries
        .iter()
        .map(|name| Entry {
            name: name.as_bytes(),
            kind: Kind::Plain,
        })
        .collect();
    select(&entries).map(|chosen| chosen.into_iter().map(|chosen| chosen.file).collect())
}

fn listed(folder: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    found.sort();
    found
}

// Central directory records start with PK\x01\x02; the flags sit 8 bytes in, the method 10.
fn patch_central(zip: &mut [u8], offset: usize, value: u16) {
    let starts: Vec<usize> = zip
        .windows(4)
        .enumerate()
        .filter(|(_, window)| *window == b"PK\x01\x02")
        .map(|(start, _)| start)
        .collect();
    assert!(!starts.is_empty());
    for start in starts {
        zip[start + offset..start + offset + 2].copy_from_slice(&value.to_le_bytes());
    }
}

#[test]
fn only_the_app_files_are_extracted() {
    let folder = tempfile::tempdir().unwrap();
    extract_into(release_with(&[]), folder.path()).unwrap();
    let extracted = folder.path().join("new-0.2.0");
    let mut expected: Vec<String> = APP_FILES.iter().map(|file| (*file).to_owned()).collect();
    expected.sort();
    assert_eq!(listed(&extracted), expected);
    assert_eq!(
        fs::read(extracted.join("zenkai.exe")).unwrap(),
        b"new zenkai.exe"
    );
}

#[test]
fn folder_entries_and_unknown_files_are_left_out() {
    let mut entries = vec![
        (TOP.to_owned(), Vec::new()),
        (format!("{TOP}app/"), Vec::new()),
    ];
    entries.extend(release_entries());
    entries.push((format!("{TOP}app/extra.dll"), b"x".to_vec()));
    entries.push((format!("{TOP}app/sub/zenkai.exe"), b"x".to_vec()));
    entries.push((format!("{TOP}other/pdfium.dll"), b"x".to_vec()));
    let folder = tempfile::tempdir().unwrap();
    extract_into(zip_of(&entries, stored()), folder.path()).unwrap();
    assert_eq!(
        listed(&folder.path().join("new-0.2.0")).len(),
        APP_FILES.len()
    );
}

#[test]
fn the_signed_fixture_extracts_its_app_folder() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("update")
        .join("zenkai-v0.2.0-windows-x64.zip");
    let folder = tempfile::tempdir().unwrap();
    extract_into(fs::read(path).unwrap(), folder.path()).unwrap();
    let extracted = folder.path().join("new-0.2.0");
    assert_eq!(listed(&extracted).len(), APP_FILES.len());
    assert_eq!(
        fs::read(extracted.join("zenkai.exe")).unwrap(),
        b"app 0.2.0"
    );
}

#[test]
fn a_zip_without_the_app_program_is_refused() {
    let entries: Vec<(String, Vec<u8>)> = release_entries()
        .into_iter()
        .filter(|(name, _)| !name.ends_with("app/zenkai.exe"))
        .collect();
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip_of(&entries, stored()), folder.path()),
        Err(ArchiveError::NoProgram)
    ));
    assert!(!folder.path().join("new-0.2.0").exists());
}

#[test]
fn hostile_names_refuse_the_whole_zip() {
    for name in [
        "zenkai-v0.2.0-windows-x64/../escaped.txt",
        "zenkai-v0.2.0-windows-x64/app/../../escaped.txt",
        "zenkai-v0.2.0-windows-x64/./app/zenkai.exe",
        "/zenkai-v0.2.0-windows-x64/app/x.txt",
        "zenkai-v0.2.0-windows-x64//x.txt",
        "zenkai-v0.2.0-windows-x64\\app\\pdfium.dll",
        "zenkai-v0.2.0-windows-x64/C:/x.txt",
        "zenkai-v0.2.0-windows-x64/app/zenkai.exe:stream",
        "zenkai-v0.2.0-windows-x64/app/pdfium.dll::$DATA",
        "zenkai-v0.2.0-windows-x64/CON",
        "zenkai-v0.2.0-windows-x64/app/nul.txt",
        "zenkai-v0.2.0-windows-x64/Com1.dll",
        "zenkai-v0.2.0-windows-x64/lpt9",
        "zenkai-v0.2.0-windows-x64/CONOUT$",
        "zenkai-v0.2.0-windows-x64/aux .txt",
        "zenkai-v0.2.0-windows-x64/app/zenkai.exe.",
        "zenkai-v0.2.0-windows-x64/app/zenkai.exe ",
        "zenkai-v0.2.0-windows-x64/readme. ",
        "zenkai-v0.2.0-windows-x64/what?.txt",
        "zenkai-v0.2.0-windows-x64/star*.txt",
    ] {
        let folder = tempfile::tempdir().unwrap();
        let result = extract_into(release_with(&[(name, &b"x"[..])]), folder.path());
        assert!(
            matches!(result, Err(ArchiveError::UnsafeName(_))),
            "{name:?}: {result:?}"
        );
        assert!(!folder.path().join("new-0.2.0").exists(), "{name:?}");
        assert!(!folder.path().join("escaped.txt").exists(), "{name:?}");
    }
}

#[test]
fn control_and_non_ascii_characters_are_refused() {
    for name in [
        "zenkai-v0.2.0-windows-x64/a\u{0}b",
        "zenkai-v0.2.0-windows-x64/tab\there",
        "zenkai-v0.2.0-windows-x64/line\nbreak",
        "zenkai-v0.2.0-windows-x64/caf\u{e9}.txt",
        "zenkai-v0.2.0-windows-x64/app/zenkai\u{2024}exe",
        "zenkai-v0.2.0-windows-x64/COM\u{b9}",
        "zenkai-v0.2.0-windows-x64/del\u{7f}",
    ] {
        let entries = [name, "zenkai-v0.2.0-windows-x64/app/zenkai.exe"];
        assert!(
            matches!(names(&entries), Err(ArchiveError::UnsafeName(_))),
            "{name:?}"
        );
    }
}

#[test]
fn an_exact_repeated_name_refuses_the_whole_zip() {
    let mut entries = release_entries();
    entries.push((format!("{TOP}app/zenkai.exf"), b"second".to_vec()));
    let mut zip = zip_of(&entries, stored());
    let starts: Vec<usize> = zip
        .windows(b"app/zenkai.exf".len())
        .enumerate()
        .filter(|(_, window)| *window == b"app/zenkai.exf")
        .map(|(start, _)| start)
        .collect();
    assert_eq!(starts.len(), 2);
    for start in starts {
        zip[start + b"app/zenkai.ex".len()] = b'e';
    }
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip, folder.path()),
        Err(ArchiveError::RepeatedName)
    ));
}

#[test]
fn a_name_past_the_length_limit_is_refused() {
    let long = format!("{TOP}{}", "a".repeat(300));
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(release_with(&[(long.as_str(), &b"x"[..])]), folder.path()),
        Err(ArchiveError::UnsafeName(_))
    ));
}

#[test]
fn names_that_differ_only_in_case_refuse_the_whole_zip() {
    for name in [
        "zenkai-v0.2.0-windows-x64/app/ZENKAI.EXE",
        "zenkai-v0.2.0-windows-x64/App/pdfium.dll",
        "zenkai-v0.2.0-windows-x64/license",
    ] {
        let folder = tempfile::tempdir().unwrap();
        let result = extract_into(release_with(&[(name, &b"x"[..])]), folder.path());
        assert!(
            matches!(result, Err(ArchiveError::Duplicate(_))),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn a_file_and_a_folder_of_the_same_name_refuse_the_zip() {
    assert!(matches!(
        names(&[
            "zenkai-v0.2.0-windows-x64/app/zenkai.exe",
            "zenkai-v0.2.0-windows-x64/app/zenkai.exe/",
        ]),
        Err(ArchiveError::Duplicate(_))
    ));
    assert!(matches!(
        names(&[
            "zenkai-v0.2.0-windows-x64/app",
            "zenkai-v0.2.0-windows-x64/APP/zenkai.exe",
        ]),
        Err(ArchiveError::FileAsFolder(_))
    ));
}

#[test]
fn everything_must_sit_under_one_top_level_folder() {
    assert!(matches!(
        names(&[
            "zenkai-v0.2.0-windows-x64/app/zenkai.exe",
            "other/app/zenkai.exe",
        ]),
        Err(ArchiveError::NotOneTopFolder)
    ));
    assert!(matches!(
        names(&["zenkai-v0.2.0-windows-x64/app/zenkai.exe", "loose.txt"]),
        Err(ArchiveError::NotOneTopFolder)
    ));
    assert!(matches!(
        names(&["app/zenkai.exe"]),
        Err(ArchiveError::NoProgram)
    ));
    assert_eq!(names(&["any-name/app/zenkai.exe"]).unwrap(), ["zenkai.exe"]);
}

#[test]
fn a_symlink_refuses_the_whole_zip() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in release_entries() {
        writer.start_file(name, stored()).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer
        .add_symlink(format!("{TOP}docs"), "C:\\Windows", stored())
        .unwrap();
    let zip = writer.finish().unwrap().into_inner();
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip, folder.path()),
        Err(ArchiveError::Link(_))
    ));
}

#[test]
fn special_files_and_reparse_points_count_as_links() {
    assert_eq!(kind_of(Some(0o100_644), 0x20, false), Kind::Plain);
    assert_eq!(kind_of(Some(0o040_755), 0x10, false), Kind::Plain);
    assert_eq!(kind_of(Some(0o644), 0, false), Kind::Plain);
    assert_eq!(kind_of(None, 0x20, false), Kind::Plain);
    assert_eq!(kind_of(Some(0o120_777), 0, true), Kind::Link);
    for special in [0o010_644, 0o020_644, 0o060_644, 0o140_644] {
        assert_eq!(kind_of(Some(special), 0, false), Kind::Link, "{special:o}");
    }
    assert_eq!(kind_of(None, 0x400, false), Kind::Link);
}

#[test]
fn a_special_file_in_the_zip_refuses_it() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in release_entries() {
        writer.start_file(name, stored()).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer
        .start_file(
            format!("{TOP}pipe"),
            stored()
                .system(System::Unix)
                .external_attributes(0o010_644 << 16),
        )
        .unwrap();
    let zip = writer.finish().unwrap().into_inner();
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip, folder.path()),
        Err(ArchiveError::Link(_))
    ));
}

#[test]
fn a_compression_bomb_stops_at_the_ratio_limit() {
    let mut entries = release_entries();
    entries.retain(|(name, _)| !name.ends_with("app/pdfium.dll"));
    entries.push((format!("{TOP}app/pdfium.dll"), vec![0; 4 * 1024 * 1024]));
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip_of(&entries, deflated()), folder.path()),
        Err(ArchiveError::Ratio)
    ));
    assert!(!folder.path().join("new-0.2.0").exists());
}

#[test]
fn the_total_is_counted_on_the_bytes_written() {
    let zip = zip_of(&release_entries(), stored());
    let folder = tempfile::tempdir().unwrap();
    let tight = Limits {
        total_bytes: 40,
        ratio: LIMITS.ratio,
    };
    assert!(matches!(
        extract(
            Cursor::new(zip.clone()),
            &folder.path().join("new-0.2.0"),
            tight
        ),
        Err(ArchiveError::TooLarge(40))
    ));
    assert!(!folder.path().join("new-0.2.0").exists());
    extract(Cursor::new(zip), &folder.path().join("new-0.2.0"), LIMITS).unwrap();
}

#[test]
fn a_declared_size_that_lies_is_refused() {
    let mut entries = release_entries();
    entries.retain(|(name, _)| !name.ends_with("app/zenkai.exe"));
    entries.insert(0, (format!("{TOP}app/zenkai.exe"), vec![7; 4096]));
    let mut zip = zip_of(&entries[..1], deflated());
    patch_central(&mut zip, 24, 16);
    let folder = tempfile::tempdir().unwrap();
    assert!(extract_into(zip, folder.path()).is_err());
    assert!(!folder.path().join("new-0.2.0").exists());
}

#[test]
fn encrypted_or_oddly_compressed_entries_are_refused() {
    let mut encrypted = zip_of(&release_entries(), stored());
    patch_central(&mut encrypted, 8, 1);
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(encrypted, folder.path()),
        Err(ArchiveError::Encrypted)
    ));
    let mut bzip2 = zip_of(&release_entries(), stored());
    patch_central(&mut bzip2, 10, 12);
    assert!(matches!(
        extract_into(bzip2, folder.path()),
        Err(ArchiveError::Method)
    ));
}

#[test]
fn a_damaged_entry_is_refused_and_leaves_nothing() {
    let mut zip = zip_of(&release_entries(), stored());
    let at = zip
        .windows(b"new zenkai.exe".len())
        .position(|window| window == b"new zenkai.exe")
        .unwrap();
    zip[at] ^= 0x01;
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip, folder.path()),
        Err(ArchiveError::Damaged(_))
    ));
    assert!(!folder.path().join("new-0.2.0").exists());
}

#[test]
fn too_many_entries_are_refused_before_any_is_read() {
    let mut entries = release_entries();
    for index in 0..MAX_ENTRIES {
        entries.push((format!("{TOP}docs/{index}.txt"), Vec::new()));
    }
    let folder = tempfile::tempdir().unwrap();
    assert!(matches!(
        extract_into(zip_of(&entries, stored()), folder.path()),
        Err(ArchiveError::TooManyEntries)
    ));
}

#[test]
fn an_existing_destination_is_never_written_into() {
    let folder = tempfile::tempdir().unwrap();
    let destination = folder.path().join("new-0.2.0");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("kept.txt"), b"kept").unwrap();
    assert!(matches!(
        extract(Cursor::new(release_with(&[])), &destination, LIMITS),
        Err(ArchiveError::Write(_))
    ));
    assert_eq!(fs::read(destination.join("kept.txt")).unwrap(), b"kept");
}
