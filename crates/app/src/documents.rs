use std::path::{Path, PathBuf};

use zenkai_engine::{Unsupported, Workbook};
use zenkai_types::WorkbookId;

use crate::document::Document;
use crate::files;
use crate::spaces::{Neighbour, SpaceId, Spaces};

const MAX_CLOSED: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

// There is always exactly one active document: closing the last one opens a new blank.
pub struct Documents {
    entries: Vec<Document>,
    active: usize,
    next_id: u64,
    next_untitled: u32,
    closed: Vec<PathBuf>,
    spaces: Spaces,
}

impl Documents {
    pub fn new(first: Workbook) -> Documents {
        let mut documents = Documents {
            entries: Vec::new(),
            active: 0,
            next_id: 0,
            next_untitled: 1,
            closed: Vec::new(),
            spaces: Spaces::new(),
        };
        documents.create(first);
        documents
    }

    // An untouched blank workbook gives way to the one opened, as in Excel.
    pub fn open(
        &mut self,
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
    ) -> WorkbookId {
        self.insert(workbook, path, unsupported, true)
    }

    pub fn create(&mut self, workbook: Workbook) -> WorkbookId {
        self.insert(workbook, None, Vec::new(), false)
    }

    fn insert(
        &mut self,
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
        replace_blank: bool,
    ) -> WorkbookId {
        let id = WorkbookId(self.next_id);
        self.next_id += 1;
        let untitled = if path.is_none() {
            self.next_untitled += 1;
            self.next_untitled - 1
        } else {
            0
        };
        let space = self
            .entries
            .get(self.active)
            .map_or_else(|| self.spaces.first(), |active| active.space);
        let document = Document::new(id, space, untitled, workbook, path, unsupported);
        match self.entries.get(self.active) {
            Some(active) if replace_blank && active.is_pristine() => {
                self.entries[self.active] = document
            }
            _ => {
                self.entries.push(document);
                self.active = self.entries.len() - 1;
            }
        }
        id
    }

