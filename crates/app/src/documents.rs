use std::path::{Path, PathBuf};
use std::time::Instant;

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Unsupported, Workbook};
use zenkai_types::{SheetId, WorkbookId};

use crate::document::Document;
use crate::entry::{Entry, Link, LinkStatus, Slot};
use crate::files;
use crate::spaces::{SpaceId, Spaces};

mod restore;
#[cfg(test)]
mod session_property;
mod spaces;
#[cfg(test)]
mod test_support;

const MAX_CLOSED: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

// Who asked for a workbook that is still loading: a restored session only gets the screen if
// the user has not started something else, a user's switch always does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Restore,
    Switch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Want {
    id: WorkbookId,
    intent: Intent,
}

#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

// The workbook on screen, when there is one, is held apart from the rest and is always
// loaded. The others, in tab order around it, may be links that load on activation. With
// nothing on screen every slot is in `before`.
pub struct Documents {
    before: Vec<Slot>,
    active: Option<Box<Document>>,
    after: Vec<Slot>,
    wanted: Option<Want>,
    next_id: u64,
    // The space a workbook created while nothing is open joins.
    empty_space: SpaceId,
    closed: Vec<PathBuf>,
    spaces: Spaces,
}

pub enum Installed {
    OnScreen,
    Background,
    Gone,
}

pub struct Loaded {
    pub id: WorkbookId,
    pub workbook: Workbook,
    pub unsupported: Vec<Unsupported>,
    pub read_only: bool,
    pub origin: FileOrigin,
    pub from_recovery: bool,
}

impl Loaded {
    fn into_document(self, link: &Link) -> Document {
        let unsupported = if self.from_recovery {
            link.unsupported.clone()
        } else {
            self.unsupported
        };
        let mut document = Document::new(
            link.id,
            link.space,
            link.untitled,
            self.workbook,
            link.path.clone(),
            unsupported,
        );
        document.read_only = self.read_only || (self.from_recovery && link.read_only);
        document.dirty = self.from_recovery && link.dirty;
        document.origin = link.origin.unwrap_or(self.origin);
        document.view = link.view;
        let last = u32::try_from(document.sheets.len().saturating_sub(1)).unwrap_or(0);
        document.sheet = SheetId(link.sheet.0.min(last));
        document
    }
}

// Takes a loaded workbook out of the list; anything else stays where it is.
fn take_loaded(slots: &mut Vec<Slot>, index: usize) -> Option<Box<Document>> {
    if index >= slots.len() {
        return None;
    }
    match slots.remove(index) {
        Slot::Loaded(document) => Some(document),
        other => {
            slots.insert(index, other);
            None
        }
    }
}

impl Documents {
    pub fn new(first: Workbook) -> Documents {
        let spaces = Spaces::new();
        let space = spaces.first();
        let active = Some(Box::new(Document::new(
            WorkbookId(0),
            space,
            1,
            first,
            None,
            Vec::new(),
        )));
        Documents {
            before: Vec::new(),
            active,
            after: Vec::new(),
            wanted: None,
            next_id: 1,
            empty_space: space,
            closed: Vec::new(),
            spaces,
        }
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
        let space = self.current_space();
        let document = Box::new(self.new_document(space, workbook, path, unsupported));
        let id = document.id;
        match self.active.replace(document) {
            Some(old) if replace_blank && old.is_pristine() => {}
            old => {
                self.before.extend(old.map(Slot::Loaded));
                self.before.append(&mut self.after);
            }
        }
        self.wanted = None;
        id
    }

    fn new_document(
        &mut self,
        space: SpaceId,
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
    ) -> Document {
        let id = self.take_id();
        let untitled = if path.is_none() {
            self.lowest_free_untitled()
        } else {
            0
        };
        Document::new(id, space, untitled, workbook, path, unsupported)
    }

    fn take_id(&mut self) -> WorkbookId {
        self.next_id += 1;
        WorkbookId(self.next_id - 1)
    }

