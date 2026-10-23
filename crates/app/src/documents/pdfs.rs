use std::path::PathBuf;

use zenkai_types::WorkbookId;

use super::{Documents, Installed};
use crate::entry::Slot;
use crate::pdf::PdfPosition;
use crate::pdf_tab::PdfTab;

impl Documents {
    // An untouched blank workbook gives way to the PDF opened, as it does to a workbook.
    pub fn open_pdf(&mut self, path: PathBuf, position: PdfPosition) -> WorkbookId {
        let space = self.current_space();
        let id = self.take_id();
        let keep_old = self
            .active
            .as_ref()
            .is_some_and(|active| !active.is_pristine());
        let old = self.active.take().filter(|_| keep_old);
        self.before.extend(old.map(Slot::Loaded));
        self.before.append(&mut self.after);
        self.before.push(Slot::Pdf(Box::new(PdfTab {
            id,
            space,
            path,
            position,
        })));
        self.tab_shown = Some(id);
        self.wanted = None;
        id
    }

    // The PDF takes its link's place, space and remembered position. Opening the file is the
    // view's job, so nothing is read here.
    pub fn install_pdf(&mut self, id: WorkbookId) -> Installed {
        let on_screen = self.shows_when_loaded(id);
        let blank = self.active.as_ref().is_some_and(|a| a.is_pristine());
        let Some(slot) = self
            .before
            .iter_mut()
            .chain(self.after.iter_mut())
            .find(|slot| slot.id() == id)
        else {
            return Installed::Gone;
        };
        let Slot::Link(link) = slot else {
            return Installed::Gone;
        };
        let Some(path) = link.path.clone() else {
            return Installed::Gone;
        };
        *slot = Slot::Pdf(Box::new(PdfTab {
            id,
            space: link.space,
            path,
            position: link.pdf,
        }));
        if !on_screen {
            return Installed::Background;
        }
        self.show_tab(id, !blank);
        Installed::OnScreen
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::test_support::*;
    use super::*;
    use crate::entry::Entry;
    use crate::pdf::layout::ZoomMode;
    use crate::session::{FileRecord, Session, SpaceRecord, ViewRecord};
    use crate::text_document::DocumentKind;

    fn open(documents: &mut Documents, name: &str) -> WorkbookId {
        documents.open_pdf(PathBuf::from(name), PdfPosition::default())
    }

    fn session_with_pdf(active: bool, position: PdfPosition) -> Session {
        Session::new(
            true,
            vec![SpaceRecord {
                name: "Reading".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![FileRecord {
                    path: Some("manual.pdf".to_string()),
                    recovery: None,
                    untitled: 0,
                    dirty: false,
                    read_only: true,
                    kind: DocumentKind::Pdf,
                    mode: Default::default(),
                    base: None,
                    pdf: position,
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

    fn restored_pdf(documents: &mut Documents, active: bool, position: PdfPosition) -> WorkbookId {
        documents.restore(&session_with_pdf(active, position), None);
        documents
            .entries()
            .find(|entry| entry.kind() == DocumentKind::Pdf)
            .map(Entry::id)
            .expect("the restored pdf")
    }

    #[test]
    fn opening_a_pdf_replaces_the_untouched_blank_and_takes_the_screen() {
        let mut documents = documents();
        let id = open(&mut documents, "a.pdf");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.shown_id(), Some(id));
        assert!(documents.active().is_none());
        assert_eq!(documents.shown_pdf().unwrap().name(), "a.pdf");
        assert!(documents.shown_text().is_none());
    }

    #[test]
    fn a_pdf_beside_a_workbook_with_work_keeps_the_workbook_in_its_tab() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let id = open(&mut documents, "a.pdf");
        assert_eq!(names(&documents), ["Book1", "a.pdf"]);
        assert_eq!(documents.shown_id(), Some(id));
        assert!(documents.activate(book));
        assert!(documents.shown_pdf().is_none());
        assert!(documents.activate(id));
        assert_eq!(documents.shown_id(), Some(id));
        assert!(documents.active().is_none());
        assert_eq!(names(&documents), ["Book1", "a.pdf"]);
    }

    #[test]
    fn a_pdf_is_never_dirty_and_holds_no_unsaved_work() {
        let mut documents = documents();
        let id = open(&mut documents, "a.pdf");
        assert!(documents.unsaved().is_empty());
        assert!(!documents.holds_unsaved_work(id));
        assert!(documents.unsaved_edits().is_empty());
    }

    #[test]
    fn a_pdf_never_unloads_or_hibernates() {
        let mut documents = documents();
        let first = open(&mut documents, "a.pdf");
        open(&mut documents, "b.pdf");
        assert!(!documents.unload(first));
        assert!(documents.begin_hibernation(first).is_none());
        assert!(matches!(documents.entry(first), Some(Entry::Pdf(_))));
    }

    #[test]
    fn closing_the_shown_pdf_lands_on_a_neighbour_and_remembers_the_file() {
        let mut documents = busy();
        let book = documents.active_id().unwrap();
        let id = open(&mut documents, "a.pdf");
        assert!(documents.close(id));
        assert_eq!(documents.shown_id(), Some(book));
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.pdf")));
    }

    #[test]
    fn a_pdf_turns_into_a_link_that_keeps_its_position() {
        let mut documents = busy();
        let position = PdfPosition {
            page: 9,
            zoom: ZoomMode::Percent(150),
        };
        let id = documents.open_pdf(PathBuf::from("a.pdf"), position);
        let link = documents.entry(id).unwrap().to_link();
        assert_eq!(link.kind, DocumentKind::Pdf);
        assert_eq!(link.pdf, position);
        assert_eq!(link.path.as_deref(), Some(Path::new("a.pdf")));
    }

    #[test]
    fn a_session_restores_a_pdf_at_the_page_and_zoom_it_had() {
        let position = PdfPosition {
            page: 41,
            zoom: ZoomMode::FitPage,
        };
        let mut documents = documents();
        let id = restored_pdf(&mut documents, true, position);
        assert!(matches!(documents.entry(id), Some(Entry::Link(_))));
        assert!(matches!(documents.install_pdf(id), Installed::OnScreen));
        assert_eq!(documents.shown_pdf().unwrap().position, position);
        assert!(documents.shown_text().is_none());
    }

    #[test]
    fn a_pdf_restored_in_the_background_stays_off_screen_until_activated() {
        let mut documents = documents();
        let id = restored_pdf(&mut documents, false, PdfPosition::default());
        assert!(matches!(documents.install_pdf(id), Installed::Background));
        assert!(documents.shown_pdf().is_none());
        assert!(documents.activate(id));
        assert!(documents.shown_pdf().is_some());
    }

    #[test]
    fn the_snapshot_of_a_session_keeps_the_position_of_a_pdf() {
        let mut documents = documents();
        let position = PdfPosition {
            page: 3,
            zoom: ZoomMode::FitWidth,
        };
        let id = documents.open_pdf(PathBuf::from("a.pdf"), position);
        documents.pdf_mut(id).unwrap().position.page = 8;
        let session = documents.snapshot(true, Default::default(), |_| None);
        let record = &session.spaces[0].files[0];
        assert_eq!(record.kind, DocumentKind::Pdf);
        assert_eq!(record.pdf.page, 8);
        assert_eq!(record.pdf.zoom, ZoomMode::FitWidth);
    }

    #[test]
    fn the_position_of_a_pdf_survives_the_json_of_the_session() {
        let position = PdfPosition {
            page: 12,
            zoom: ZoomMode::Percent(175),
        };
        let session = session_with_pdf(true, position);
        let text = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back.spaces[0].files[0].pdf, position);
    }

    #[test]
    fn a_session_written_before_pdfs_still_reads() {
        let session = session_with_pdf(true, PdfPosition::default());
        let mut value = serde_json::to_value(&session).unwrap();
        value["spaces"][0]["files"][0]
            .as_object_mut()
            .unwrap()
            .remove("pdf");
        let back: Session = serde_json::from_value(value).unwrap();
        assert_eq!(back.spaces[0].files[0].pdf, PdfPosition::default());
    }
}
