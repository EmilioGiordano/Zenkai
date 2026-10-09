use std::path::{Path, PathBuf};

use zenkai_types::WorkbookId;

use super::{Documents, Intent};
use crate::document::Document;
use crate::entry::Slot;
use crate::session::{self, Session, SpaceRecord};
use crate::spaces::Spaces;

impl Documents {
    // The startup blank joins the first space and stays on screen until the active workbook
    // has loaded.
    pub fn restore(
        &mut self,
        session: &Session,
        recovery_directory: Option<&Path>,
    ) -> Option<WorkbookId> {
        let (first, others) = session.spaces.split_first()?;
        self.spaces = Spaces::new();
        let first_space = self.spaces.first();
        self.spaces.rename(first_space, &first.name);
        let mut groups = vec![(first_space, first)];
        groups.extend(
            others
                .iter()
                .map(|record| (self.spaces.add(&record.name), record)),
        );
        let mut active = None;
        for (space, record) in groups {
            self.spaces.expand(space, !record.collapsed);
            self.spaces.set_color(space, record.color);
            for file in &record.files {
                let id = self.take_id();
                self.next_untitled = self.next_untitled.max(file.untitled + 1);
                let link = session::link_of(file, id, space, recovery_directory);
                self.after.push(Slot::Link(link));
                if file.active {
                    active = Some(id);
                }
            }
        }
        self.active.space = first_space;
        if let Some(id) = active {
            self.want(id, Intent::Restore);
        }
        active
    }

