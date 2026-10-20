use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zenkai_agent::preferences::MarkdownOpensIn;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use crate::entry::{Link, LinkStatus};
use crate::spaces::SpaceId;
use crate::text_file::{Format, Stamp};

pub const MARKDOWN_EXTENSIONS: [&str; 2] = ["md", "markdown"];

// What a tab holds. Opening, the session and the sidebar dispatch on it; a later PDF or docx
// viewer adds a variant here and reuses the rest of the plumbing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    #[default]
    Workbook,
    Markdown,
}

impl DocumentKind {
    pub fn of(path: &Path) -> DocumentKind {
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
        match extension.as_deref() {
            Some(extension) if MARKDOWN_EXTENSIONS.contains(&extension) => DocumentKind::Markdown,
            _ => DocumentKind::Workbook,
        }
    }

    pub fn is_text(self) -> bool {
        self == DocumentKind::Markdown
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Preview,
    Split,
    Source,
}

impl From<MarkdownOpensIn> for ViewMode {
    fn from(choice: MarkdownOpensIn) -> ViewMode {
        match choice {
            MarkdownOpensIn::Preview => ViewMode::Preview,
            MarkdownOpensIn::Split => ViewMode::Split,
        }
    }
}

impl ViewMode {
    pub fn next(self) -> ViewMode {
        match self {
            ViewMode::Preview => ViewMode::Split,
            ViewMode::Split => ViewMode::Source,
            ViewMode::Source => ViewMode::Preview,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Preview => t!("markdown.mode.preview"),
            ViewMode::Split => t!("markdown.mode.split"),
            ViewMode::Source => t!("markdown.mode.source"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiskNotice {
    #[default]
    None,
    ChangedWithEdits,
    Deleted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Writing {
    #[default]
    Idle,
    Saving,
    Autosaving,
}

// Save As keeps a Markdown name as typed and gives anything else the `.md` extension, so the
// file opens as Markdown again.
pub fn markdown_target(chosen: &Path) -> PathBuf {
    if DocumentKind::of(chosen).is_text() {
        return chosen.to_path_buf();
    }
    let mut name = chosen.as_os_str().to_owned();
    name.push(".md");
    PathBuf::from(name)
}

pub fn text_name(path: Option<&Path>, untitled: u32) -> String {
    match path {
        Some(path) => path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        None => t!("markdown.untitled", number = untitled),
    }
}

// The state of a text tab that the list needs without a window. The text itself lives in the
// editor entity of the workspace, which is the only copy that can change.
pub struct TextDocument {
    pub id: WorkbookId,
    pub space: SpaceId,
    untitled: u32,
    pub path: Option<PathBuf>,
    pub kind: DocumentKind,
    pub mode: ViewMode,
    pub dirty: bool,
    pub format: Format,
    pub stamp: Option<Stamp>,
    // Bumped whenever `stamp` is set, so a poll of the file that started earlier is ignored.
    pub stamp_epoch: u64,
    pub notice: DiskNotice,
    pub writing: Writing,
    edit_count: u64,
    last_edited: Option<Instant>,
    autosaved_at: Option<u64>,
    autosave_attempted: Instant,
    recovery_stale_since: Option<Instant>,
    recovery_on_disk: bool,
}

impl TextDocument {
    pub fn new(
        id: WorkbookId,
        space: SpaceId,
        untitled: u32,
        path: Option<PathBuf>,
        kind: DocumentKind,
        mode: ViewMode,
    ) -> TextDocument {
        TextDocument {
            id,
            space,
            untitled,
            path,
            kind,
            mode,
            dirty: false,
            format: Format::default(),
            stamp: None,
            stamp_epoch: 0,
            notice: DiskNotice::None,
            writing: Writing::Idle,
            edit_count: 0,
            last_edited: None,
            autosaved_at: None,
            autosave_attempted: Instant::now(),
            recovery_stale_since: None,
            recovery_on_disk: false,
        }
    }

    pub fn untitled(&self) -> u32 {
        self.untitled
    }

    pub fn name(&self) -> String {
        text_name(self.path.as_deref(), self.untitled)
    }

    pub fn edit_count(&self) -> u64 {
        self.edit_count
    }

    pub fn edited(&mut self) {
        // Clean work owes no recovery copy, so a stale instant from before it must not make
        // the first new edit overdue.
        if !self.needs_recovery() {
            self.recovery_stale_since = None;
        }
        self.edit_count += 1;
        self.last_edited = Some(Instant::now());
        self.recovery_stale_since.get_or_insert_with(Instant::now);
        self.dirty = true;
    }

    // Work that exists only in memory: unsaved edits, or a text never saved that was typed in.
    pub fn needs_recovery(&self) -> bool {
        self.dirty || (self.path.is_none() && self.edit_count > 0)
    }

    // `edits` is the edit count the written text was taken at.
    pub fn set_stamp(&mut self, stamp: Option<Stamp>) {
        self.stamp = stamp;
        self.stamp_epoch += 1;
    }

    pub fn saved(&mut self, edits: u64, stamp: Option<Stamp>) {
        self.set_stamp(stamp);
        self.dirty = self.edit_count != edits;
        self.notice = DiskNotice::None;
    }

    // The editor holds the file's text again, with the history dropped.
    pub fn reloaded(&mut self, stamp: Option<Stamp>) {
        self.set_stamp(stamp);
        self.dirty = false;
        self.notice = DiskNotice::None;
        self.edit_count += 1;
        self.autosaved_at = None;
    }

    pub fn since_last_edit(&self) -> Option<Duration> {
        self.last_edited.map(|edited| edited.elapsed())
    }

    pub fn since_last_autosave_attempt(&self) -> Duration {
        self.autosave_attempted.elapsed()
    }

    pub fn mark_autosave_attempt(&mut self) {
        self.autosave_attempted = Instant::now();
    }

    pub fn recovery_is_current(&self) -> bool {
        self.autosaved_at == Some(self.edit_count)
    }

    pub fn recovery_written(&mut self, edits: u64) {
        self.autosaved_at = Some(edits);
        self.recovery_on_disk = true;
        self.recovery_stale_since = (edits != self.edit_count).then(Instant::now);
    }

    pub fn recovery_removed(&mut self) {
        self.autosaved_at = None;
        self.recovery_on_disk = false;
        self.recovery_stale_since = None;
    }

    pub fn recovery_stale_for(&self) -> Option<Duration> {
        self.recovery_stale_since.map(|since| since.elapsed())
    }

    pub fn has_recovery_copy(&self) -> bool {
        self.recovery_on_disk
    }

    pub fn mark_recovery_adopted(&mut self) {
        self.recovery_on_disk = true;
    }

    pub fn to_link(&self) -> Link {
        Link {
            id: self.id,
            space: self.space,
            untitled: self.untitled,
            path: self.path.clone(),
            recovery: None,
            dirty: self.dirty,
            read_only: false,
            unsupported: Vec::new(),
            sheet: zenkai_types::SheetId(0),
            view: zenkai_grid::ViewState::default(),
            size: None,
            status: LinkStatus::NotLoaded,
            recovery_lost: false,
            origin: None,
            kind: self.kind,
            mode: self.mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(path: Option<&str>) -> TextDocument {
        TextDocument::new(
            WorkbookId(1),
            SpaceId(0),
            2,
            path.map(PathBuf::from),
            DocumentKind::Markdown,
            ViewMode::Preview,
        )
    }

    #[test]
    fn the_kind_comes_from_the_extension_in_any_case() {
        assert_eq!(
            DocumentKind::of(Path::new("a/notes.md")),
            DocumentKind::Markdown
        );
        assert_eq!(
            DocumentKind::of(Path::new("NOTES.MARKDOWN")),
            DocumentKind::Markdown
        );
        assert_eq!(
            DocumentKind::of(Path::new("a.xlsx")),
            DocumentKind::Workbook
        );
        assert_eq!(DocumentKind::of(Path::new("md")), DocumentKind::Workbook);
        assert_eq!(
            DocumentKind::of(Path::new("a.md.xlsx")),
            DocumentKind::Workbook
        );
    }

    #[test]
    fn save_as_keeps_a_markdown_extension_and_adds_one_otherwise() {
        assert_eq!(
            markdown_target(Path::new("a/notes.md")),
            Path::new("a/notes.md")
        );
        assert_eq!(
            markdown_target(Path::new("a/NOTES.MARKDOWN")),
            Path::new("a/NOTES.MARKDOWN")
        );
        assert_eq!(
            markdown_target(Path::new("a/notes")),
            Path::new("a/notes.md")
        );
        assert_eq!(
            markdown_target(Path::new("a/notes.txt")),
            Path::new("a/notes.txt.md")
        );
    }

    #[test]
    fn the_mode_cycles_preview_split_source() {
        assert_eq!(ViewMode::Preview.next(), ViewMode::Split);
        assert_eq!(ViewMode::Split.next(), ViewMode::Source);
        assert_eq!(ViewMode::Source.next(), ViewMode::Preview);
    }

    #[test]
    fn a_text_with_a_path_is_named_after_its_file_and_one_without_is_numbered() {
        assert_eq!(document(Some("dir/report.md")).name(), "report.md");
        assert_eq!(document(None).name(), "Untitled 2.md");
    }

    #[test]
    fn an_edit_makes_the_document_dirty_and_a_save_of_the_same_edit_count_clears_it() {
        let mut text = document(Some("a.md"));
        assert!(!text.needs_recovery());
        text.edited();
        assert!(text.dirty && text.needs_recovery());
        text.saved(1, None);
        assert!(!text.dirty);
    }

    #[test]
    fn an_edit_made_while_saving_keeps_the_document_dirty() {
        let mut text = document(Some("a.md"));
        text.edited();
        let taken_at = text.edit_count();
        text.edited();
        text.saved(taken_at, None);
        assert!(text.dirty);
    }

    #[test]
    fn a_text_never_saved_needs_recovery_once_typed_in() {
        let mut text = document(None);
        assert!(!text.needs_recovery());
        text.edited();
        assert!(text.needs_recovery());
    }

    #[test]
    fn the_recovery_copy_is_current_until_the_next_edit() {
        let mut text = document(Some("a.md"));
        text.edited();
        text.recovery_written(text.edit_count());
        assert!(text.recovery_is_current() && text.has_recovery_copy());
        text.edited();
        assert!(!text.recovery_is_current());
        assert!(text.recovery_stale_for().is_some());
    }

    #[test]
    fn reloading_clears_the_dirty_state_and_the_notice() {
        let mut text = document(Some("a.md"));
        text.edited();
        text.notice = DiskNotice::ChangedWithEdits;
        text.reloaded(None);
        assert!(!text.dirty);
        assert_eq!(text.notice, DiskNotice::None);
    }

    #[test]
    fn the_link_of_a_text_keeps_its_kind_mode_and_path() {
        let mut text = document(Some("a.md"));
        text.mode = ViewMode::Source;
        let link = text.to_link();
        assert_eq!(link.kind, DocumentKind::Markdown);
        assert_eq!(link.mode, ViewMode::Source);
        assert_eq!(link.path.as_deref(), Some(Path::new("a.md")));
    }
}