    fn lowest_free_untitled(&self) -> u32 {
        let taken: Vec<u32> = self
            .entries()
            .filter(|entry| entry.path().is_none())
            .map(Entry::untitled)
            .collect();
        (1..).find(|number| !taken.contains(number)).unwrap_or(1)
    }

    pub fn active(&self) -> Option<&Document> {
        self.active.as_deref()
    }

    pub fn active_mut(&mut self) -> Option<&mut Document> {
        self.active.as_deref_mut()
    }

    // The space where a workbook created now would land.
    pub fn current_space(&self) -> SpaceId {
        self.active.as_ref().map_or(self.empty_space, |a| a.space)
    }

    pub fn active_id(&self) -> Option<WorkbookId> {
        self.active.as_ref().map(|document| document.id)
    }

    // Every workbook in tab order, the one on screen included.
    pub fn entries(&self) -> impl Iterator<Item = Entry<'_>> {
        self.before
            .iter()
            .map(Slot::as_entry)
            .chain(self.active.as_deref().map(Entry::Loaded))
            .chain(self.after.iter().map(Slot::as_entry))
    }

    pub fn entry(&self, id: WorkbookId) -> Option<Entry<'_>> {
        self.entries().find(|entry| entry.id() == id)
    }

    fn slot_mut(&mut self, id: WorkbookId) -> Option<SlotMut<'_>> {
        if let Some(active) = self.active.as_deref_mut().filter(|a| a.id == id) {
            return Some(SlotMut::Active(active));
        }
        self.before
            .iter_mut()
            .chain(self.after.iter_mut())
            .find(|slot| slot.id() == id)
            .map(SlotMut::Other)
    }

    fn set_space(&mut self, id: WorkbookId, space: SpaceId) -> bool {
        match self.slot_mut(id) {
            Some(SlotMut::Active(document)) => document.space = space,
            Some(SlotMut::Other(slot)) => slot.set_space(space),
            None => return false,
        }
        true
    }

    pub fn get(&self, id: WorkbookId) -> Option<&Document> {
        self.entry(id).and_then(Entry::loaded)
    }

    pub fn get_mut(&mut self, id: WorkbookId) -> Option<&mut Document> {
        match self.slot_mut(id)? {
            SlotMut::Active(document) => Some(document),
            SlotMut::Other(slot) => slot.loaded_mut(),
        }
    }

    fn link_mut(&mut self, id: WorkbookId) -> Option<&mut Link> {
        match self.slot_mut(id)? {
            SlotMut::Other(Slot::Link(link)) => Some(link),
            _ => None,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.entries().filter_map(Entry::loaded)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        self.before
            .iter_mut()
            .filter_map(Slot::loaded_mut)
            .chain(self.active.as_deref_mut())
            .chain(self.after.iter_mut().filter_map(Slot::loaded_mut))
    }

    pub fn len(&self) -> usize {
        self.before.len() + usize::from(self.active.is_some()) + self.after.len()
    }

    pub fn position_of(&self, id: WorkbookId) -> usize {
        self.entries()
            .position(|entry| entry.id() == id)
            .map_or(0, |index| index + 1)
    }

    // The edit counts of the workbooks holding work that exists only in memory.
    pub fn unsaved_edits(&self) -> Vec<(WorkbookId, u64)> {
        self.iter()
            .filter(|document| document.needs_recovery())
            .map(|document| (document.id, document.edit_count()))
            .collect()
    }

    pub fn has_path(&self, path: &Path) -> bool {
        self.entries().any(|entry| entry.path() == Some(path))
    }

    // Spellings that differ only by case, separators or `.` and `..` are the same file; the
    // check never touches the disk, so a slow or offline share cannot stall the window.
    pub fn find_by_path(&self, path: &Path) -> Option<WorkbookId> {
        self.entries()
            .find(|entry| {
                entry
                    .path()
                    .is_some_and(|open| files::same_path(open, path))
            })
            .map(Entry::id)
    }

    // Only a loaded workbook can be on screen; a link asks for loading with `want`.
    pub fn activate(&mut self, id: WorkbookId) -> bool {
        if let Some(active) = self.active.as_deref_mut().filter(|a| a.id == id) {
            active.last_used = Instant::now();
            self.wanted = None;
            return true;
        }
        if let Some(index) = self
            .before
            .iter()
            .position(|slot| slot.id() == id && slot.is_loaded())
            && let Some(target) = take_loaded(&mut self.before, index)
        {
            self.promote(Side::Before, index, target, true);
            return true;
        }
        if let Some(index) = self
            .after
            .iter()
            .position(|slot| slot.id() == id && slot.is_loaded())
            && let Some(target) = take_loaded(&mut self.after, index)
        {
            self.promote(Side::After, index, target, true);
            return true;
        }
        false
    }

    // `target` was already taken out of `side` at `index`; it takes the screen and the
    // workbook that was on screen goes back in its place, unless it is dropped.
    fn promote(&mut self, side: Side, index: usize, target: Box<Document>, keep_old: bool) {
        let mut target = target;
        target.last_used = Instant::now();
        let old = self
            .active
            .replace(target)
            .filter(|_| keep_old)
            .map(|mut old| {
                old.last_used = Instant::now();
                Slot::Loaded(old)
            });
        match side {
            Side::Before => {
                let mut tail = self.before.split_off(index);
                tail.extend(old);
                tail.append(&mut self.after);
                self.after = tail;
            }
            Side::After => {
                let rest = self.after.split_off(index);
                self.before.extend(old);
                self.before.append(&mut self.after);
                self.after = rest;
            }
        }
        self.wanted = None;
    }

    pub fn want(&mut self, id: WorkbookId, intent: Intent) {
        if self.entry(id).is_some() {
            self.wanted = Some(Want { id, intent });
        }
    }

    pub fn wanted(&self) -> Option<WorkbookId> {
        self.wanted.map(|want| want.id)
    }

    // Stepping starts from the workbook being waited for, so repeated presses keep moving
    // while a link is still loading.
    pub fn step(&self, step: Step) -> Option<WorkbookId> {
        let ids: Vec<WorkbookId> = self.entries().map(Entry::id).collect();
        let from = self.wanted().or_else(|| self.active_id());
        let index = from.and_then(|from| ids.iter().position(|id| *id == from));
        let count = ids.len();
        let target = match (step, index) {
            (Step::Next, Some(index)) => (index + 1) % count,
            (Step::Previous, Some(index)) => (index + count - 1) % count,
            (Step::Next, None) => 0,
            (Step::Previous, None) => count.checked_sub(1)?,
        };
        ids.get(target).copied()
    }

    pub fn start_loading(&mut self, id: WorkbookId) -> Option<Link> {
        let link = self.link_mut(id)?;
        if link.status == LinkStatus::Loading {
            return None;
        }
        link.status = LinkStatus::Loading;
        Some(link.clone())
    }

    pub fn set_link_status(&mut self, id: WorkbookId, status: LinkStatus) {
        if let Some(link) = self.link_mut(id) {
            link.status = status;
        }
        if status != LinkStatus::Loading && self.wanted() == Some(id) {
            self.wanted = None;
        }
    }

    pub fn set_link_size(&mut self, id: WorkbookId, size: Option<u64>) {
        if let Some(link) = self.link_mut(id) {
            link.size = size;
        }
    }

    pub fn shows_when_loaded(&self, id: WorkbookId) -> bool {
        match self.wanted {
            Some(Want { id: wanted, intent }) if wanted == id => match intent {
                Intent::Switch => true,
                Intent::Restore => self.active.as_ref().is_none_or(|a| a.is_pristine()),
            },
            _ => false,
        }
    }

    // The loaded workbook takes its link's place, space and saved view. It comes on screen
    // when it is what the user waits for, dropping an untouched startup blank.
    pub fn install(&mut self, loaded: Loaded) -> Installed {
        let id = loaded.id;
        let on_screen = self.shows_when_loaded(id);
        let blank = self.active.as_ref().is_some_and(|a| a.is_pristine());
        let found = self
            .before
            .iter()
            .position(|slot| slot.id() == id)
            .map(|index| (Side::Before, index))
            .or_else(|| {
                self.after
                    .iter()
                    .position(|slot| slot.id() == id)
                    .map(|index| (Side::After, index))
            });
        let Some((side, index)) = found else {
            return Installed::Gone;
        };
        let slots = match side {
            Side::Before => &mut self.before,
            Side::After => &mut self.after,
        };
        let Some(Slot::Link(link)) = slots.get(index) else {
            return Installed::Gone;
        };
        let document = Box::new(loaded.into_document(link));
        if !on_screen {
            slots[index] = Slot::Loaded(document);
            return Installed::Background;
        }
        slots.remove(index);
        self.promote(side, index, document, !blank);
        Installed::OnScreen
    }

    // A clean workbook off screen goes back to being a link; its saved view and sheet stay.
    // Undo history is dropped with it.
    pub fn unload(&mut self, id: WorkbookId) -> bool {
        if self.wanted() == Some(id) {
            return false;
        }
        let Some(SlotMut::Other(slot)) = self.slot_mut(id) else {
            return false;
        };
        match slot {
            Slot::Loaded(document) if document.can_unload() => {
                *slot = Slot::Link(document.to_link());
                true
            }
            _ => false,
        }
    }

    // The nearest loaded neighbour takes over, to the right first, so the screen never lands
    // on a link. With none left nothing is on screen, as in Excel.
    pub fn close(&mut self, id: WorkbookId) -> bool {
        if self.wanted() == Some(id) {
            self.wanted = None;
        }
        if self.active_id() != Some(id) {
            let removed = [&mut self.before, &mut self.after]
                .into_iter()
                .find_map(|slots| {
                    let index = slots.iter().position(|slot| slot.id() == id)?;
                    Some(slots.remove(index))
                });
            return match removed {
                Some(slot) => {
                    self.remember(slot.as_entry().path());
                    true
                }
                None => false,
            };
        }
        let next_right = self.after.iter().position(Slot::is_loaded);
        let next_left = self.before.iter().rposition(Slot::is_loaded);
        let target = if let Some(index) = next_right
            && let Some(target) = take_loaded(&mut self.after, index)
        {
            let rest = self.after.split_off(index);
            self.before.append(&mut self.after);
            self.after = rest;
            Some(target)
        } else if let Some(index) = next_left
            && let Some(target) = take_loaded(&mut self.before, index)
        {
            let mut tail = self.before.split_off(index);
            tail.append(&mut self.after);
            self.after = tail;
            Some(target)
        } else {
            self.before.append(&mut self.after);
            None
        };
        let Some(closed) = std::mem::replace(&mut self.active, target) else {
            return false;
        };
        self.empty_space = closed.space;
        self.remember(closed.path.as_deref());
        true
    }

    fn remember(&mut self, path: Option<&Path>) {
        if let Some(path) = path {
            self.closed.retain(|known| known != path);
            self.closed.push(path.to_path_buf());
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
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

enum SlotMut<'a> {
    Active(&'a mut Document),
    Other(&'a mut Slot),
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::session::{FileRecord, Session, SpaceRecord, ViewRecord};
    use crate::space_appearance::{NewSpaceColor, SpaceAppearance};

    fn session_with_files() -> SpaceRecord {
        let file = |path: &str| FileRecord {
            path: Some(path.to_string()),
            recovery: None,
            untitled: 0,
            dirty: false,
            read_only: false,
            unsupported: Vec::new(),
            active: false,
            sheet: 0,
            view: ViewRecord {
                active_row: 0,
                active_col: 0,
                corner_row: 0,
                corner_col: 0,
                top: 0,
                left: 0,
            },
        };
        SpaceRecord {
            name: "Q3".to_string(),
            collapsed: false,
            color: Default::default(),
            appearance: Default::default(),
            files: vec![file("b.xlsx"), file("c.xlsx")],
        }
    }

    #[test]
    fn starts_with_one_active_blank_workbook() {
        let documents = documents();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.active().unwrap().name_with_marker(), "Book1");
    }

    #[test]
    fn opening_a_file_replaces_the_untouched_blank() {
        let mut documents = documents();
        let id = open_file(&mut documents, "a.xlsx");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.active_id().unwrap(), id);
    }

    #[test]
    fn creating_adds_beside_the_untouched_blank() {
        let mut documents = documents();
        let id = documents.create(blank());
        assert_eq!(names(&documents), ["Book1", "Book2"]);
        assert_eq!(documents.active_id().unwrap(), id);
        assert_eq!(documents.position_of(id), 2);
    }

    #[test]
    fn opening_adds_and_activates_when_the_active_has_work() {
        let mut documents = busy();
        let id = open_file(&mut documents, "a.xlsx");
        assert_eq!(names(&documents), ["Book1", "a.xlsx"]);
        assert_eq!(documents.active_id().unwrap(), id);
    }

    #[test]
    fn untitled_workbooks_are_numbered_in_order_of_creation() {
        let mut documents = busy();
        documents.open(blank(), None, Vec::new());
        assert_eq!(names(&documents), ["Book1", "Book2"]);
    }

    #[test]
    fn ids_grow_and_are_never_reused_after_a_close() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        assert!(first < second && second < third);
        documents.close(third);
        let fourth = open_file(&mut documents, "c.xlsx");
        assert!(fourth > third);
    }

    #[test]
    fn activate_finds_by_id_and_refuses_unknown_ids() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        open_file(&mut documents, "a.xlsx");
        assert!(documents.activate(first));
        assert_eq!(documents.active_id().unwrap(), first);
        assert!(!documents.activate(WorkbookId(99)));
        assert_eq!(documents.active_id().unwrap(), first);
    }

    #[test]
    fn activating_keeps_the_tab_order() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        open_file(&mut documents, "a.xlsx");
        open_file(&mut documents, "b.xlsx");
        documents.activate(first);
        assert_eq!(names(&documents), ["Book1", "a.xlsx", "b.xlsx"]);
        assert_eq!(documents.active().unwrap().name(), "Book1");
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        assert_eq!(documents.step(Step::Next).unwrap(), first);
        documents.activate(first);
        assert_eq!(documents.step(Step::Next).unwrap(), second);
        documents.activate(second);
        assert_eq!(documents.step(Step::Previous).unwrap(), first);
        documents.activate(first);
        assert_eq!(documents.step(Step::Previous).unwrap(), third);
    }

    #[test]
    fn closing_the_active_activates_its_right_neighbour_then_the_left() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        documents.activate(second);
        documents.close(second);
        assert_eq!(documents.active_id().unwrap(), third);
        assert_eq!(names(&documents), ["Book1", "b.xlsx"]);
        documents.close(third);
        assert_eq!(documents.active_id().unwrap(), first);
    }

    #[test]
    fn closing_another_document_keeps_the_active_one() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        documents.close(first);
        assert_eq!(documents.active_id().unwrap(), third);
        assert_eq!(documents.len(), 2);
    }

    #[test]
    fn closing_the_last_document_leaves_nothing_open() {
        let mut documents = documents();
        let only = documents.active_id().unwrap();
        assert!(documents.close(only));
        assert_eq!(documents.len(), 0);
        assert!(documents.active().is_none());
        assert_eq!(documents.active_id(), None);
        assert_eq!(documents.entries().count(), 0);
        assert!(!documents.close(only));
    }

    #[test]
    fn a_workbook_created_from_the_empty_state_is_book1_again() {
        let mut documents = documents();
        let only = documents.active_id().unwrap();
        documents.close(only);
        let id = documents.create(blank());
        assert_eq!(documents.active_id(), Some(id));
        assert_eq!(documents.active().unwrap().name(), "Book1");
        assert_eq!(documents.len(), 1);
    }

    #[test]
    fn untitled_numbers_reuse_the_lowest_free_one() {
        let mut documents = busy();
        let second = documents.create(blank());
        documents.create(blank());
        assert_eq!(names(&documents), ["Book1", "Book2", "Book3"]);
        documents.close(second);
        documents.create(blank());
        assert_eq!(names(&documents), ["Book1", "Book3", "Book2"]);
    }

    #[test]
    fn opening_a_file_from_the_empty_state_makes_it_the_only_workbook() {
        let mut documents = documents();
        let only = documents.active_id().unwrap();
        documents.close(only);
        let id = open_file(&mut documents, "a.xlsx");
        assert_eq!(documents.active_id(), Some(id));
        assert_eq!(names(&documents), ["a.xlsx"]);
    }

    #[test]
    fn the_empty_state_keeps_the_space_of_the_closed_workbook() {
        let mut documents = documents();
        let q3 = documents.add_space("Q3", NewSpaceColor::None);
        let only = documents.active_id().unwrap();
        documents.move_to_space(only, q3);
        documents.close(only);
        assert_eq!(documents.current_space(), q3);
        let id = documents.create(blank());
        assert_eq!(documents.get(id).unwrap().space, q3);
    }

    #[test]
    fn closing_the_active_with_only_links_left_leaves_nothing_on_screen() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        documents.restore(&Session::new(false, vec![session_with_files()]), None);
        documents.close(first);
        assert!(documents.active().is_none());
        assert_eq!(documents.len(), 2);
    }

    #[test]
    fn stepping_from_the_empty_state_starts_at_either_end() {
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        let second = open_file(&mut documents, "a.xlsx");
        documents.restore(&Session::new(false, vec![session_with_files()]), None);
        documents.close(first);
        documents.close(second);
        assert!(documents.active().is_none());
        let ids: Vec<_> = documents.entries().map(Entry::id).collect();
        assert_eq!(documents.step(Step::Next), ids.first().copied());
        assert_eq!(documents.step(Step::Previous), ids.last().copied());
    }

    #[test]
    fn stepping_with_nothing_at_all_goes_nowhere() {
        let mut documents = documents();
        let only = documents.active_id().unwrap();
        documents.close(only);
        assert_eq!(documents.step(Step::Next), None);
        assert_eq!(documents.step(Step::Previous), None);
    }

    #[test]
    fn the_snapshot_of_the_empty_state_lists_no_files() {
        let mut documents = documents();
        let only = documents.active_id().unwrap();
        documents.close(only);
        let snapshot = documents.snapshot(true, SpaceAppearance::default(), |_| None);
        assert!(snapshot.spaces.iter().all(|space| space.files.is_empty()));
    }

    #[test]
    fn a_reopened_file_comes_back_from_the_empty_state() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        documents.close(a);
        assert!(documents.active().is_none());
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.xlsx")));
    }

    #[test]
    fn closing_an_unknown_id_changes_nothing() {
        let mut documents = documents();
        assert!(!documents.close(WorkbookId(99)));
        assert_eq!(documents.len(), 1);
    }

    #[test]
    fn reopen_returns_the_last_closed_path_first() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        let b = open_file(&mut documents, "b.xlsx");
        documents.close(a);
        documents.close(b);
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("b.xlsx")));
        assert_eq!(documents.take_reopenable(), Some(PathBuf::from("a.xlsx")));
        assert_eq!(documents.take_reopenable(), None);
    }

    #[test]
    fn reopen_skips_untitled_documents_and_paths_already_open() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        let untitled = documents.open(blank(), None, Vec::new());
        documents.close(a);
        documents.close(untitled);
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
        let mut documents = busy();
        let first = documents.active_id().unwrap();
        documents.active_mut().unwrap().queue(Box::new(|_| Ok(())));
        let generation = documents.active().unwrap().generation();
        assert!(documents.active_mut().unwrap().take_batch().is_some());
        open_file(&mut documents, "a.xlsx");
        let background = documents.get_mut(first).unwrap();
        assert!(background.has_pending());
        assert!(background.finish_batch(generation));
        assert!(!background.has_pending());
    }

    #[test]
    fn an_edit_during_closing_shows_in_the_unsaved_edit_counts() {
        let mut documents = busy();
        let before = documents.unsaved_edits();
        assert_eq!(before, [(documents.active_id().unwrap(), 0)]);
        documents.active_mut().unwrap().queue(Box::new(|_| Ok(())));
        assert_ne!(documents.unsaved_edits(), before);
    }

    #[test]
    fn a_workbook_that_becomes_dirty_shows_in_the_unsaved_edit_counts() {
        let mut documents = documents();
        assert!(documents.unsaved_edits().is_empty());
        documents.active_mut().unwrap().dirty = true;
        assert_eq!(documents.unsaved_edits().len(), 1);
    }

    #[test]
    fn an_idle_workbook_unloads_to_a_link_that_keeps_its_place_and_view() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        documents.active_mut().unwrap().view.top = zenkai_types::RowIdx::clamped(40);
        let b = open_file(&mut documents, "b.xlsx");
        assert!(documents.unload(a));
        assert!(documents.entry(a).unwrap().loaded().is_none());
        assert_eq!(documents.active_id().unwrap(), b);
        assert_eq!(names(&documents), ["a.xlsx", "b.xlsx"]);
        let Some(Entry::Link(link)) = documents.entry(a) else {
            panic!("expected a link");
        };
        assert_eq!(link.view.top.get(), 40);
        assert_eq!(link.status, LinkStatus::NotLoaded);
    }

    #[test]
    fn the_workbook_on_screen_one_with_unsaved_work_and_one_being_waited_for_never_unload() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        let b = open_file(&mut documents, "b.xlsx");
        assert!(!documents.unload(b));
        documents.get_mut(a).unwrap().dirty = true;
        assert!(!documents.unload(a));
        documents.get_mut(a).unwrap().dirty = false;
        documents.want(a, Intent::Switch);
        assert!(!documents.unload(a));
        assert!(documents.get(a).is_some() && documents.get(b).is_some());
    }

    #[test]
    fn a_workbook_that_is_busy_or_has_no_file_never_unloads() {
        let mut documents = documents();
        let untitled = documents.create(blank());
        documents
            .get_mut(untitled)
            .unwrap()
            .queue(Box::new(|_| Ok(())));
        let a = open_file(&mut documents, "a.xlsx");
        open_file(&mut documents, "b.xlsx");
        assert!(!documents.unload(untitled));
        documents.get_mut(a).unwrap().queue(Box::new(|_| Ok(())));
        assert!(!documents.unload(a));
    }

    #[test]
    fn an_unloaded_workbook_comes_back_where_it_was() {
        let mut documents = documents();
        let a = open_file(&mut documents, "a.xlsx");
        documents.active_mut().unwrap().view.top = zenkai_types::RowIdx::clamped(40);
        open_file(&mut documents, "b.xlsx");
        documents.unload(a);
        documents.want(a, Intent::Switch);
        assert!(matches!(documents.install(loaded(a)), Installed::OnScreen));
        assert_eq!(documents.active().unwrap().view.top.get(), 40);
        assert_eq!(
            documents.active().unwrap().path.as_deref(),
            Some(Path::new("a.xlsx"))
        );
        assert_eq!(names(&documents), ["a.xlsx", "b.xlsx"]);
    }
}
