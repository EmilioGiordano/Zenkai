// Structured fuzzing of the xlsx open path, preflight included: the parts of real files are
// damaged the way a corrupt or hostile file would be. Opening must end in a workbook or a
// clear error, within a bounded time, and a workbook that opens must save and reopen. The
// fast reader is held to the same.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::mpsc;
use std::time::Duration;

use proptest::prelude::*;
use zenkai_engine::{
    Engine, XlsxReader, open_xlsx, open_xlsx_with, save_xlsx_atomic, scan_unsupported,
};

#[path = "../../../test-support/xlsx_damage.rs"]
mod xlsx_damage;

use xlsx_damage::{apply, corpus, damage, parts_of, zip_of};

const HANG_AFTER: Duration = Duration::from_secs(60);

fn open_with_watchdog(bytes: Vec<u8>) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(|| {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("fuzzed.xlsx");
            std::fs::write(&path, &bytes).unwrap();
            let _ = scan_unsupported(&bytes);
            let _ = open_xlsx_with(&path, XlsxReader::Fast);
            if let Ok(opened) = open_xlsx(&path) {
                opened.workbook.sheets();
                let saved = folder.path().join("saved.xlsx");
                match save_xlsx_atomic(&opened.workbook, &saved) {
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
