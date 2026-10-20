use std::path::{Path, PathBuf};
use std::time::Instant;

use zenkai_types::WorkbookId;

use super::{Documents, Installed};
use crate::entry::Slot;
use crate::text_document::{DocumentKind, TextDocument, ViewMode};
use crate::text_file::{Format, Stamp};

pub struct TextLoad {
    pub id: WorkbookId,
    pub format: Format,
    pub stamp: Option<Stamp>,
    // Read from a recovery copy: the text holds work the file does not.
    pub from_recovery: bool,
}

impl Documents {
    // An untouched blank workbook gives way to the text opened, as it does to a workbook.
    pub fn open_text(
        &mut self,
        path: Option<PathBuf>,
        kind: DocumentKind,
        mode: ViewMode,
        format: Format,
        stamp: Option<Stamp>,
    ) -> WorkbookId {
        let space = self.current_space();
        let id = self.take_id();
        let untitled = if path.is_none() {
            self.lowest_free_untitled(kind)
        } else {
            0
        };
        let mut text = TextDocument::new(id, space, untitled, path, kind, mode);
        text.format = format;
        text.stamp = stamp;
        let keep_old = self
            .active
            .as_ref()
            .is_some_and(|active| !active.is_pristine());
        let old = self.active.take().filter(|_| keep_old);
        self.before.extend(old.map(Slot::Loaded));
        self.before.append(&mut self.after);
        self.before.push(Slot::Text(Box::new(text)));
        self.text_shown = Some(id);
        self.wanted = None;
        id
    }

    // The text takes its link's place, space and remembered mode. It comes on screen when it is
    // what the user waits for, dropping an untouched startup blank.
    pub fn install_text(&mut self, load: TextLoad) -> Installed {
        let on_screen = self.shows_when_loaded(load.id);
        let blank = self.active.as_ref().is_some_and(|a| a.is_pristine());
        let Some(slot) = self
            .before
            .iter_mut()
            .chain(self.after.iter_mut())
            .find(|slot| slot.id() == load.id)
        else {
            return Installed::Gone;
        };
        let Slot::Link(link) = slot else {
            return Installed::Gone;
        };
        let mut text = TextDocument::new(
            link.id,
            link.space,
            link.untitled,
            link.path.clone(),
            link.kind,
            link.mode,
        );
        text.format = load.format;
        text.stamp = load.stamp;
        if load.from_recovery {
            text.edited();
        }
        *slot = Slot::Text(Box::new(text));
        if !on_screen {
            return Installed::Background;
        }
        self.show_text(load.id, !blank);
        Installed::OnScreen
    }

    // `keep_old` is false only for a blank workbook nobody touched.
    pub(super) fn show_text(&mut self, id: WorkbookId, keep_old: bool) {
        if let Some(mut old) = self.active.take()
            && keep_old
        {
            old.last_used = Instant::now();
            self.before.push(Slot::Loaded(old));
            self.before.append(&mut self.after);
        }
        self.text_shown = Some(id);
        self.wanted = None;
    }

    // Whether the tab or link at this path is a text document, loaded or not.
    pub fn is_text_at(&self, path: &Path) -> bool {
        self.find_by_path(path)
            .and_then(|id| self.entry(id))
            .is_some_and(|entry| entry.kind().is_text())
    }

