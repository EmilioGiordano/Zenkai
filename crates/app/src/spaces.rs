#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpaceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Neighbour {
    Previous,
    Next,
}

// Muted on purpose: a space color marks a group, it never competes with the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceColor {
    #[default]
    Default,
    Gray,
    Red,
    Orange,
    Amber,
    Green,
    Teal,
    Blue,
    Violet,
}

impl SpaceColor {
    pub const ALL: [SpaceColor; 9] = [
        SpaceColor::Default,
        SpaceColor::Gray,
        SpaceColor::Red,
        SpaceColor::Orange,
        SpaceColor::Amber,
        SpaceColor::Green,
        SpaceColor::Teal,
        SpaceColor::Blue,
        SpaceColor::Violet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SpaceColor::Default => "Default",
            SpaceColor::Gray => "Gray",
            SpaceColor::Red => "Red",
            SpaceColor::Orange => "Orange",
            SpaceColor::Amber => "Amber",
            SpaceColor::Green => "Green",
            SpaceColor::Teal => "Teal",
            SpaceColor::Blue => "Blue",
            SpaceColor::Violet => "Violet",
        }
    }

    pub fn rgb(self) -> Option<u32> {
        match self {
            SpaceColor::Default => None,
            SpaceColor::Gray => Some(0x8a8f98),
            SpaceColor::Red => Some(0xc2625d),
            SpaceColor::Orange => Some(0xc98250),
            SpaceColor::Amber => Some(0xbfa04a),
            SpaceColor::Green => Some(0x6fa06f),
            SpaceColor::Teal => Some(0x4f9a9a),
            SpaceColor::Blue => Some(0x5b8abf),
            SpaceColor::Violet => Some(0x8c74b8),
        }
    }

    pub fn next(self) -> SpaceColor {
        let index = Self::ALL
            .iter()
            .position(|color| *color == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub collapsed: bool,
    pub color: SpaceColor,
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
            color: SpaceColor::Default,
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

    pub fn set_color(&mut self, id: SpaceId, color: SpaceColor) {
        if let Some(space) = self.spaces.iter_mut().find(|space| space.id == id) {
            space.color = color;
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
    fn colors_cycle_through_every_swatch_and_back_to_default() {
        let mut color = SpaceColor::Default;
        for _ in 0..SpaceColor::ALL.len() {
            color = color.next();
        }
        assert_eq!(color, SpaceColor::Default);
        assert_eq!(SpaceColor::Default.rgb(), None);
        assert!(SpaceColor::ALL[1..].iter().all(|c| c.rgb().is_some()));
    }

    #[test]
    fn a_color_is_kept_on_its_space() {
        let mut spaces = Spaces::new();
        let id = spaces.first();
        spaces.set_color(id, SpaceColor::Teal);
        assert_eq!(spaces.get(id).unwrap().color, SpaceColor::Teal);
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