    pub fn active(&self) -> &Document {
        &self.entries[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Document {
        &mut self.entries[self.active]
    }

    pub fn active_id(&self) -> WorkbookId {
        self.active().id
    }

    pub fn get(&self, id: WorkbookId) -> Option<&Document> {
        self.entries.iter().find(|document| document.id == id)
    }

    pub fn get_mut(&mut self, id: WorkbookId) -> Option<&mut Document> {
        self.entries.iter_mut().find(|document| document.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn position_of(&self, id: WorkbookId) -> usize {
        self.entries
            .iter()
            .position(|document| document.id == id)
            .map_or(0, |index| index + 1)
    }

    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    pub fn members(&self, space: SpaceId) -> impl Iterator<Item = &Document> {
        self.entries
            .iter()
            .filter(move |document| document.space == space)
    }

    pub fn add_space(&mut self, name: &str) -> SpaceId {
        self.spaces.add(name)
    }

    pub fn rename_space(&mut self, id: SpaceId, name: &str) -> bool {
        self.spaces.rename(id, name)
    }

    pub fn toggle_space(&mut self, id: SpaceId) {
        self.spaces.toggle(id);
    }

    pub fn expand_space(&mut self, id: SpaceId, expanded: bool) {
        self.spaces.expand(id, expanded);
    }

    // Its documents are never closed: they go to the neighbouring space.
    pub fn delete_space(&mut self, id: SpaceId) -> bool {
        let Some(heir) = self.spaces.remove(id) else {
            return false;
        };
        self.entries
            .iter_mut()
            .filter(|document| document.space == id)
            .for_each(|document| document.space = heir);
        true
    }

    pub fn move_to_space(&mut self, id: WorkbookId, space: SpaceId) -> bool {
        if self.spaces.get(space).is_none() {
            return false;
        }
        match self.get_mut(id) {
            Some(document) => {
                document.space = space;
                true
            }
            None => false,
        }
    }

    pub fn shift_space(&mut self, id: WorkbookId, side: Neighbour) -> bool {
        let Some(current) = self.get(id).map(|document| document.space) else {
            return false;
        };
        match self.spaces.neighbour(current, side) {
            Some(target) => self.move_to_space(id, target),
            None => false,
        }
    }

    pub fn has_path(&self, path: &Path) -> bool {
        self.entries
            .iter()
            .any(|document| document.path.as_deref() == Some(path))
    }

    pub fn find_by_path(&self, path: &Path) -> Option<WorkbookId> {
        self.entries
            .iter()
            .find(|document| {
                document
                    .path
                    .as_deref()
                    .is_some_and(|open| open == path || files::same_file(open, path))
            })
            .map(|document| document.id)
    }

    pub fn activate(&mut self, id: WorkbookId) -> bool {
        match self.entries.iter().position(|document| document.id == id) {
            Some(index) => {
                self.active = index;
                true
            }
            None => false,
        }
    }

    pub fn step(&mut self, step: Step) -> WorkbookId {
        let count = self.entries.len();
        self.active = match step {
            Step::Next => (self.active + 1) % count,
            Step::Previous => (self.active + count - 1) % count,
        };
        self.active_id()
    }

    // The neighbour to the right takes over, or the one to the left at the end.
    pub fn close(&mut self, id: WorkbookId, empty: impl FnOnce() -> Workbook) -> bool {
        let Some(index) = self.entries.iter().position(|document| document.id == id) else {
            return false;
        };
        let closed = self.entries.remove(index);
        if let Some(path) = closed.path {
            self.closed.retain(|known| known != &path);
            self.closed.push(path);
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        if self.entries.is_empty() {
            self.create(empty());
        } else if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = index.min(self.entries.len() - 1);
        }
        true
    }

    pub fn take_reopenable(&mut self) -> Option<PathBuf> {
        while let Some(path) = self.closed.pop() {
            if self.find_by_path(&path).is_none() {
                return Some(path);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Workbook {
        Workbook::new_empty().unwrap()
    }

    fn documents() -> Documents {
        Documents::new(blank())
    }

    fn open_file(documents: &mut Documents, name: &str) -> WorkbookId {
        documents.open(blank(), Some(PathBuf::from(name)), Vec::new())
    }

    fn names(documents: &Documents) -> Vec<String> {
        documents
            .iter()
            .map(|document| document.name_with_marker())
            .collect()
    }

    #[test]
    fn starts_with_one_active_blank_workbook() {
        let documents = documents();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.active().name_with_marker(), "Book1");
    }

    #[test]
    fn opening_a_file_replaces_the_untouched_blank() {
        let mut documents = documents();
        let id = open_file(&mut documents, "a.xlsx");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.active_id(), id);
    }

    #[test]
    fn creating_adds_beside_the_untouched_blank() {
        let mut documents = documents();
        let id = documents.create(blank());
        assert_eq!(names(&documents), ["Book1", "Book2"]);
        assert_eq!(documents.active_id(), id);
        assert_eq!(documents.position_of(id), 2);
    }

    #[test]
    fn opening_adds_and_activates_when_the_active_has_work() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let id = open_file(&mut documents, "a.xlsx");
        assert_eq!(names(&documents), ["• Book1", "a.xlsx"]);
        assert_eq!(documents.active_id(), id);
    }

    #[test]
    fn untitled_workbooks_are_numbered_in_order_of_creation() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        documents.open(blank(), None, Vec::new());
        assert_eq!(names(&documents), ["• Book1", "Book2"]);
    }

    #[test]
    fn ids_grow_and_are_never_reused_after_a_close() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let first = documents.active_id();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        assert!(first < second && second < third);
        documents.close(third, blank);
        let fourth = open_file(&mut documents, "c.xlsx");
        assert!(fourth > third);
    }

    #[test]
    fn activate_finds_by_id_and_refuses_unknown_ids() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let first = documents.active_id();
        open_file(&mut documents, "a.xlsx");
        assert!(documents.activate(first));
        assert_eq!(documents.active_id(), first);
        assert!(!documents.activate(WorkbookId(99)));
        assert_eq!(documents.active_id(), first);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let first = documents.active_id();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        assert_eq!(documents.step(Step::Next), first);
        assert_eq!(documents.step(Step::Next), second);
        assert_eq!(documents.step(Step::Previous), first);
        assert_eq!(documents.step(Step::Previous), third);
    }

    #[test]
    fn closing_the_active_activates_its_right_neighbour_then_the_left() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let first = documents.active_id();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        documents.activate(second);
        documents.close(second, blank);
        assert_eq!(documents.active_id(), third);
        documents.close(third, blank);
        assert_eq!(documents.active_id(), first);
    }

