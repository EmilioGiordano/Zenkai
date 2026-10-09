// Structured fuzzing of the xlsx open path, preflight included: the parts of real files are
// damaged the way a corrupt or hostile file would be. Opening must end in a workbook or a
// clear error, within a bounded time, and a workbook that opens must save and reopen.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use proptest::prelude::*;
use zenkai_engine::{Engine, open_xlsx, save_xlsx_atomic, scan_unsupported, xlsx_bytes};

const HANG_AFTER: Duration = Duration::from_secs(60);

const TOKENS: [&str; 14] = [
    "<f>",
    "</row>",
    r#" r="A0""#,
    r#" r="XFE1048577""#,
    "1048577",
    "-1",
    "9e999",
    "&#0;",
    "]]>",
    "<!ENTITY x SYSTEM 'file:///c:/windows/win.ini'>",
    "<c><f>SUM(A1:XFD1048576)</f></c>",
    "<mergeCell ref=\"A1:A1\"/>",
    "<col min=\"0\" max=\"99999\" width=\"1e9\"/>",
    "<xf numFmtId=\"999\" fontId=\"99\" fillId=\"99\" borderId=\"99\"/>",
];

#[derive(Clone, Debug)]
struct Damage {
    part: prop::sample::Index,
    at: prop::sample::Index,
    how: u8,
    token: usize,
    byte: u8,
}

fn damage() -> impl Strategy<Value = Damage> {
    (
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
        0u8..7,
        0..TOKENS.len(),
        any::<u8>(),
    )
        .prop_map(|(part, at, how, token, byte)| Damage {
            part,
            at,
            how,
            token,
            byte,
        })
}

fn corpus() -> Vec<PathBuf> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat");
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "xlsx"))
        .collect();
    files.sort();
    files
}

fn parts_of(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    (0..archive.len())
        .filter_map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            if entry.is_dir() {
                return None;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            Some((entry.name().to_string(), bytes))
        })
        .collect()
}

fn zip_of(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts {
        zip.start_file(name.as_str(), zip::write::FileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn apply(parts: &mut Vec<(String, Vec<u8>)>, damage: &Damage) {
    let index = damage.part.index(parts.len());
    let bytes = &mut parts[index].1;
    let at = damage.at.index(bytes.len().max(1)).min(bytes.len());
    match damage.how {
        0 if !bytes.is_empty() => {
            let last = bytes.len() - 1;
            bytes[at.min(last)] ^= damage.byte | 1;
        }
        1 => {
            let end = (at + 1 + usize::from(damage.byte)).min(bytes.len());
            bytes.drain(at..end);
        }
        2 => {
            let token = TOKENS[damage.token].as_bytes().to_vec();
            bytes.splice(at..at, token);
        }
        3 => bytes.truncate(at),
        4 => bytes.clear(),
        5 => {
            parts.remove(index);
        }
        _ => parts[index].0 = parts[index].0.to_uppercase(),
    }
}

fn open_with_watchdog(bytes: Vec<u8>) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(|| {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("fuzzed.xlsx");
            std::fs::write(&path, &bytes).unwrap();
            let _ = scan_unsupported(&bytes);
            if let Ok(opened) = open_xlsx(&path) {
                opened.workbook.sheets();
                let saved = folder.path().join("saved.xlsx");
                match xlsx_bytes(&opened.workbook)
                    .and_then(|bytes| save_xlsx_atomic(&bytes, &saved))
                {
                    Ok(()) => {
                        open_xlsx(&saved).expect("a file Zenkai just wrote must reopen");
                    }
                    Err(error) => {
                        assert!(!error.to_string().is_empty());
                        assert!(!saved.exists(), "a failed save left a file behind");
                    }
                }
            }
        });
        let _ = done.send(outcome.is_ok());
    });
    match finished.recv_timeout(HANG_AFTER) {
        Ok(true) => {}
        Ok(false) => panic!("opening a damaged file panicked"),
        Err(_) => panic!("opening a damaged file did not finish in {HANG_AFTER:?}"),
    }
}

#[test]
fn the_undamaged_corpus_opens_saves_and_reopens() {
    for path in corpus() {
        open_with_watchdog(std::fs::read(&path).unwrap());
    }
    assert!(corpus().len() >= 10);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 120, ..ProptestConfig::default() })]

    #[test]
    fn damaged_parts_open_or_fail_cleanly(
        file in any::<prop::sample::Index>(),
        damages in prop::collection::vec(damage(), 1..4),
    ) {
        let files = corpus();
        let mut parts = parts_of(&files[file.index(files.len())]);
        for damage in &damages {
            if parts.is_empty() {
                break;
            }
            apply(&mut parts, damage);
        }
        open_with_watchdog(zip_of(&parts));
    }

    #[test]
    fn damaged_archives_open_or_fail_cleanly(
        file in any::<prop::sample::Index>(),
        flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..6),
        cut in prop::option::of(any::<prop::sample::Index>()),
    ) {
        let files = corpus();
        let mut bytes = std::fs::read(&files[file.index(files.len())]).unwrap();
        for (at, byte) in flips {
            let at = at.index(bytes.len());
            bytes[at] ^= byte | 1;
        }
        if let Some(cut) = cut {
            bytes.truncate(cut.index(bytes.len()));
        }
        open_with_watchdog(bytes);
    }

    #[test]
    fn arbitrary_bytes_are_not_a_workbook(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        open_with_watchdog(bytes);
    }
}
