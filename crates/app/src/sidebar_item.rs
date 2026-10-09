use std::path::Path;

use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use crate::entry::{Entry, Link, LinkStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    Ready,
    Dirty,
    Loading,
    NotLoaded,
    Missing,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileItem {
    pub id: WorkbookId,
    pub name: String,
    pub place: String,
    pub state: FileState,
    pub active: bool,
}

pub fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    match bytes {
        b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
        b if b >= 10 * MB => format!("{} MB", b / MB),
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b => format!("{} KB", b.div_ceil(KB).max(1)),
    }
}

pub fn folder_name(path: &Path) -> Option<String> {
    path.parent()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn link_meta(link: &Link) -> String {
    let mut parts = Vec::new();
    if link.dirty {
        parts.push(t!("sidebar.unsaved_changes").to_string());
    }
    match link.status {
        LinkStatus::Loading => parts.push(t!("sidebar.loading").to_string()),
        LinkStatus::Missing => parts.push(t!("sidebar.not_found").to_string()),
        LinkStatus::NotLoaded => {
            parts.push(t!("sidebar.not_loaded").to_string());
            parts.extend(link.size.map(human_size));
        }
    }
    parts.join(", ")
}

pub fn describe(entry: Entry, active: bool) -> FileItem {
    let (meta, state) = match entry {
        Entry::Loaded(document) => {
            let count = document.sheets.len();
            let meta = t!("sidebar.sheets", count = count);
            let state = if document.dirty {
                FileState::Dirty
            } else {
                FileState::Ready
            };
            (meta, state)
        }
        Entry::Link(link) => {
            let state = match link.status {
                LinkStatus::Loading => FileState::Loading,
                LinkStatus::Missing => FileState::Missing,
                LinkStatus::NotLoaded => FileState::NotLoaded,
            };
            (link_meta(link), state)
        }
    };
    let folder = entry
        .path()
        .and_then(folder_name)
        .unwrap_or_else(|| t!("sidebar.not_saved").to_string());
    FileItem {
        id: entry.id(),
        name: entry.name(),
        place: format!("{folder}, {meta}"),
        state,
        active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenkai_grid::ViewState;
    use zenkai_types::SheetId;

    use crate::spaces::SpaceId;

    fn link(status: LinkStatus) -> Link {
        Link {
            id: WorkbookId(1),
            space: SpaceId(0),
            untitled: 0,
            path: Some("C:\\data\\q3\\sales.xlsx".into()),
            recovery: None,
            dirty: false,
            read_only: false,
            unsupported: Vec::new(),
            sheet: SheetId(0),
            view: ViewState::default(),
            size: None,
            status,
            recovery_lost: false,
            origin: None,
        }
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(human_size(1), "1 KB");
        assert_eq!(human_size(1500), "2 KB");
        assert_eq!(human_size(1_572_864), "1.5 MB");
        assert_eq!(human_size(40 * 1024 * 1024), "40 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn a_not_loaded_link_says_so_with_its_size_and_a_hollow_state() {
        let mut waiting = link(LinkStatus::NotLoaded);
        waiting.size = Some(2 * 1024 * 1024);
        let item = describe(Entry::Link(&waiting), false);
        assert_eq!(item.state, FileState::NotLoaded);
        assert_eq!(item.place, "q3, not loaded, 2.0 MB");
        assert_eq!(item.name, "sales.xlsx");
    }

    #[test]
    fn a_missing_file_says_not_found_in_words() {
        let item = describe(Entry::Link(&link(LinkStatus::Missing)), true);
        assert_eq!(item.state, FileState::Missing);
        assert_eq!(item.place, "q3, not found");
        assert!(item.active);
    }

    #[test]
    fn a_loading_link_and_unsaved_work_are_both_named() {
        let mut loading = link(LinkStatus::Loading);
        loading.dirty = true;
        let item = describe(Entry::Link(&loading), false);
        assert_eq!(item.state, FileState::Loading);
        assert_eq!(item.place, "q3, unsaved changes, loading…");
    }

    #[test]
    fn an_untitled_link_has_no_folder() {
        let mut untitled = link(LinkStatus::NotLoaded);
        untitled.path = None;
        untitled.untitled = 3;
        let item = describe(Entry::Link(&untitled), false);
        assert_eq!(item.name, "Book3");
        assert_eq!(item.place, "not saved, not loaded");
    }
}
