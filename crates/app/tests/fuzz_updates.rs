// Structured fuzzing of what the update check reads from outside: the GitHub release list and
// updates.json. Damaged input is refused or harmless, never a panic or a hang.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the parsers are included whole; the test uses only part of them"
)]

#[path = "../../../test-support/json_mutation.rs"]
mod json_mutation;
#[path = "../src/updates/record.rs"]
mod record;
#[path = "../src/updates/version.rs"]
mod version;

mod recovery {
    pub fn session_directory() -> Option<std::path::PathBuf> {
        None
    }
}

use proptest::prelude::*;
use record::Record;
use version::{Version, newest_stable};

fn releases() -> serde_json::Value {
    serde_json::json!([
        { "tag_name": "v0.2.0", "draft": false, "prerelease": false, "name": "Zenkai 0.2.0",
          "body": "notes", "assets": [{ "name": "zenkai.zip", "size": 10 }] },
        { "tag_name": "v0.3.0-rc1", "draft": false, "prerelease": true },
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

fn check_version(version: Version) {
    assert_eq!(Version::parse_bare(&version.to_string()), Some(version));
}

#[test]
fn the_baseline_release_list_gives_its_newest_stable_version() {
    let body = releases().to_string();
    let newest = newest_stable(body.as_bytes()).unwrap().unwrap();
    assert_eq!(newest.to_string(), "0.2.0");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn a_damaged_release_list_is_refused_or_gives_a_real_version(bytes in json_mutation::mutated(releases())) {
        if let Ok(Some(version)) = newest_stable(&bytes) {
            check_version(version);
        }
    }

    #[test]
    fn any_bytes_are_a_release_list_or_an_error(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        if let Ok(Some(version)) = newest_stable(&bytes) {
            check_version(version);
        }
    }

    #[test]
    fn a_tag_that_parses_prints_back_to_itself(tag in prop::collection::vec(any::<char>(), 0..40).prop_map(String::from_iter)) {
        if let Some(version) = Version::parse_tag(&tag) {
            prop_assert_eq!(format!("v{version}"), tag);
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
