// Structured fuzzing of what the update download reads from outside: the minisign signature and
// its trusted comment, the zip entry names, and the zip itself. Damaged input is refused, never
// a panic, and whatever is accepted is exactly what the checks promise.
#![cfg(windows)]
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the modules are included whole; the test uses only part of them"
)]

#[path = "../src/updates/archive.rs"]
mod archive;
#[path = "../src/updates/signature.rs"]
mod signature;
#[path = "../src/updates/version.rs"]
mod version;

use std::collections::HashSet;
use std::io::Cursor;
use std::path::PathBuf;

use archive::{APP_FILES, Entry, Kind, LIMITS};
use proptest::prelude::*;
use version::Version;

const COMMENT: &str = "zenkai/0.2.0/windows-x64";

fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "update",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(path).unwrap()
}

fn version(text: &str) -> Version {
    Version::parse_bare(text).unwrap()
}

fn from_pieces(pieces: &'static [&'static str], count: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(pieces), 0..count).prop_map(|parts| parts.concat())
}

fn comment_text() -> impl Strategy<Value = String> {
    const PIECES: &[&str] = &[
        "zenkai",
        "Zenkai",
        "/",
        "/",
        "0",
        "1",
        "2",
        "00",
        ".",
        "v",
        "windows-x64",
        "linux-x64",
        " ",
        "\t",
        "-",
        "\u{e9}",
        "\u{0}",
        "4294967296",
    ];
    prop_oneof![
        from_pieces(PIECES, 12),
        prop::collection::vec(any::<char>(), 0..40).prop_map(String::from_iter),
    ]
}

// Each edit is (where, what, byte): replace, insert or delete one byte of the text.
fn edited(original: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec((any::<usize>(), 0u8..3, any::<u8>()), 1..6).prop_map(move |edits| {
        let mut text = original.clone();
        for (at, what, byte) in edits {
            let at = at % (text.len() + 1);
            match what {
                0 if at < text.len() => text[at] = byte,
                1 => text.insert(at, byte),
                _ if at < text.len() => {
                    text.remove(at);
                }
                _ => text.push(byte),
            }
        }
        text
    })
}

fn entry_name() -> impl Strategy<Value = String> {
    const PIECES: &[&str] = &[
        "zenkai-v0.2.0-windows-x64/",
        "zenkai-v0.2.0-windows-x64/",
        "zenkai-v0.2.0-windows-x64/",
        "other/",
        "app/",
        "app/",
        "APP/",
        "zenkai.exe",
        "zenkai.exe",
        "ZENKAI.EXE",
        "pdfium.dll",
        "pdfium-LICENSE.txt",
        "LICENSE",
        "../",
        "./",
        "/",
        "CON",
        "nul.txt",
        "a:b",
        "a\\b",
        "x ",
        "x.",
        "\u{e9}",
        "\u{0}",
    ];
    from_pieces(PIECES, 5)
}

fn entries() -> impl Strategy<Value = Vec<(String, bool)>> {
    prop::collection::vec((entry_name(), prop::bool::weighted(0.05)), 0..8)
}

fn plain_component(component: &str) -> bool {
    let device = ["CON", "PRN", "AUX", "NUL"].iter().any(|device| {
        component
            .split('.')
            .next()
            .unwrap()
            .eq_ignore_ascii_case(device)
    });
    !component.is_empty()
        && !component.ends_with('.')
        && !component.ends_with(' ')
        && !device
        && component
            .bytes()
            .all(|byte| (b' '..=b'~').contains(&byte) && !b"\\:*?\"<>|".contains(&byte))
}

#[test]
fn the_baseline_signature_verifies() {
    let zip = fixture("zenkai-v0.2.0-windows-x64.zip");
    signature::verify(
        &mut &zip[..],
        &fixture("good.minisig"),
        version("0.2.0"),
        version("0.1.0"),
    )
    .unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn a_comment_that_parses_is_exactly_its_three_fields(comment in comment_text()) {
        if let Some(signed) = signature::parse_comment(&comment) {
            prop_assert_eq!(format!("zenkai/{}/{}", signed.version, signed.platform), comment);
        }
    }

    #[test]
    fn a_damaged_signature_never_verifies_another_comment(text in edited(fixture("good.minisig"))) {
        let zip = fixture("zenkai-v0.2.0-windows-x64.zip");
        let result = signature::verify(&mut &zip[..], &text, version("0.2.0"), version("0.1.0"));
        if result.is_ok() {
            let decoded = signature::decode(&text).unwrap();
            prop_assert_eq!(decoded.trusted_comment(), COMMENT);
        }
    }

    #[test]
    fn any_bytes_are_a_signature_or_an_error(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        if let Ok(decoded) = signature::decode(&bytes) {
            decoded.trusted_comment();
        }
    }

    #[test]
    fn accepted_entries_are_plain_unique_and_choose_only_app_files(listed in entries()) {
        let kinds: Vec<Entry<'_>> = listed
            .iter()
            .map(|(name, link)| Entry {
                name: name.as_bytes(),
                kind: if *link { Kind::Link } else { Kind::Plain },
            })
            .collect();
        if let Ok(chosen) = archive::select(&kinds) {
            let mut seen = HashSet::new();
            let mut tops = HashSet::new();
            for (name, link) in &listed {
                prop_assert!(!link);
                let path = name.strip_suffix('/').unwrap_or(name);
                prop_assert!(path.split('/').all(plain_component), "{name:?}");
                prop_assert!(seen.insert(path.to_ascii_lowercase()), "{name:?}");
                tops.insert(path.split('/').next().unwrap().to_owned());
            }
            prop_assert_eq!(tops.len(), 1);
            let mut files = HashSet::new();
            for choice in &chosen {
                prop_assert!(APP_FILES.contains(&choice.file));
                prop_assert!(files.insert(choice.file));
                let parts: Vec<&str> = listed[choice.index].0.split('/').collect();
                prop_assert_eq!(parts.len(), 3);
                prop_assert_eq!(parts[1], "app");
                prop_assert_eq!(parts[2], choice.file);
            }
            prop_assert!(files.contains("zenkai.exe"));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn a_damaged_zip_is_refused_whole_or_gives_only_app_files(bytes in edited(fixture("zenkai-v0.2.0-windows-x64.zip"))) {
        let folder = tempfile::tempdir().unwrap();
        let destination = folder.path().join("new-0.2.0");
        match archive::extract(Cursor::new(bytes), &destination, LIMITS) {
            Ok(()) => {
                for entry in std::fs::read_dir(&destination).unwrap() {
                    let name = entry.unwrap().file_name().into_string().unwrap();
                    prop_assert!(APP_FILES.contains(&name.as_str()), "{name}");
                }
            }
            Err(_) => prop_assert!(!destination.exists()),
        }
        let outside: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "new-0.2.0")
            .collect();
        prop_assert!(outside.is_empty(), "{outside:?}");
    }
}
