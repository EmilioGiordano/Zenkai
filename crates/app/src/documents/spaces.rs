use zenkai_types::WorkbookId;

use super::Documents;
use crate::entry::Entry;
use crate::space_appearance::{NewSpaceColor, SpaceOverride};
use crate::spaces::{Neighbour, SpaceColor, SpaceId, Spaces};

impl Documents {
    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    pub fn members(&self, space: SpaceId) -> impl Iterator<Item = Entry<'_>> {
        self.entries().filter(move |entry| entry.space() == space)
    }

    pub fn add_space(&mut self, name: &str, color: NewSpaceColor) -> SpaceId {
        self.spaces.add_for_user(name, color)
    }

    pub fn rename_space(&mut self, id: SpaceId, name: &str) -> bool {
        self.spaces.rename(id, name)
    }

    pub fn is_saved(&self, id: WorkbookId) -> bool {
        self.entry(id).is_some_and(|entry| entry.path().is_some())
    }

    pub fn set_space_color(&mut self, id: SpaceId, color: SpaceColor) {
        self.spaces.set_color(id, color);
    }

    pub fn set_space_appearance(&mut self, id: SpaceId, appearance: SpaceOverride) {
        self.spaces.set_appearance(id, appearance);
    }

    pub fn toggle_space(&mut self, id: SpaceId) {
        self.spaces.toggle(id);
    }

    pub fn expand_space(&mut self, id: SpaceId, expanded: bool) {
        self.spaces.expand(id, expanded);
    }

    // Its entries are never closed: they go to the neighbouring space.
    pub fn delete_space(&mut self, id: SpaceId) -> bool {
        let Some(heir) = self.spaces.remove(id) else {
            return false;
        };
        if self.active.space == id {
            self.active.space = heir;
        }
        self.before
            .iter_mut()
            .chain(self.after.iter_mut())
            .filter(|slot| slot.as_entry().space() == id)
            .for_each(|slot| slot.set_space(heir));
        true
    }

    pub fn move_to_space(&mut self, id: WorkbookId, space: SpaceId) -> bool {
        self.spaces.get(space).is_some() && self.set_space(id, space)
    }

    pub fn shift_space(&mut self, id: WorkbookId, side: Neighbour) -> bool {
        let Some(current) = self.entry(id).map(Entry::space) else {
            return false;
        };
        match self.spaces.neighbour(current, side) {
            Some(target) => self.move_to_space(id, target),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    fn member_names(documents: &Documents, space: SpaceId) -> Vec<String> {
        documents.members(space).map(Entry::name).collect()
    }

    #[test]
    fn a_new_document_joins_the_space_of_the_one_before_it() {
        let mut documents = busy();
        let second = documents.add_space("Q3", NewSpaceColor::None);
        let first = documents.active_id();
        documents.move_to_space(first, second);
        open_file(&mut documents, "a.xlsx");
        assert_eq!(member_names(&documents, second), ["Book1", "a.xlsx"]);
    }

    #[test]
    fn moving_changes_the_space_without_touching_the_order_of_documents() {
        let mut documents = busy();
        let q3 = documents.add_space("Q3", NewSpaceColor::None);
        let a = open_file(&mut documents, "a.xlsx");
        open_file(&mut documents, "b.xlsx");
        assert!(documents.move_to_space(a, q3));
        assert_eq!(member_names(&documents, q3), ["a.xlsx"]);
        assert_eq!(names(&documents), ["Book1", "a.xlsx", "b.xlsx"]);
        assert!(!documents.move_to_space(a, SpaceId(99)));
        assert!(!documents.move_to_space(WorkbookId(99), q3));
    }

    #[test]
    fn shifting_moves_to_the_neighbouring_space_and_stops_at_the_ends() {
        let mut documents = documents();
        let first = documents.spaces().first();
        let second = documents.add_space("Q3", NewSpaceColor::None);
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
        let second = documents.add_space("Q3", NewSpaceColor::None);
        let id = documents.active_id();
        documents.move_to_space(id, second);
        assert!(documents.delete_space(second));
        assert_eq!(documents.active().space, first);
        assert_eq!(documents.len(), 1);
        assert!(!documents.delete_space(first));
    }
}
