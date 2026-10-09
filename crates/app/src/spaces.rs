#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpaceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Neighbour {
    Previous,
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub collapsed: bool,
}

// At least one space always exists, so every document has somewhere to be listed.
#[derive(Debug)]
pub struct Spaces {
    spaces: Vec<Space>,
    next_id: u64,
}

pub const FIRST_SPACE_NAME: &str = "Workbooks";
pub const NEW_SPACE_NAME: &str = "New space";

impl Spaces {
    pub fn new() -> Spaces {
        let mut spaces = Spaces {
            spaces: Vec::new(),
            next_id: 0,
        };
        spaces.add(FIRST_SPACE_NAME);
        spaces
    }

    pub fn add(&mut self, name: &str) -> SpaceId {
        let id = SpaceId(self.next_id);
        self.next_id += 1;
        self.spaces.push(Space {
            id,
            name: name.to_string(),
            collapsed: false,
        });
        id
    }

    pub fn first(&self) -> SpaceId {
        self.spaces[0].id
    }

    pub fn iter(&self) -> impl Iterator<Item = &Space> {
        self.spaces.iter()
    }

    pub fn get(&self, id: SpaceId) -> Option<&Space> {
        self.spaces.iter().find(|space| space.id == id)
    }

    pub fn rename(&mut self, id: SpaceId, name: &str) -> bool {
        let name = name.trim();
        match self.spaces.iter_mut().find(|space| space.id == id) {
            Some(space) if !name.is_empty() => {
                space.name = name.to_string();
                true
            }
            _ => false,
        }
    }

    pub fn toggle(&mut self, id: SpaceId) {
        if let Some(space) = self.spaces.iter_mut().find(|space| space.id == id) {
            space.collapsed = !space.collapsed;
        }
    }

    pub fn expand(&mut self, id: SpaceId, expanded: bool) {
        if let Some(space) = self.spaces.iter_mut().find(|space| space.id == id) {
            space.collapsed = !expanded;
        }
    }

    // The last space cannot go; the one that takes over the documents is returned.
    pub fn remove(&mut self, id: SpaceId) -> Option<SpaceId> {
        if self.spaces.len() < 2 {
            return None;
        }
        let index = self.spaces.iter().position(|space| space.id == id)?;
        self.spaces.remove(index);
        Some(self.spaces[index.saturating_sub(1)].id)
    }

    pub fn neighbour(&self, id: SpaceId, side: Neighbour) -> Option<SpaceId> {
        let index = self.spaces.iter().position(|space| space.id == id)?;
        let target = match side {
            Neighbour::Previous => index.checked_sub(1)?,
            Neighbour::Next => index + 1,
        };
        self.spaces.get(target).map(|space| space.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_one_space() {
        let spaces = Spaces::new();
        assert_eq!(spaces.iter().count(), 1);
        assert_eq!(spaces.get(spaces.first()).unwrap().name, FIRST_SPACE_NAME);
    }

    #[test]
    fn ids_are_not_reused_after_a_removal() {
        let mut spaces = Spaces::new();
        let second = spaces.add("Q3");
        let third = spaces.add("Suppliers");
        spaces.remove(third);
        let fourth = spaces.add("Other");
        assert!(second < third && third < fourth);
    }

    #[test]
    fn rename_trims_and_refuses_an_empty_name() {
        let mut spaces = Spaces::new();
        let id = spaces.first();
        assert!(spaces.rename(id, "  Q3 close  "));
        assert_eq!(spaces.get(id).unwrap().name, "Q3 close");
        assert!(!spaces.rename(id, "   "));
        assert_eq!(spaces.get(id).unwrap().name, "Q3 close");
        assert!(!spaces.rename(SpaceId(99), "x"));
    }

    #[test]
    fn the_last_space_cannot_be_removed() {
        let mut spaces = Spaces::new();
        assert_eq!(spaces.remove(spaces.first()), None);
        assert_eq!(spaces.iter().count(), 1);
    }

    #[test]
    fn removing_hands_the_documents_to_the_previous_space_or_the_next_for_the_first() {
        let mut spaces = Spaces::new();
        let first = spaces.first();
        let second = spaces.add("B");
        let third = spaces.add("C");
        assert_eq!(spaces.remove(third), Some(second));
        assert_eq!(spaces.remove(first), Some(second));
        assert_eq!(spaces.iter().count(), 1);
    }

    #[test]
    fn neighbours_stop_at_the_ends() {
        let mut spaces = Spaces::new();
        let first = spaces.first();
        let second = spaces.add("B");
        assert_eq!(spaces.neighbour(first, Neighbour::Previous), None);
        assert_eq!(spaces.neighbour(first, Neighbour::Next), Some(second));
        assert_eq!(spaces.neighbour(second, Neighbour::Next), None);
    }

    #[test]
    fn toggle_and_expand_change_the_collapsed_flag() {
        let mut spaces = Spaces::new();
        let id = spaces.first();
        spaces.toggle(id);
        assert!(spaces.get(id).unwrap().collapsed);
        spaces.expand(id, true);
        assert!(!spaces.get(id).unwrap().collapsed);
    }
}