    // Loaded workbooks that hold unsaved work are listed with the recovery copy that keeps it,
    // when there is one.
    pub fn snapshot(
        &self,
        sidebar_visible: bool,
        recovery_file: impl Fn(WorkbookId) -> Option<PathBuf>,
    ) -> Session {
        let active = self.wanted().unwrap_or_else(|| self.active_id());
        let spaces = self
            .spaces
            .iter()
            .map(|space| SpaceRecord {
                name: space.name.clone(),
                collapsed: space.collapsed,
                color: space.color,
                files: self
                    .members(space.id)
                    .filter(|entry| !entry.loaded().is_some_and(Document::is_pristine))
                    .map(|entry| {
                        let mut link = entry.to_link();
                        if entry.loaded().is_some_and(Document::needs_recovery) {
                            link.recovery = recovery_file(link.id);
                        }
                        session::record_of(&link, link.id == active)
                    })
                    .collect(),
            })
            .collect();
        Session::new(sidebar_visible, spaces)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::super::{Installed, Intent};
    use super::*;
    use crate::entry::{Entry, LinkStatus};
    use crate::session::{FileRecord, ViewRecord};
    use crate::spaces::SpaceId;
    use zenkai_engine::Unsupported;
    use zenkai_types::SheetId;

    fn member_names(documents: &Documents, space: SpaceId) -> Vec<String> {
        documents.members(space).map(Entry::name).collect()
    }

    fn file(path: Option<&str>, active: bool) -> FileRecord {
        FileRecord {
            path: path.map(str::to_string),
            recovery: None,
            untitled: if path.is_none() { 3 } else { 0 },
            dirty: false,
            read_only: false,
            unsupported: Vec::new(),
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

    fn session() -> Session {
        Session::new(
            true,
            vec![
                SpaceRecord {
                    name: "Q3".to_string(),
                    collapsed: false,
                    color: Default::default(),
                    files: vec![file(Some("a.xlsx"), false), file(Some("gone.xlsx"), true)],
                },
                SpaceRecord {
                    name: "Suppliers".to_string(),
                    collapsed: true,
                    color: Default::default(),
                    files: vec![file(None, false)],
                },
            ],
        )
    }

    #[test]
    fn restoring_lists_every_file_as_a_link_in_its_space_and_returns_the_active_one() {
        let mut documents = documents();
        let active = documents.restore(&session(), None).unwrap();
        assert_eq!(documents.len(), 4);
        let spaces: Vec<_> = documents.spaces().iter().cloned().collect();
        assert_eq!(spaces.len(), 2);
        assert_eq!(spaces[0].name, "Q3");
        assert!(spaces[1].collapsed);
        assert_eq!(
            member_names(&documents, spaces[0].id),
            ["Book1", "a.xlsx", "gone.xlsx"]
        );
        assert_eq!(member_names(&documents, spaces[1].id), ["Book3"]);
        assert_eq!(documents.entry(active).unwrap().name(), "gone.xlsx");
        assert!(documents.entry(active).unwrap().loaded().is_none());
        assert_eq!(documents.active().name(), "Book1");
    }

    #[test]
    fn untitled_numbers_continue_after_the_restored_ones() {
        let mut documents = documents();
        documents.restore(&session(), None);
        documents.active_mut().dirty = true;
        documents.create(blank());
        assert_eq!(documents.active().name(), "Book4");
    }

    #[test]
    fn an_empty_session_changes_nothing() {
        let mut documents = documents();
        let empty = Session::new(false, Vec::new());
        assert_eq!(documents.restore(&empty, None), None);
        assert_eq!(documents.len(), 1);
    }

    #[test]
    fn the_snapshot_keeps_links_that_never_loaded_and_the_missing_ones() {
        let mut documents = documents();
        documents.restore(&session(), None);
        let missing = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        documents.set_link_status(missing, LinkStatus::Missing);
        let snapshot = documents.snapshot(true, |_| None);
        let files: Vec<_> = snapshot
            .spaces
            .iter()
            .flat_map(|space| space.files.iter().map(|file| file.path.clone()))
            .collect();
        assert_eq!(
            files,
            [
                Some("a.xlsx".to_string()),
                Some("gone.xlsx".to_string()),
                None
            ]
        );
        assert!(snapshot.spaces[0].files[1].active);
    }

    #[test]
    fn the_snapshot_points_unsaved_work_at_its_recovery_copy() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let snapshot = documents.snapshot(false, |id| {
            Some(PathBuf::from(format!("recovery/{}.xlsx", id.0)))
        });
        let record = &snapshot.spaces[0].files[0];
        assert_eq!(record.recovery.as_deref(), Some("0.xlsx"));
        assert!(record.dirty);
    }

    #[test]
    fn unsaved_work_without_a_recovery_copy_is_listed_without_one() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let snapshot = documents.snapshot(false, |_| None);
        assert_eq!(snapshot.spaces[0].files[0].recovery, None);
    }

    #[test]
    fn a_restored_active_workbook_replaces_the_untouched_blank_when_it_loads() {
        let mut documents = documents();
        let active = documents.restore(&session(), None).unwrap();
        assert!(matches!(
            documents.install(loaded(active)),
            Installed::OnScreen
        ));
        assert_eq!(documents.active_id(), active);
        assert_eq!(documents.len(), 3);
        assert_eq!(documents.active().sheet, SheetId(0));
        assert_eq!(documents.active().view.selection.active.row.get(), 9);
        assert_eq!(names(&documents), ["a.xlsx", "gone.xlsx", "Book3"]);
    }

    #[test]
    fn a_workbook_loaded_while_the_user_works_elsewhere_stays_in_the_background() {
        let mut documents = documents();
        let active = documents.restore(&session(), None).unwrap();
        documents.active_mut().dirty = true;
        assert!(matches!(
            documents.install(loaded(active)),
            Installed::Background
        ));
        assert_eq!(documents.active().name(), "Book1");
        assert!(documents.get(active).is_some());
    }

    #[test]
    fn a_switch_made_during_a_load_overrides_the_restored_active_workbook() {
        let mut documents = documents();
        let restored = documents.restore(&session(), None).unwrap();
        let other = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        documents.want(other, Intent::Switch);
        assert!(matches!(
            documents.install(loaded(restored)),
            Installed::Background
        ));
        assert!(matches!(
            documents.install(loaded(other)),
            Installed::OnScreen
        ));
        assert_eq!(documents.active_id(), other);
    }

    #[test]
    fn stepping_starts_from_the_workbook_being_waited_for() {
        let mut documents = busy();
        documents.restore(&session(), None);
        let a = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        let gone = documents.find_by_path(Path::new("gone.xlsx")).unwrap();
        documents.want(a, Intent::Switch);
        assert_eq!(documents.step(super::super::Step::Next), gone);
    }

    #[test]
    fn closing_the_active_workbook_never_lands_on_a_link() {
        let mut documents = busy();
        let first = documents.active_id();
        documents.restore(&session(), None);
        let a = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        documents.close(first, blank);
        assert!(documents.entry(a).unwrap().loaded().is_none());
        assert_eq!(documents.active().name(), "Book4");
    }

    #[test]
    fn a_dirty_restored_workbook_keeps_its_save_guard() {
        let mut record = file(Some("a.xlsx"), true);
        record.dirty = true;
        record.unsupported = vec!["charts".to_string()];
        let session = Session::new(
            false,
            vec![SpaceRecord {
                name: "Q3".to_string(),
                collapsed: false,
                color: Default::default(),
                files: vec![record],
            }],
        );
        let mut documents = documents();
        let id = documents
            .restore(&session, Some(Path::new("recovery")))
            .unwrap();
        let mut recovered = loaded(id);
        recovered.from_recovery = true;
        documents.install(recovered);
        let document = documents.active();
        assert!(document.dirty);
        assert_eq!(document.unsupported, [Unsupported::Charts]);
        assert_eq!(document.path.as_deref(), Some(Path::new("a.xlsx")));
    }

    #[test]
    fn closing_a_link_remembers_its_path_for_reopening() {
        let mut documents = documents();
        documents.restore(&session(), None);
        let a = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        documents.close(a, blank);
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.xlsx")));
    }
}
