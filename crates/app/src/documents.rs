use std::path::{Path, PathBuf};

use zenkai_engine::{Unsupported, Workbook};
use zenkai_types::{SheetId, WorkbookId};

use crate::document::Document;
use crate::entry::{Entry, Link, LinkStatus};
use crate::files;
use crate::session::{self, Session, SpaceRecord};
use crate::spaces::{Neighbour, SpaceId, Spaces};

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

// The workbook on screen is always loaded (`active` only ever points at a loaded entry); the
// rest may be links that load on activation. Closing the last entry opens a new blank.
pub struct Documents {
    entries: Vec<Entry>,
    active: usize,
    wanted: Option<Want>,
    next_id: u64,
    next_untitled: u32,
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
        document.view = link.view;
        let last = u32::try_from(document.sheets.len().saturating_sub(1)).unwrap_or(0);
        document.sheet = SheetId(link.sheet.0.min(last));
        document
    }
}

impl Documents {
    pub fn new(first: Workbook) -> Documents {
        let mut documents = Documents {
            entries: Vec::new(),
            active: 0,
            wanted: None,
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
        let id = self.take_id();
        let untitled = if path.is_none() {
            self.take_untitled()
        } else {
            0
        };
        let space = self
            .entries
            .get(self.active)
            .map_or_else(|| self.spaces.first(), Entry::space);
        let document = Document::new(id, space, untitled, workbook, path, unsupported);
        let entry = Entry::Loaded(Box::new(document));
        match self.entries.get(self.active) {
            Some(Entry::Loaded(active)) if replace_blank && active.is_pristine() => {
                self.entries[self.active] = entry;
            }
            _ => {
                self.entries.push(entry);
                self.active = self.entries.len() - 1;
            }
        }
        self.wanted = None;
        id
    }

    fn take_id(&mut self) -> WorkbookId {
        self.next_id += 1;
        WorkbookId(self.next_id - 1)
    }

    fn take_untitled(&mut self) -> u32 {
        self.next_untitled += 1;
        self.next_untitled - 1
    }

    pub fn active(&self) -> &Document {
        match &self.entries[self.active] {
            Entry::Loaded(document) => document,
            Entry::Link(_) => unreachable!("the active entry is always loaded"),
        }
    }

    pub fn active_mut(&mut self) -> &mut Document {
        match &mut self.entries[self.active] {
            Entry::Loaded(document) => document,
            Entry::Link(_) => unreachable!("the active entry is always loaded"),
        }
    }

    pub fn active_id(&self) -> WorkbookId {
        self.active().id
    }

    pub fn entry(&self, id: WorkbookId) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    pub fn entry_mut(&mut self, id: WorkbookId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.id() == id)
    }

    pub fn get(&self, id: WorkbookId) -> Option<&Document> {
        self.entry(id).and_then(Entry::loaded)
    }

    pub fn get_mut(&mut self, id: WorkbookId) -> Option<&mut Document> {
        self.entry_mut(id).and_then(Entry::loaded_mut)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.entries.iter().filter_map(Entry::loaded)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        self.entries.iter_mut().filter_map(Entry::loaded_mut)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn position_of(&self, id: WorkbookId) -> usize {
        self.entries
            .iter()
            .position(|entry| entry.id() == id)
            .map_or(0, |index| index + 1)
    }

    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    pub fn members(&self, space: SpaceId) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |entry| entry.space() == space)
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

    // Its entries are never closed: they go to the neighbouring space.
    pub fn delete_space(&mut self, id: SpaceId) -> bool {
        let Some(heir) = self.spaces.remove(id) else {
            return false;
        };
        self.entries
            .iter_mut()
            .filter(|entry| entry.space() == id)
            .for_each(|entry| entry.set_space(heir));
        true
    }

