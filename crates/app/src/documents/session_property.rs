// Spec: whatever the user does with workbooks, quitting and restarting brings back every
// unsaved change, and every workbook that has a file or work stays in the session.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use zenkai_agent::protected_view::FileOrigin;
use zenkai_types::WorkbookId;

use super::test_support::{blank, documents};
use super::{Documents, Loaded};
use crate::entry::Entry;
use crate::session::{self, Loaded as LoadedSession};
use crate::space_appearance::SpaceAppearance;

#[derive(Clone, Debug)]
enum Op {
    Open(u8),
    Create,
    Edit(usize),
    Save(usize),
    Close(usize),
    Unload(usize),
    Load(usize),
    Quit,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => (0u8..6).prop_map(Op::Open),
        2 => Just(Op::Create),
        4 => any::<usize>().prop_map(Op::Edit),
        2 => any::<usize>().prop_map(Op::Save),
        2 => any::<usize>().prop_map(Op::Close),
        2 => any::<usize>().prop_map(Op::Unload),
        2 => any::<usize>().prop_map(Op::Load),
        3 => Just(Op::Quit),
    ]
}

struct Model {
    documents: Documents,
    recovery_dir: PathBuf,
    session_dir: PathBuf,
    // Written by the user and not saved: what a restart must bring back.
    unsaved: BTreeSet<WorkbookId>,
    // The recovery copy name each unsaved workbook is given, stable across restarts.
    copies: BTreeMap<WorkbookId, String>,
    next_copy: u32,
    next_save: u32,
}

impl Model {
    fn new(root: &Path) -> Model {
        Model {
            documents: documents(),
            recovery_dir: root.join("recovery"),
            session_dir: root.to_path_buf(),
            unsaved: BTreeSet::new(),
            copies: BTreeMap::new(),
            next_copy: 0,
            next_save: 0,
        }
    }

    fn pick(&self, index: usize) -> Option<WorkbookId> {
        let count = self.documents.len();
        if count == 0 {
            return None;
        }
        self.documents.entries().nth(index % count).map(Entry::id)
    }

    fn load(&mut self, id: WorkbookId) {
        let Some(Entry::Link(link)) = self.documents.entry(id) else {
            return;
        };
        let from_recovery = link.recovery.is_some();
        self.documents.start_loading(id);
        self.documents.install(Loaded {
            id,
            workbook: blank(),
            unsupported: Vec::new(),
            read_only: false,
            origin: FileOrigin::Local,
            from_recovery,
        });
    }

    fn copy_name(&mut self, id: WorkbookId) -> String {
        if let Some(name) = self.copies.get(&id) {
            return name.clone();
        }
        self.next_copy += 1;
        let name = format!("autosave-1-2-{}.xlsx", self.next_copy);
        self.copies.insert(id, name.clone());
        name
    }

    fn apply(&mut self, op: &Op) {
        match op {
            Op::Open(n) => {
                let path = PathBuf::from(format!("C:/work/file{n}.xlsx"));
                if !self.documents.has_path(&path) {
                    self.documents.open(blank(), Some(path), Vec::new());
                }
            }
            Op::Create => {
                self.documents.create(blank());
            }
            Op::Edit(index) => {
                let Some(id) = self.pick(*index) else { return };
                self.load(id);
                self.documents.get_mut(id).unwrap().dirty = true;
                self.unsaved.insert(id);
            }
            Op::Save(index) => {
                let Some(id) = self.pick(*index) else { return };
                self.load(id);
                self.next_save += 1;
                let saved_as = PathBuf::from(format!("C:/work/saved{}.xlsx", self.next_save));
                let document = self.documents.get_mut(id).unwrap();
                document.dirty = false;
                if document.path.is_none() {
                    document.path = Some(saved_as);
                }
                self.unsaved.remove(&id);
            }
            Op::Close(index) => {
                let Some(id) = self.pick(*index) else { return };
                self.documents.close(id);
                self.unsaved.remove(&id);
            }
            Op::Unload(index) => {
                let Some(id) = self.pick(*index) else { return };
                if self.documents.unload(id) {
                    assert!(
                        !self.unsaved.contains(&id),
                        "a workbook with unsaved work was unloaded"
                    );
                }
            }
            Op::Load(index) => {
                if let Some(id) = self.pick(*index) {
                    self.load(id);
                }
            }
            Op::Quit => self.quit_and_restart(),
        }
    }

    fn quit_and_restart(&mut self) {
        let paths_before: BTreeSet<String> = self
            .documents
            .entries()
            .filter_map(|entry| entry.path().map(|path| path.to_string_lossy().into_owned()))
            .collect();
        let ids: Vec<WorkbookId> = self.unsaved.iter().copied().collect();
        let names: BTreeMap<WorkbookId, String> =
            ids.into_iter().map(|id| (id, self.copy_name(id))).collect();
        let recovery_dir = self.recovery_dir.clone();
        let session = self
            .documents
            .snapshot(true, SpaceAppearance::default(), |id| {
                names.get(&id).map(|name| recovery_dir.join(name))
            });

        let records: Vec<_> = session.spaces.iter().flat_map(|s| &s.files).collect();
        for (id, name) in &names {
            let record = records
                .iter()
                .find(|record| record.recovery.as_deref() == Some(name.as_str()))
                .unwrap_or_else(|| panic!("workbook {id:?} lost its recovery copy {name}"));
            assert!(record.dirty, "{name} would not be offered as unsaved work");
        }
        let paths_after: BTreeSet<String> = records
            .iter()
            .filter_map(|record| record.path.clone())
            .collect();
        assert_eq!(paths_after, paths_before, "a workbook left the session");

        session::save(&self.session_dir, &session).unwrap();
        let LoadedSession::Restored(restored) = session::load(&self.session_dir) else {
            panic!("the session written at quit could not be read back");
        };
        assert_eq!(restored, session);

        let mut fresh = documents();
        fresh.restore(&restored, Some(&self.recovery_dir));
        let mut carried = BTreeSet::new();
        let mut copies = BTreeMap::new();
        for entry in fresh.entries() {
            let Entry::Link(link) = entry else { continue };
            let Some(file) = &link.recovery else { continue };
            assert!(link.dirty, "a recovery copy without the unsaved flag");
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            carried.insert(link.id);
            copies.insert(link.id, name);
        }
        let expected: BTreeSet<&String> = names.values().collect();
        let found: BTreeSet<&String> = copies.values().collect();
        assert_eq!(
            found, expected,
            "the restart did not bring back the unsaved work"
        );

        self.documents = fresh;
        self.unsaved = carried;
        self.copies = copies;
    }

    fn check(&self) {
        for id in &self.unsaved {
            match self.documents.entry(*id) {
                Some(Entry::Loaded(document)) => assert!(
                    document.needs_recovery(),
                    "unsaved work is not tracked for recovery"
                ),
                Some(Entry::Link(link)) => assert!(
                    link.dirty && link.recovery.is_some(),
                    "an unloaded workbook lost its unsaved work"
                ),
                None => panic!("a workbook with unsaved work vanished"),
            }
        }
        if let Some(id) = self.documents.active_id() {
            assert!(
                matches!(self.documents.entry(id), Some(Entry::Loaded(_))),
                "the screen shows a link"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 150, ..ProptestConfig::default() })]

    #[test]
    fn restart_always_brings_back_unsaved_work(ops in prop::collection::vec(op(), 1..40)) {
        let root = tempfile::tempdir().unwrap();
        let mut model = Model::new(root.path());
        for op in &ops {
            model.apply(op);
            model.check();
        }
        model.quit_and_restart();
        model.check();
    }
}
