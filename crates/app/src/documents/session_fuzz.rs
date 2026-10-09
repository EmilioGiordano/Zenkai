// Structured fuzzing of session.json, which survives crashes and can be damaged or written by
// another program: a damaged file is set aside, never crashes the start, and a session that
// is accepted cannot point the recovery machinery outside the recovery folder.
#[path = "../../../../test-support/json_mutation.rs"]
mod json_mutation;

use proptest::prelude::*;

use super::test_support::documents;
use crate::entry::Entry;
use crate::session::{self, FileRecord, Loaded, Session, SpaceRecord, ViewRecord};
use crate::space_appearance::SpaceAppearance;

fn file(path: Option<&str>, recovery: Option<&str>, active: bool) -> FileRecord {
    FileRecord {
        path: path.map(str::to_string),
        recovery: recovery.map(str::to_string),
        untitled: u32::from(path.is_none()),
        dirty: recovery.is_some(),
        read_only: false,
        unsupported: vec!["charts".to_string(), "macros".to_string()],
        active,
        sheet: 1,
        view: ViewRecord {
            active_row: 9,
            active_col: 2,
            corner_row: 9,
            corner_col: 2,
            top: 5,
            left: 0,
        },
    }
}

fn baseline() -> serde_json::Value {
    let space = |name: &str, files| SpaceRecord {
        name: name.to_string(),
        collapsed: false,
        color: Default::default(),
        appearance: Default::default(),
        files,
    };
    let session = Session::new(
        true,
        vec![
            space(
                "Q3",
                vec![
                    file(Some("C:/work/a.xlsx"), Some("autosave-1-2-3.xlsx"), true),
                    file(Some("C:/work/b.xlsx"), None, false),
                ],
            ),
            space(
                "Loose",
                vec![file(None, Some("autosave-1-2-4.xlsx"), false)],
            ),
        ],
    );
    serde_json::to_value(session).unwrap()
}

fn check_restored(session: &Session) {
    let recovery_dir = std::path::Path::new("recovery");
    let mut documents = documents();
    documents.restore(session, Some(recovery_dir));
    for entry in documents.entries() {
        let Entry::Link(link) = entry else { continue };
        if let Some(copy) = &link.recovery {
            let name = copy.file_name().unwrap().to_string_lossy();
            assert_eq!(copy.parent(), Some(recovery_dir), "{copy:?}");
            assert!(name.starts_with("autosave-") && name.ends_with(".xlsx"));
            assert!(!name.contains(':') && !name.chars().any(std::path::is_separator));
        }
    }
    documents.snapshot(true, SpaceAppearance::default(), |_| None);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn damaged_sessions_are_set_aside_or_restore_safely(bytes in json_mutation::mutated(baseline())) {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(session::file_in(folder.path()), &bytes).unwrap();
        match session::load(folder.path()) {
            Loaded::Restored(session) => check_restored(&session),
            Loaded::Unreadable => prop_assert!(
                !session::file_in(folder.path()).exists(),
                "an unreadable session was left in place to be overwritten by the next close"
            ),
            Loaded::Absent => prop_assert!(false, "a file that exists is not absent"),
        }
    }

    #[test]
    fn arbitrary_bytes_never_crash_the_session_loader(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(session::file_in(folder.path()), &bytes).unwrap();
        if let Loaded::Restored(session) = session::load(folder.path()) {
            check_restored(&session);
        }
    }
}

#[test]
fn the_baseline_session_is_restored() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(session::file_in(folder.path()), baseline().to_string()).unwrap();
    let Loaded::Restored(session) = session::load(folder.path()) else {
        panic!("the baseline session must load");
    };
    check_restored(&session);
}
