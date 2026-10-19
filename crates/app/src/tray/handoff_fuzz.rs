// Structured fuzzing of the message a second launch sends: whatever arrives on the pipe, the
// decoder does not crash and an accepted path is a plain local drive path to a file the app opens.
use std::path::{Component, Path, Prefix};

use proptest::prelude::*;

use super::handoff::Handoff;
use crate::documents::session_fuzz::json_mutation;
use crate::files::is_openable;

fn baseline() -> serde_json::Value {
    let units: Vec<u16> = "C:\\Datos\\planilla.xlsx".encode_utf16().collect();
    serde_json::json!({ "open": units })
}

fn check_accepted(path: &Path) {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        panic!("{path:?} has no drive or share");
    };
    match prefix.kind() {
        Prefix::Disk(_) => {}
        Prefix::UNC(server, share) => {
            assert!(!server.is_empty() && !share.is_empty(), "{path:?}");
            assert!(!server.to_string_lossy().contains(':'), "{path:?}");
            assert!(!share.to_string_lossy().contains(':'), "{path:?}");
        }
        other => panic!("{path:?} has the prefix {other:?}"),
    }
    assert_eq!(components.next(), Some(Component::RootDir));
    for component in components {
        let Component::Normal(name) = component else {
            panic!("{path:?} has a component that is not a plain name");
        };
        assert!(!name.to_string_lossy().contains(':'), "{path:?}");
    }
    assert!(is_openable(path), "{path:?}");
}

fn check_message(bytes: &[u8]) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    if let Some(Ok(path)) = Handoff::decode(text).and_then(|handoff| handoff.requested()) {
        check_accepted(&path);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn damaged_messages_never_crash_and_only_plain_drive_paths_pass(
        bytes in json_mutation::mutated(baseline())
    ) {
        check_message(&bytes);
    }

    #[test]
    fn arbitrary_bytes_never_crash_the_decoder(
        bytes in prop::collection::vec(any::<u8>(), 0..300)
    ) {
        check_message(&bytes);
    }

    #[test]
    fn arbitrary_code_units_are_refused_or_plain_drive_paths(
        units in prop::collection::vec(any::<u16>(), 0..80)
    ) {
        let message = serde_json::json!({ "open": units }).to_string();
        check_message(message.as_bytes());
    }

    #[test]
    fn drive_and_share_shaped_paths_with_odd_segments_stay_inside_the_rules(
        segments in prop::collection::vec("[a-zA-Z0-9 .:*?<>|\\\\/_-]{0,12}", 0..5),
        extension in prop::sample::select(vec!["xlsx", "csv", "exe", "xlsx:x", ""]),
        root in prop::sample::select(vec![r"C:\", r"\\srv\share\", r"\\srv\", r"\\?\C:\"]),
    ) {
        let text = format!("{root}{}.{extension}", segments.join("\\"));
        let units: Vec<u16> = text.encode_utf16().collect();
        let message = serde_json::json!({ "open": units }).to_string();
        check_message(message.as_bytes());
    }
}