    pub fn set_mode(&mut self, id: WorkbookId, mode: ViewMode) {
        if let Some(text) = self.text_mut(id) {
            text.mode = mode;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::test_support::*;
    use super::*;
    use crate::documents::Step;
    use crate::entry::{Entry, LinkStatus};
    use crate::session::{FileRecord, Session, SpaceRecord, ViewRecord};
    use crate::text_document::DiskNotice;

    fn open_text(documents: &mut Documents, name: &str) -> WorkbookId {
        documents.open_text(
            Some(PathBuf::from(name)),
            DocumentKind::Markdown,
            ViewMode::Preview,
            Format::default(),
            None,
        )
    }

    fn session_with_text(active: bool) -> Session {
        Session::new(
            true,
            vec![SpaceRecord {
                name: "Notes".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![FileRecord {
                    path: Some("plan.md".to_string()),
                    recovery: None,
                    untitled: 0,
                    dirty: false,
                    read_only: false,
                    kind: DocumentKind::Markdown,
                    mode: ViewMode::Source,
                    base: None,
                    unsupported: Vec::new(),
                    active,
                    sheet: 0,
                    view: ViewRecord {
                        active_row: 0,
                        active_col: 0,
                        corner_row: 0,
                        corner_col: 0,
                        top: 0,
                        left: 0,
                    },
                }],
            }],
        )
    }

    fn restored_text(documents: &mut Documents, active: bool) -> WorkbookId {
        documents.restore(&session_with_text(active), None);
        documents
            .entries()
            .find(|entry| entry.kind() == DocumentKind::Markdown)
            .map(Entry::id)
            .expect("the restored text")
    }

    #[test]
    fn opening_a_text_replaces_the_untouched_blank_and_takes_the_screen() {
        let mut documents = documents();
        let id = open_text(&mut documents, "a.md");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.shown_id(), Some(id));
        assert!(documents.active().is_none());
        assert_eq!(documents.shown_text().unwrap().name(), "a.md");
    }

    #[test]
    fn opening_a_text_beside_a_workbook_with_work_keeps_the_workbook_in_its_tab() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let id = open_text(&mut documents, "a.md");
        assert_eq!(names(&documents), ["Book1", "a.md"]);
        assert_eq!(documents.shown_id(), Some(id));
        assert!(documents.activate(book));
        assert_eq!(documents.active_id(), Some(book));
        assert!(documents.shown_text().is_none());
        assert_eq!(names(&documents), ["Book1", "a.md"]);
    }

