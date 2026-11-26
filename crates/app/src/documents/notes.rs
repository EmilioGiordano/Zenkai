use zenkai_types::WorkbookId;

use super::Documents;
use crate::entry::{Entry, Slot};
use crate::notes_tab::NotesTab;

impl Documents {
    // An untouched blank workbook gives way to the tab, as it does to any other tab.
    pub fn open_notes(&mut self) -> WorkbookId {
        let space = self.current_space();
        let id = self.take_id();
        let keep_old = self
            .active
            .as_ref()
            .is_some_and(|active| !active.is_pristine());
        let old = self.active.take().filter(|_| keep_old);
        self.before.extend(old.map(Slot::Loaded));
        self.before.append(&mut self.after);
        self.before
            .push(Slot::Notes(Box::new(NotesTab { id, space })));
        self.tab_shown = Some(id);
        self.wanted = None;
        id
    }

    pub fn notes_id(&self) -> Option<WorkbookId> {
        self.entries()
            .find_map(|entry| entry.notes().map(|tab| tab.id))
    }

    pub fn notes(&self, id: WorkbookId) -> Option<&NotesTab> {
        self.entry(id).and_then(Entry::notes)
    }

    pub fn shown_notes(&self) -> Option<&NotesTab> {
        self.tab_shown.and_then(|id| self.notes(id))
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use crate::entry::Entry;
    use crate::session::{FileRecord, Session, SpaceRecord, ViewRecord};
    use crate::space_appearance::SpaceAppearance;
    use crate::text_document::DocumentKind;

    #[test]
    fn the_tab_replaces_an_untouched_blank_and_takes_the_screen() {
        let mut documents = documents();
        let id = documents.open_notes();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.shown_id(), Some(id));
        assert!(documents.active().is_none());
        assert_eq!(documents.notes_id(), Some(id));
        assert_eq!(documents.shown_notes().map(|tab| tab.id), Some(id));
    }

    #[test]
    fn beside_a_workbook_with_work_the_workbook_keeps_its_tab() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let id = documents.open_notes();
        assert_eq!(documents.len(), 2);
        assert!(documents.activate(book));
        assert_eq!(documents.shown_id(), Some(book));
        assert!(documents.shown_notes().is_none());
        assert_eq!(documents.notes_id(), Some(id));
    }

    #[test]
    fn the_tab_is_never_dirty_and_has_no_path() {
        let mut documents = documents();
        let id = documents.open_notes();
        let entry = documents.entry(id).unwrap();
        assert!(!entry.dirty());
        assert!(entry.path().is_none());
        assert_eq!(entry.kind(), DocumentKind::WhatsNew);
        assert!(matches!(entry, Entry::Notes(_)));
    }

    #[test]
    fn a_session_that_names_the_tab_does_not_bring_it_back() {
        let mut documents = documents();
        let session = Session::new(
            true,
            vec![SpaceRecord {
                name: "Notes".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![FileRecord {
                    path: None,
                    recovery: None,
                    untitled: 0,
                    dirty: false,
                    read_only: true,
                    kind: DocumentKind::WhatsNew,
                    mode: Default::default(),
                    csv: None,
                    base: None,
                    file_modified: None,
                    file_len: None,
                    pdf: Default::default(),
                    text_zoom: Default::default(),
                    unsupported: Vec::new(),
                    active: true,
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
        );
        documents.restore(&session, None);
        assert!(documents.entries().all(|entry| entry.notes().is_none()));
        assert!(
            documents
                .entries()
                .all(|entry| entry.kind() != DocumentKind::WhatsNew)
        );
    }

    #[test]
    fn it_is_left_out_of_the_session() {
        let mut documents = documents();
        documents.open_notes();
        let session = documents.snapshot(true, SpaceAppearance::default(), |_| None);
        assert!(session.spaces.iter().all(|space| space.files.is_empty()));
    }
}
