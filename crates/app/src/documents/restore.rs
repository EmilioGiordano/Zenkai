use std::path::{Path, PathBuf};

use zenkai_types::WorkbookId;

use super::{Documents, Intent};
use crate::document::Document;
use crate::entry::{Entry, Slot};
use crate::session::{self, Session, SpaceRecord};
use crate::space_appearance::SpaceAppearance;
use crate::spaces::Spaces;

fn has_recovery_copy_to_keep(entry: Entry<'_>) -> bool {
    entry
        .loaded()
        .is_some_and(|document| document.needs_recovery() && document.has_recovery_copy())
        || entry
            .text()
            .is_some_and(|text| text.needs_recovery() && text.has_recovery_copy())
}

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
            self.spaces.set_appearance(space, record.appearance);
            for file in &record.files {
                let id = self.take_id();
                let link = session::link_of(file, id, space, recovery_directory);
                self.after.push(Slot::Link(link));
                if file.active {
                    active = Some(id);
                }
            }
        }
        self.empty_space = first_space;
        if let Some(active) = self.active.as_deref_mut() {
            active.space = first_space;
        }
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
        space_appearance: SpaceAppearance,
        recovery_file: impl Fn(WorkbookId) -> Option<PathBuf>,
    ) -> Session {
        self.snapshot_leaving(sidebar_visible, space_appearance, recovery_file, &[])
    }

    // A discarded workbook is recorded as it was last saved, so the tab still comes back next
    // time without the work the user chose to lose; one never saved has nothing to come back to.
    pub fn snapshot_leaving(
        &self,
        sidebar_visible: bool,
        space_appearance: SpaceAppearance,
        recovery_file: impl Fn(WorkbookId) -> Option<PathBuf>,
        discarded: &[WorkbookId],
    ) -> Session {
        let active = self.wanted().or_else(|| self.shown_id());
        let spaces = self
            .spaces
            .iter()
            .map(|space| SpaceRecord {
                name: space.name.clone(),
                collapsed: space.collapsed,
                color: space.color,
                appearance: space.appearance,
                files: self
                    .members(space.id)
                    .filter(|entry| !entry.loaded().is_some_and(Document::is_pristine))
                    .filter(|entry| entry.path().is_some() || !discarded.contains(&entry.id()))
                    .map(|entry| {
                        let mut link = entry.to_link();
                        if discarded.contains(&link.id) {
                            link.dirty = false;
                            link.recovery = None;
                        } else if has_recovery_copy_to_keep(entry) {
                            link.recovery = recovery_file(link.id);
                        }
                        session::record_of(&link, Some(link.id) == active)
                    })
                    .collect(),
            })
            .collect();
        let reopened = discarded
            .iter()
            .filter(|id| self.entry(**id).is_some_and(|entry| entry.path().is_some()))
            .count();
        Session::new(sidebar_visible, spaces)
            .with_space_appearance(space_appearance)
            .with_reopened_as_saved(u32::try_from(reopened).unwrap_or(u32::MAX))
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
            kind: Default::default(),
            mode: Default::default(),
            base: None,
            pdf: Default::default(),
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
                    appearance: Default::default(),
                    files: vec![file(Some("a.xlsx"), false), file(Some("gone.xlsx"), true)],
                },
                SpaceRecord {
                    name: "Suppliers".to_string(),
                    collapsed: true,
                    color: Default::default(),
                    appearance: Default::default(),
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
        assert_eq!(documents.active().unwrap().name(), "Book1");
    }

    #[test]
    fn untitled_numbers_fill_the_gaps_left_by_the_restored_ones() {
        let mut documents = documents();
        documents.restore(&session(), None);
        documents.active_mut().unwrap().dirty = true;
        documents.create(blank());
        assert_eq!(documents.active().unwrap().name(), "Book2");
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
        let snapshot = documents.snapshot(true, SpaceAppearance::default(), |_| None);
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
    fn restoring_and_snapshotting_keep_the_color_and_the_appearance_of_each_space() {
        use crate::space_appearance::{Custom, Look, SpaceOverride, SpaceStyle};
        use crate::spaces::SpaceColor;
        let custom = SpaceOverride::Custom(Custom {
            look: Look {
                style: SpaceStyle::Border,
                ..Look::default()
            },
            color: None,
        });
        let mut stored = session();
        stored.spaces[1].color = SpaceColor::Violet;
        stored.spaces[1].appearance = custom;
        let mut documents = documents();
        documents.restore(&stored, None);
        let snapshot = documents.snapshot(true, SpaceAppearance::default(), |_| None);
        assert_eq!(snapshot.spaces[1].color, SpaceColor::Violet);
        assert_eq!(snapshot.spaces[1].appearance, custom);
        assert_eq!(snapshot.spaces[0].appearance, SpaceOverride::Default);
    }

    #[test]
    fn the_snapshot_points_unsaved_work_at_its_recovery_copy() {
        let mut documents = documents();
        documents.active_mut().unwrap().dirty = true;
        documents.active_mut().unwrap().mark_recovery_adopted();
        let snapshot = documents.snapshot(false, SpaceAppearance::default(), |id| {
            Some(PathBuf::from(format!("recovery/{}.xlsx", id.0)))
        });
        let record = &snapshot.spaces[0].files[0];
        assert_eq!(record.recovery.as_deref(), Some("0.xlsx"));
        assert!(record.dirty);
    }

    #[test]
    fn unsaved_work_is_not_pointed_at_a_copy_that_was_never_written() {
        let mut documents = documents();
        documents.active_mut().unwrap().dirty = true;
        let snapshot = documents.snapshot(false, SpaceAppearance::default(), |id| {
            Some(PathBuf::from(format!("recovery/{}.xlsx", id.0)))
        });
        let record = &snapshot.spaces[0].files[0];
        assert_eq!(record.recovery, None);
        assert!(record.dirty);
    }

    #[test]
    fn unsaved_work_without_a_recovery_copy_is_listed_without_one() {
        let mut documents = documents();
        documents.active_mut().unwrap().dirty = true;
        let snapshot = documents.snapshot(false, SpaceAppearance::default(), |_| None);
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
        assert_eq!(documents.active_id().unwrap(), active);
        assert_eq!(documents.len(), 3);
        assert_eq!(documents.active().unwrap().sheet, SheetId(0));
        assert_eq!(
            documents.active().unwrap().view.selection.active.row.get(),
            9
        );
        assert_eq!(names(&documents), ["a.xlsx", "gone.xlsx", "Book3"]);
    }

    #[test]
    fn a_workbook_loaded_while_the_user_works_elsewhere_stays_in_the_background() {
        let mut documents = documents();
        let active = documents.restore(&session(), None).unwrap();
        documents.active_mut().unwrap().dirty = true;
        assert!(matches!(
            documents.install(loaded(active)),
            Installed::Background
        ));
        assert_eq!(documents.active().unwrap().name(), "Book1");
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
        assert_eq!(documents.active_id().unwrap(), other);
    }

    #[test]
    fn stepping_starts_from_the_workbook_being_waited_for() {
        let mut documents = busy();
        documents.restore(&session(), None);
        let a = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        let gone = documents.find_by_path(Path::new("gone.xlsx")).unwrap();
        documents.want(a, Intent::Switch);
        assert_eq!(documents.step(super::super::Step::Next).unwrap(), gone);
    }

    #[test]
    fn closing_the_active_workbook_leaves_the_screen_empty_instead_of_landing_on_a_link() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        documents.restore(&session(), None);
        let a = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        documents.close(first);
        assert!(documents.entry(a).unwrap().loaded().is_none());
        assert!(documents.active().is_none());
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
                appearance: Default::default(),
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
        let document = documents.active().unwrap();
        assert!(document.dirty);
        assert_eq!(document.unsupported, [Unsupported::Charts]);
        assert!(document.may_lose_unseen_content());
        assert_eq!(document.path.as_deref(), Some(Path::new("a.xlsx")));
    }

    #[test]
    fn closing_a_link_remembers_its_path_for_reopening() {
        let mut documents = documents();
        documents.restore(&session(), None);
        let a = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        documents.close(a);
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.xlsx")));
    }

    fn snapshot_files(documents: &Documents, discarded: &[WorkbookId]) -> Vec<FileRecord> {
        documents
            .snapshot_leaving(
                true,
                SpaceAppearance::default(),
                |_| Some(PathBuf::from("autosave-1-2-3.xlsx")),
                discarded,
            )
            .spaces
            .into_iter()
            .flat_map(|space| space.files)
            .collect()
    }

    #[test]
    fn a_discarded_workbook_is_recorded_as_last_saved_and_the_others_keep_their_work() {
        let mut documents = documents();
        let saved = open_file(&mut documents, "saved.xlsx");
        let other = open_file(&mut documents, "other.xlsx");
        for id in [saved, other] {
            let document = documents.get_mut(id).unwrap();
            document.dirty = true;
            document.mark_recovery_adopted();
        }

        let kept = snapshot_files(&documents, &[]);
        assert!(
            kept.iter()
                .all(|file| file.dirty && file.recovery.is_some())
        );

        let files = snapshot_files(&documents, &[saved]);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path.as_deref(), Some("saved.xlsx"));
        assert!(!files[0].dirty);
        assert_eq!(files[0].recovery, None);
        assert!(files[1].dirty);
        assert!(files[1].recovery.is_some());
    }

    #[test]
    fn a_discarded_workbook_that_was_never_saved_has_nothing_to_come_back_to() {
        let mut documents = busy();
        let blank = documents.active_id().unwrap();
        open_file(&mut documents, "saved.xlsx");
        assert_eq!(snapshot_files(&documents, &[]).len(), 2);
        let files = snapshot_files(&documents, &[blank]);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_deref(), Some("saved.xlsx"));
    }

    #[test]
    fn a_discarded_restored_link_drops_the_pointer_to_its_recovery_copy() {
        let mut record = file(Some("a.xlsx"), false);
        record.dirty = true;
        record.recovery = Some("autosave-9-9-9.xlsx".to_string());
        let session = Session::new(
            true,
            vec![SpaceRecord {
                name: "Q3".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![record],
            }],
        );
        let mut documents = documents();
        documents.restore(&session, Some(Path::new("recovery")));
        let link = documents.find_by_path(Path::new("a.xlsx")).unwrap();
        let files = snapshot_files(&documents, &[link]);
        assert_eq!(files.len(), 1);
        assert!(!files[0].dirty);
        assert_eq!(files[0].recovery, None);
        assert!(snapshot_files(&documents, &[])[0].recovery.is_some());
    }

    fn reopened_after(documents: &Documents, discarded: &[WorkbookId]) -> u32 {
        documents
            .snapshot_leaving(true, SpaceAppearance::default(), |_| None, discarded)
            .reopened_as_saved
    }

    #[test]
    fn the_reopened_notice_counts_only_discarded_workbooks_that_come_back() {
        let mut documents = busy();
        let blank = documents.active_id().unwrap();
        let saved = open_file(&mut documents, "saved.xlsx");
        documents.get_mut(saved).unwrap().dirty = true;
        assert_eq!(reopened_after(&documents, &[]), 0);
        assert_eq!(reopened_after(&documents, &[blank]), 0);
        assert_eq!(reopened_after(&documents, &[blank, saved]), 1);
    }

    #[test]
    fn starting_empty_or_a_crash_never_carries_the_notice() {
        let mut documents = documents();
        let saved = open_file(&mut documents, "saved.xlsx");
        let session =
            documents.snapshot_leaving(true, SpaceAppearance::default(), |_| None, &[saved]);
        assert_eq!(session.reopened_as_saved, 1);
        assert_eq!(session.without_unsaved_work().reopened_as_saved, 0);
        assert_eq!(
            documents
                .snapshot(true, SpaceAppearance::default(), |_| None)
                .reopened_as_saved,
            0
        );
    }
}