    #[test]
    fn activating_a_text_puts_the_workbook_away_without_reordering() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let first = open_text(&mut documents, "a.md");
        let second = open_file(&mut documents, "b.xlsx");
        assert!(documents.activate(first));
        assert_eq!(documents.shown_id(), Some(first));
        assert!(documents.active().is_none());
        assert_eq!(names(&documents), ["Book1", "a.md", "b.xlsx"]);
        assert!(documents.activate(second));
        assert_eq!(documents.shown_id(), Some(second));
        assert!(documents.activate(book));
        assert_eq!(names(&documents), ["Book1", "a.md", "b.xlsx"]);
    }

    #[test]
    fn next_and_previous_step_through_texts_and_workbooks_alike() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let text = open_text(&mut documents, "a.md");
        assert_eq!(documents.step(Step::Next), Some(book));
        assert_eq!(documents.step(Step::Previous), Some(book));
        documents.activate(book);
        assert_eq!(documents.step(Step::Next), Some(text));
    }

    #[test]
    fn closing_the_shown_text_lands_on_its_right_neighbour_then_the_left() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let first = open_text(&mut documents, "a.md");
        let second = open_file(&mut documents, "b.xlsx");
        documents.activate(first);
        assert!(documents.close(first));
        assert_eq!(documents.shown_id(), Some(second));
        assert_eq!(documents.active_id(), Some(second));
        documents.activate(book);
        let last = open_text(&mut documents, "c.md");
        assert!(documents.close(last));
        assert_eq!(documents.shown_id(), Some(second));
    }

    #[test]
    fn closing_the_shown_workbook_can_land_on_a_text() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let text = open_text(&mut documents, "a.md");
        documents.activate(book);
        assert!(documents.close(book));
        assert_eq!(documents.shown_id(), Some(text));
        assert!(documents.active().is_none());
    }

    #[test]
    fn closing_the_last_text_leaves_nothing_on_screen() {
        let mut documents = documents();
        let id = open_text(&mut documents, "a.md");
        assert!(documents.close(id));
        assert_eq!(documents.len(), 0);
        assert_eq!(documents.shown_id(), None);
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.md")));
    }

    #[test]
    fn a_text_with_unsaved_edits_is_listed_and_counted_for_recovery() {
        let mut documents = documents();
        let id = open_text(&mut documents, "reports/a.md");
        assert!(documents.unsaved().is_empty());
        documents.text_mut(id).unwrap().edited();
        let listed = documents.unsaved();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "a.md");
        assert_eq!(listed[0].folder.as_deref(), Some(Path::new("reports")));
        assert!(documents.holds_unsaved_work(id));
        assert_eq!(documents.unsaved_edits(), [(id, 1)]);
    }

    #[test]
    fn untitled_texts_are_numbered_apart_from_untitled_workbooks() {
        let mut documents = busy();
        let id = documents.open_text(
            None,
            DocumentKind::Markdown,
            ViewMode::Preview,
            Format::default(),
            None,
        );
        assert_eq!(documents.text(id).unwrap().name(), "Untitled 1.md");
        assert_eq!(names(&documents)[0], "Book1");
    }

    #[test]
    fn a_text_in_the_snapshot_keeps_its_kind_mode_and_screen() {
        let mut documents = documents();
        let id = open_text(&mut documents, "a.md");
        documents.set_mode(id, ViewMode::Split);
        let snapshot = documents.snapshot(true, Default::default(), |_| None);
        let record = &snapshot.spaces[0].files[0];
        assert_eq!(record.kind, DocumentKind::Markdown);
        assert_eq!(record.mode, ViewMode::Split);
        assert!(record.active);
    }

    #[test]
    fn a_restored_text_is_a_link_that_remembers_its_mode_until_it_loads() {
        let mut documents = documents();
        let id = restored_text(&mut documents, true);
        let Some(Entry::Link(link)) = documents.entry(id) else {
            panic!("expected a link");
        };
        assert_eq!(link.kind, DocumentKind::Markdown);
        assert_eq!(link.mode, ViewMode::Source);
        assert_eq!(link.name(), "plan.md");
        assert_eq!(link.status, LinkStatus::NotLoaded);
    }

    #[test]
    fn installing_a_loaded_text_puts_it_on_screen_in_place_of_its_link() {
        let mut documents = documents();
        let id = restored_text(&mut documents, true);
        assert!(documents.shows_when_loaded(id));
        let outcome = documents.install_text(TextLoad {
            id,
            format: Format::default(),
            stamp: None,
            from_recovery: false,
        });
        assert!(matches!(outcome, Installed::OnScreen));
        assert_eq!(documents.shown_id(), Some(id));
        assert_eq!(documents.text(id).unwrap().mode, ViewMode::Source);
        assert!(documents.active().is_none());
        assert!(!documents.text(id).unwrap().dirty);
    }

    #[test]
    fn a_text_restored_from_a_recovery_copy_is_dirty() {
        let mut documents = documents();
        let id = restored_text(&mut documents, false);
        let outcome = documents.install_text(TextLoad {
            id,
            format: Format::default(),
            stamp: None,
            from_recovery: true,
        });
        assert!(matches!(outcome, Installed::Background));
        let text = documents.text(id).unwrap();
        assert!(text.dirty && text.needs_recovery());
        assert_eq!(text.notice, DiskNotice::None);
    }

    #[test]
    fn a_link_that_is_gone_installs_nothing() {
        let mut documents = documents();
        let outcome = documents.install_text(TextLoad {
            id: WorkbookId(99),
            format: Format::default(),
            stamp: None,
            from_recovery: false,
        });
        assert!(matches!(outcome, Installed::Gone));
    }

    #[test]
    fn a_path_is_a_text_whether_its_tab_is_loaded_or_still_a_link() {
        let mut documents = documents();
        let link = restored_text(&mut documents, false);
        let loaded = open_text(&mut documents, "a.md");
        let book = open_file(&mut documents, "b.xlsx");
        assert!(documents.is_text_at(Path::new("a.md")));
        assert!(documents.is_text_at(Path::new("plan.md")));
        assert!(matches!(documents.entry(link), Some(Entry::Link(_))));
        assert!(!documents.is_text_at(Path::new("b.xlsx")));
        assert!(!documents.is_text_at(Path::new("none.md")));
        assert!(documents.entry(loaded).is_some() && documents.entry(book).is_some());
    }

    #[test]
    fn a_text_never_unloads_to_a_link() {
        let mut documents = documents();
        let first = open_text(&mut documents, "a.md");
        open_text(&mut documents, "b.md");
        assert!(!documents.unload(first));
        assert!(documents.text(first).is_some());
    }

    #[test]
    fn a_file_listed_by_the_agent_folder_gets_its_kind_from_the_extension() {
        let mut documents = documents();
        let report = documents.add_link(PathBuf::from("out/report.md"), Some(10), ViewMode::Split);
        let book = documents.add_link(PathBuf::from("out/sales.xlsx"), Some(10), ViewMode::Split);
        assert_eq!(
            documents.entry(report).unwrap().kind(),
            DocumentKind::Markdown
        );
        assert_eq!(
            documents.entry(book).unwrap().kind(),
            DocumentKind::Workbook
        );
    }

    #[test]
    fn deleting_a_space_moves_its_texts_to_the_neighbour() {
        let mut documents = documents();
        let id = open_text(&mut documents, "a.md");
        let notes = documents.add_space("Notes", Default::default());
        assert!(documents.move_to_space(id, notes));
        assert_eq!(documents.text(id).unwrap().space, notes);
        assert_eq!(documents.current_space(), notes);
        assert!(documents.delete_space(notes));
        assert_ne!(documents.text(id).unwrap().space, notes);
    }
}