    #[test]
    fn closing_another_document_keeps_the_active_one() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let first = documents.active_id();
        open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        documents.close(first, blank);
        assert_eq!(documents.active_id(), third);
        assert_eq!(documents.len(), 2);
    }

    #[test]
    fn closing_the_last_document_leaves_a_fresh_blank() {
        let mut documents = documents();
        let only = documents.active_id();
        assert!(documents.close(only, blank));
        assert_eq!(documents.len(), 1);
        assert_ne!(documents.active_id(), only);
        assert_eq!(documents.active().name_with_marker(), "Book2");
    }

    #[test]
    fn closing_an_unknown_id_changes_nothing() {
        let mut documents = documents();
        assert!(!documents.close(WorkbookId(99), blank));
        assert_eq!(documents.len(), 1);
    }

    #[test]
    fn reopen_returns_the_last_closed_path_first() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        documents.active_mut().dirty = true;
        let b = open_file(&mut documents, "b.xlsx");
        documents.close(a, blank);
        documents.close(b, blank);
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("b.xlsx")));
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.xlsx")));
        assert_eq!(documents.take_reopenable(), None);
    }

    #[test]
    fn reopen_skips_untitled_documents_and_paths_already_open() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        let untitled = documents.open(blank(), None, Vec::new());
        documents.close(a, blank);
        documents.close(untitled, blank);
        open_file(&mut documents, "a.xlsx");
        assert_eq!(documents.take_reopenable(), None);
    }

    #[test]
    fn an_open_path_is_found_by_path() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        assert_eq!(documents.find_by_path(Path::new("a.xlsx")), Some(a));
        assert_eq!(documents.find_by_path(Path::new("b.xlsx")), None);
    }

    #[test]
    fn a_batch_finishing_in_an_inactive_document_releases_that_document() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let first = documents.active_id();
        documents.active_mut().queue(Box::new(|_| Ok(())));
        let generation = documents.active().generation();
        assert!(documents.active_mut().take_batch().is_some());
        open_file(&mut documents, "a.xlsx");
        let background = documents.get_mut(first).unwrap();
        assert!(background.has_pending());
        assert!(background.finish_batch(generation));
        assert!(!background.has_pending());
    }

    fn member_names(documents: &Documents, space: SpaceId) -> Vec<String> {
        documents
            .members(space)
            .map(|document| document.name_with_marker())
            .collect()
    }

    #[test]
    fn a_new_document_joins_the_space_of_the_one_before_it() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let second = documents.add_space("Q3");
        let first = documents.active_id();
        documents.move_to_space(first, second);
        open_file(&mut documents, "a.xlsx");
        assert_eq!(member_names(&documents, second), ["• Book1", "a.xlsx"]);
    }

    #[test]
    fn moving_changes_the_space_without_touching_the_order_of_documents() {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        let q3 = documents.add_space("Q3");
        let a = open_file(&mut documents, "a.xlsx");
        open_file(&mut documents, "b.xlsx");
        assert!(documents.move_to_space(a, q3));
        assert_eq!(member_names(&documents, q3), ["a.xlsx"]);
        assert_eq!(names(&documents), ["• Book1", "a.xlsx", "b.xlsx"]);
        assert!(!documents.move_to_space(a, SpaceId(99)));
        assert!(!documents.move_to_space(WorkbookId(99), q3));
    }

    #[test]
    fn shifting_moves_to_the_neighbouring_space_and_stops_at_the_ends() {
        let mut documents = documents();
        let first = documents.spaces().first();
        let second = documents.add_space("Q3");
        let id = documents.active_id();
        assert!(!documents.shift_space(id, Neighbour::Previous));
        assert!(documents.shift_space(id, Neighbour::Next));
        assert_eq!(documents.active().space, second);
        assert!(!documents.shift_space(id, Neighbour::Next));
        assert!(documents.shift_space(id, Neighbour::Previous));
        assert_eq!(documents.active().space, first);
    }

    #[test]
    fn deleting_a_space_keeps_its_documents_in_the_neighbour() {
        let mut documents = documents();
        let first = documents.spaces().first();
        let second = documents.add_space("Q3");
        let id = documents.active_id();
        documents.move_to_space(id, second);
        assert!(documents.delete_space(second));
        assert_eq!(documents.active().space, first);
        assert_eq!(documents.len(), 1);
        assert!(!documents.delete_space(first));
    }
}
