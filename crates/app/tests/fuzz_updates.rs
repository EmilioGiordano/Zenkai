// Structured fuzzing of what the update check reads from outside: the GitHub release list,
// updates.json and the release notes cache. Damaged input is refused or harmless, never a
// panic or a hang, and whatever is accepted stays inside the limits the What's new tab relies on.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the parsers are included whole; the test uses only part of them"
)]

#[path = "../../../test-support/json_mutation.rs"]
mod json_mutation;
#[path = "../src/updates/notes.rs"]
mod notes;
#[path = "../src/updates/record.rs"]
mod record;
#[path = "../src/updates/releases.rs"]
mod releases;
#[path = "../src/updates/store.rs"]
mod store;
#[path = "../src/updates/version.rs"]
mod version;

mod recovery {
    pub fn session_directory() -> Option<std::path::PathBuf> {
        None
    }
}

use notes::{MAX_NOTE_BYTES, MAX_NOTES, Notes};
use proptest::prelude::*;
use record::Record;
use version::Version;

fn release_list() -> serde_json::Value {
    serde_json::json!([
        { "tag_name": "v0.2.0", "draft": false, "prerelease": false, "name": "Zenkai 0.2.0",
          "published_at": "2026-12-04T10:00:00Z", "body": "## Added\n\n- notes",
          "assets": [{ "name": "zenkai.zip", "size": 10 }] },
        { "tag_name": "v0.3.0-rc1", "draft": false, "prerelease": true },
        { "tag_name": "v0.1.1", "draft": false, "prerelease": false,
          "published_at": "2026-11-20T10:00:00Z", "body": "fixed" },
        { "tag_name": "v0.1.0", "draft": false, "prerelease": false },
        { "tag_name": "v9.0.0", "draft": true, "prerelease": false }
    ])
}

fn stored_record() -> serde_json::Value {
    serde_json::json!({
        "last_check": 1_700_000_000u64,
        "last_success": 1_699_999_000u64,
        "newest": "0.2.0",
        "announced": "0.1.1"
    })
}

fn stored_notes() -> serde_json::Value {
    serde_json::json!({
        "notes": [
            { "version": "0.2.0", "published": "2026-12-04", "body": "## Added\n\n- notes" },
            { "version": "0.1.1", "published": null, "body": null },
            { "version": "0.1.0" }
        ]
    })
}

fn check_version(version: Version) {
    assert_eq!(Version::parse_bare(&version.to_string()), Some(version));
}

fn installed() -> Version {
    Version::parse_bare("0.1.0").unwrap()
}

fn newest(bytes: &[u8]) -> Option<Version> {
    releases::parse(bytes, installed())
        .ok()
        .and_then(|found| found.newest)
}

fn check_notes(notes: &Notes) {
    let everything: Vec<_> = notes
        .newer_than(Version::parse_bare("0.0.0").unwrap())
        .collect();
    assert!(everything.len() <= MAX_NOTES);
    for pair in everything.windows(2) {
        assert!(pair[0].version > pair[1].version);
    }
    for note in everything {
        check_version(note.version);
        assert!(
            note.body
                .as_ref()
                .is_none_or(|body| body.len() <= MAX_NOTE_BYTES)
        );
        if let Some(day) = note.published {
            assert_eq!(notes::Day::parse(&day.to_string()), Some(day));
        }
    }
}

#[test]
fn the_baseline_release_list_gives_its_newest_stable_version_and_its_notes() {
    let body = release_list().to_string();
    let found = releases::parse(body.as_bytes(), installed()).unwrap();
    assert_eq!(found.newest.unwrap().to_string(), "0.2.0");
    let versions: Vec<String> = found
        .notes
        .newer_than(installed())
        .map(|note| note.version.to_string())
        .collect();
    assert_eq!(versions, ["0.2.0", "0.1.1"]);
}

#[test]
fn the_baseline_notes_cache_is_accepted() {
    let notes = Notes::parse(stored_notes().to_string().as_bytes()).unwrap();
    check_notes(&notes);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn a_damaged_release_list_is_refused_or_gives_real_versions(bytes in json_mutation::mutated(release_list())) {
        if let Ok(found) = releases::parse(&bytes, installed()) {
            if let Some(version) = found.newest {
                check_version(version);
            }
            check_notes(&found.notes);
        }
    }

    #[test]
    fn any_bytes_are_a_release_list_or_an_error(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        if let Some(version) = newest(&bytes) {
            check_version(version);
        }
    }

    #[test]
    fn a_damaged_notes_cache_is_refused_or_within_the_limits(bytes in json_mutation::mutated(stored_notes())) {
        if let Ok(notes) = Notes::parse(&bytes) {
            check_notes(&notes);
            let again = Notes::parse(notes.to_json().unwrap().as_bytes()).unwrap();
            prop_assert_eq!(again, notes);
        }
    }

    #[test]
    fn any_bytes_are_a_notes_cache_or_an_error(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        if let Ok(notes) = Notes::parse(&bytes) {
            check_notes(&notes);
        }
    }

    #[test]
    fn a_tag_that_parses_prints_back_to_itself(tag in prop::collection::vec(any::<char>(), 0..40).prop_map(String::from_iter)) {
        if let Some(version) = Version::parse_tag(&tag) {
            prop_assert_eq!(format!("v{version}"), tag);
        }
    }

    #[test]
    fn a_text_that_parses_as_a_date_prints_back_as_its_start(text in prop::collection::vec(any::<char>(), 0..24).prop_map(String::from_iter)) {
        if let Some(day) = notes::Day::parse(&text) {
            prop_assert!(text.starts_with(&day.to_string()));
        }
    }

    #[test]
    fn a_damaged_record_is_refused_or_usable(bytes in json_mutation::mutated(stored_record()), now in any::<u64>()) {
        if let Ok(record) = Record::parse(&bytes) {
            record.is_due(now);
            if let Some(newest) = record.newest {
                check_version(newest);
                record.should_announce(newest);
            }
        }
    }

    #[test]
    fn any_bytes_are_a_record_or_an_error(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let _ = Record::parse(&bytes);
    }
}