    pub fn move_to_space(&mut self, id: WorkbookId, space: SpaceId) -> bool {
        if self.spaces.get(space).is_none() {
            return false;
        }
        match self.entry_mut(id) {
            Some(entry) => {
                entry.set_space(space);
                true
            }
            None => false,
        }
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

    pub fn has_path(&self, path: &Path) -> bool {
        self.entries.iter().any(|entry| entry.path() == Some(path))
    }

    pub fn find_by_path(&self, path: &Path) -> Option<WorkbookId> {
        self.entries
            .iter()
            .find(|entry| {
                entry
                    .path()
                    .is_some_and(|open| open == path || files::same_file(open, path))
            })
            .map(Entry::id)
    }

    // Only a loaded workbook can be on screen; a link asks for loading with `want`.
    pub fn activate(&mut self, id: WorkbookId) -> bool {
        match self
            .entries
            .iter()
            .position(|entry| entry.id() == id && entry.loaded().is_some())
        {
            Some(index) => {
                self.active = index;
                self.wanted = None;
                true
            }
            None => false,
        }
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
    pub fn step(&self, step: Step) -> WorkbookId {
        let count = self.entries.len();
        let from = self.wanted().unwrap_or_else(|| self.active_id());
        let index = self.position_of(from).saturating_sub(1);
        let target = match step {
            Step::Next => (index + 1) % count,
            Step::Previous => (index + count - 1) % count,
        };
        self.entries[target].id()
    }

    // The link to load, unless it is loading already.
    pub fn start_loading(&mut self, id: WorkbookId) -> Option<Link> {
        let Entry::Link(link) = self.entry_mut(id)? else {
            return None;
        };
        if link.status == LinkStatus::Loading {
            return None;
        }
        link.status = LinkStatus::Loading;
        Some(link.clone())
    }

    pub fn set_link_status(&mut self, id: WorkbookId, status: LinkStatus) {
        if let Some(Entry::Link(link)) = self.entry_mut(id) {
            link.status = status;
        }
        if status != LinkStatus::Loading && self.wanted() == Some(id) {
            self.wanted = None;
        }
    }

    pub fn set_link_size(&mut self, id: WorkbookId, size: Option<u64>) {
        if let Some(Entry::Link(link)) = self.entry_mut(id) {
            link.size = size;
        }
    }

    pub fn shows_when_loaded(&self, id: WorkbookId) -> bool {
        match self.wanted {
            Some(Want { id: wanted, intent }) if wanted == id => match intent {
                Intent::Switch => true,
                Intent::Restore => self.active().is_pristine(),
            },
            _ => false,
        }
    }

    // The loaded workbook takes its link's place, space and saved view. It comes on screen
    // when it is what the user waits for, dropping an untouched startup blank.
    pub fn install(&mut self, loaded: Loaded) -> Installed {
        let id = loaded.id;
        let Some(index) = self.entries.iter().position(|entry| entry.id() == id) else {
            return Installed::Gone;
        };
        let Entry::Link(link) = &self.entries[index] else {
            return Installed::Gone;
        };
        let document = loaded.into_document(link);
        let on_screen = self.shows_when_loaded(id);
        self.entries[index] = Entry::Loaded(Box::new(document));
        if !on_screen {
            return Installed::Background;
        }
        let blank = self.active;
        self.active = index;
        self.wanted = None;
        if self.entries[blank]
            .loaded()
            .is_some_and(Document::is_pristine)
        {
            self.entries.remove(blank);
            if blank < self.active {
                self.active -= 1;
            }
        }
        Installed::OnScreen
    }

    // The nearest loaded neighbour takes over: to the right first, then to the left.
    pub fn close(&mut self, id: WorkbookId, empty: impl FnOnce() -> Workbook) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.id() == id) else {
            return false;
        };
        let closed = self.entries.remove(index);
        if let Some(path) = closed.path() {
            self.closed.retain(|known| known != path);
            self.closed.push(path.to_path_buf());
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        if self.wanted() == Some(id) {
            self.wanted = None;
        }
        if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            let loaded = |i: &usize| self.entries[*i].loaded().is_some();
            let right = (index..self.entries.len()).find(loaded);
            let left = (0..index).rev().find(loaded);
            match right.or(left) {
                Some(next) => self.active = next,
                None => {
                    self.create(empty());
                }
            }
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

    // Spaces and links come back from the session; the startup blank joins the first space
    // and stays on screen until the active workbook has loaded.
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
            for file in &record.files {
                let id = self.take_id();
                self.next_untitled = self.next_untitled.max(file.untitled + 1);
                let link = session::link_of(file, id, space, recovery_directory);
                self.entries.push(Entry::Link(link));
                if file.active {
                    active = Some(id);
                }
            }
        }
        if let Some(blank) = self.entries.get_mut(self.active) {
            blank.set_space(first_space);
        }
        if let Some(id) = active {
            self.want(id, Intent::Restore);
        }
        active
    }

    // Loaded workbooks that hold unsaved work are listed with the recovery copy that keeps it.
    pub fn snapshot(
        &self,
        sidebar_visible: bool,
        recovery_file: impl Fn(WorkbookId) -> PathBuf,
    ) -> Session {
        let active = self.wanted().unwrap_or_else(|| self.active_id());
        let spaces = self
            .spaces
            .iter()
            .map(|space| SpaceRecord {
                name: space.name.clone(),
                collapsed: space.collapsed,
                files: self
                    .members(space.id)
                    .filter(|entry| !entry.loaded().is_some_and(Document::is_pristine))
                    .map(|entry| {
                        let mut link = entry.to_link();
                        if entry.loaded().is_some_and(Document::needs_recovery) {
                            link.recovery = Some(recovery_file(link.id));
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
    use super::*;
    use crate::session::{FileRecord, ViewRecord};

    fn blank() -> Workbook {
        Workbook::new_empty().unwrap()
    }

    fn documents() -> Documents {
        Documents::new(blank())
    }

    fn busy() -> Documents {
        let mut documents = documents();
        documents.active_mut().dirty = true;
        documents
    }

    fn open_file(documents: &mut Documents, name: &str) -> WorkbookId {
        documents.open(blank(), Some(PathBuf::from(name)), Vec::new())
    }

    fn names(documents: &Documents) -> Vec<String> {
        documents.entries().iter().map(Entry::name).collect()
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
        let mut documents = busy();
        let id = open_file(&mut documents, "a.xlsx");
        assert_eq!(names(&documents), ["Book1", "a.xlsx"]);
        assert_eq!(documents.active_id(), id);
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
        let mut documents = busy();
        let first = documents.active_id();
        open_file(&mut documents, "a.xlsx");
        assert!(documents.activate(first));
        assert_eq!(documents.active_id(), first);
        assert!(!documents.activate(WorkbookId(99)));
        assert_eq!(documents.active_id(), first);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut documents = busy();
        let first = documents.active_id();
        let second = open_file(&mut documents, "a.xlsx");
        let third = open_file(&mut documents, "b.xlsx");
        assert_eq!(documents.step(Step::Next), first);
        documents.activate(first);
        assert_eq!(documents.step(Step::Next), second);
        documents.activate(second);
        assert_eq!(documents.step(Step::Previous), first);
        documents.activate(first);
        assert_eq!(documents.step(Step::Previous), third);
    }

    #[test]
    fn closing_the_active_activates_its_right_neighbour_then_the_left() {
        let mut documents = busy();
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
        let mut documents = busy();
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
        let mut documents = busy();
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
        documents.members(space).map(Entry::name).collect()
    }

    #[test]
    fn a_new_document_joins_the_space_of_the_one_before_it() {
        let mut documents = busy();
        let second = documents.add_space("Q3");
        let first = documents.active_id();
        documents.move_to_space(first, second);
        open_file(&mut documents, "a.xlsx");
        assert_eq!(member_names(&documents, second), ["Book1", "a.xlsx"]);
    }

    #[test]
    fn moving_changes_the_space_without_touching_the_order_of_documents() {
        let mut documents = busy();
        let q3 = documents.add_space("Q3");
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
                    files: vec![file(Some("a.xlsx"), false), file(Some("gone.xlsx"), true)],
                },
                SpaceRecord {
                    name: "Suppliers".to_string(),
                    collapsed: true,
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
        let snapshot = documents.snapshot(true, |_| PathBuf::new());
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
        let snapshot =
            documents.snapshot(false, |id| PathBuf::from(format!("recovery/{}.xlsx", id.0)));
        let record = &snapshot.spaces[0].files[0];
        assert_eq!(record.recovery.as_deref(), Some("0.xlsx"));
        assert!(record.dirty);
    }

    fn loaded(id: WorkbookId) -> Loaded {
        Loaded {
            id,
            workbook: blank(),
            unsupported: Vec::new(),
            read_only: false,
            from_recovery: false,
        }
    }

    #[test]
    fn a_restored_active_workbook_replaces_the_untouched_blank_when_it_loads() {
        let mut documents = documents();
        let active = documents.restore(&session(), None).unwrap();
        documents.want(active, Intent::Restore);
        assert!(matches!(
            documents.install(loaded(active)),
            Installed::OnScreen
        ));
        assert_eq!(documents.active_id(), active);
        assert_eq!(documents.len(), 3);
        assert_eq!(documents.active().sheet, SheetId(0));
        assert_eq!(documents.active().view.selection.active.row.get(), 9);
    }

    #[test]
    fn a_workbook_loaded_while_the_user_works_elsewhere_stays_in_the_background() {
        let mut documents = documents();
        let active = documents.restore(&session(), None).unwrap();
        documents.want(active, Intent::Restore);
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
        documents.want(restored, Intent::Restore);
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
        assert_eq!(documents.step(Step::Next), gone);
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
                files: vec![record],
            }],
        );
        let mut documents = documents();
        let id = documents
            .restore(&session, Some(Path::new("recovery")))
            .unwrap();
        documents.want(id, Intent::Restore);
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
