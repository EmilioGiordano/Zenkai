use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use zenkai_engine::Unsupported;
use zenkai_grid::{Selection, ViewState};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId, WorkbookId};

use crate::entry::{FileSeen, Link, LinkStatus, Rested};
use crate::panel_width::PanelWidths;
use crate::pdf::PdfPosition;
use crate::recovery;
use crate::space_appearance::{SpaceAppearance, SpaceOverride};
use crate::spaces::{SpaceColor, SpaceId};
use crate::text_document::{DocumentKind, ViewMode};
use crate::text_file::StampRecord;
use crate::text_zoom::TextZoom;

const VERSION: u32 = 1;
const FILE_NAME: &str = "session.json";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SPACES: usize = 200;
const MAX_FILES: usize = 5000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub sidebar_visible: bool,
    #[serde(default)]
    pub space_appearance: SpaceAppearance,
    #[serde(default)]
    pub panel_widths: PanelWidths,
    // Tabs reopened as last saved because the previous close discarded their changes.
    #[serde(default)]
    pub reopened_as_saved: u32,
    #[serde(default)]
    pub pdf_places: Vec<PdfPlace>,
    pub spaces: Vec<SpaceRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PdfPlace {
    pub path: String,
    pub position: PdfPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceRecord {
    pub name: String,
    pub collapsed: bool,
    #[serde(default)]
    pub color: SpaceColor,
    #[serde(default)]
    pub appearance: SpaceOverride,
    pub files: Vec<FileRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: Option<String>,
    // File name inside the recovery folder.
    pub recovery: Option<String>,
    pub untitled: u32,
    pub dirty: bool,
    pub read_only: bool,
    #[serde(default)]
    pub kind: DocumentKind,
    #[serde(default)]
    pub mode: ViewMode,
    #[serde(default)]
    pub base: Option<StampRecord>,
    #[serde(default)]
    pub file_modified: Option<SystemTime>,
    #[serde(default)]
    pub pdf: PdfPosition,
    #[serde(default)]
    pub text_zoom: TextZoom,
    pub unsupported: Vec<String>,
    pub active: bool,
    pub sheet: u32,
    pub view: ViewRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewRecord {
    pub active_row: u32,
    pub active_col: u16,
    pub corner_row: u32,
    pub corner_col: u16,
    pub top: u32,
    pub left: u16,
}

#[derive(Debug)]
pub enum Loaded {
    Absent,
    Restored(Session),
    // Moved aside so the next close does not overwrite it with a blank one.
    Unreadable,
    // Could not be moved aside, so it stays and no save may replace it.
    KeptInPlace,
}

impl Session {
    pub fn new(sidebar_visible: bool, spaces: Vec<SpaceRecord>) -> Session {
        Session {
            version: VERSION,
            sidebar_visible,
            space_appearance: SpaceAppearance::default(),
            panel_widths: PanelWidths::default(),
            reopened_as_saved: 0,
            pdf_places: Vec::new(),
            spaces,
        }
    }

    pub fn with_panel_widths(self, panel_widths: PanelWidths) -> Session {
        Session {
            panel_widths,
            ..self
        }
    }

    pub fn with_pdf_places(self, pdf_places: Vec<PdfPlace>) -> Session {
        Session { pdf_places, ..self }
    }

    pub fn with_reopened_as_saved(self, reopened_as_saved: u32) -> Session {
        Session {
            reopened_as_saved,
            ..self
        }
    }

    pub fn with_space_appearance(self, space_appearance: SpaceAppearance) -> Session {
        Session {
            space_appearance,
            ..self
        }
    }

    // A session past these limits was not written by this program.
    fn within_limits(&self) -> bool {
        self.spaces.len() <= MAX_SPACES
            && self
                .spaces
                .iter()
                .map(|space| space.files.len())
                .sum::<usize>()
                <= MAX_FILES
    }

    // What a start with "Restore last session" off keeps: the spaces and the links to files on
    // disk, but nothing to reopen and no unsaved work (its recovery copies are offered instead).
    pub fn without_unsaved_work(mut self) -> Session {
        self.reopened_as_saved = 0;
        for space in &mut self.spaces {
            space.files.retain(|file| file.path.is_some());
            for file in &mut space.files {
                file.recovery = None;
                file.dirty = false;
                file.active = false;
            }
        }
        self
    }

    pub fn recovery_files(&self) -> Vec<&str> {
        self.spaces
            .iter()
            .flat_map(|space| &space.files)
            .filter_map(|file| file.recovery.as_deref())
            .collect()
    }
}

const ALL_UNSUPPORTED: [Unsupported; 13] = [
    Unsupported::Charts,
    Unsupported::Images,
    Unsupported::PivotTables,
    Unsupported::Macros,
    Unsupported::Comments,
    Unsupported::Tables,
    Unsupported::Hyperlinks,
    Unsupported::DataValidation,
    Unsupported::ExternalLinks,
    Unsupported::AutoFilter,
    Unsupported::SheetProtection,
    Unsupported::Outline,
    Unsupported::PageBreaks,
];

fn unsupported_code(unsupported: Unsupported) -> &'static str {
    match unsupported {
        Unsupported::Charts => "charts",
        Unsupported::Images => "images",
        Unsupported::PivotTables => "pivot-tables",
        Unsupported::Macros => "macros",
        Unsupported::Comments => "comments",
        Unsupported::Tables => "tables",
        Unsupported::Hyperlinks => "hyperlinks",
        Unsupported::DataValidation => "data-validation",
        Unsupported::ExternalLinks => "external-links",
        Unsupported::AutoFilter => "auto-filter",
        Unsupported::SheetProtection => "sheet-protection",
        Unsupported::Outline => "outline",
        Unsupported::PageBreaks => "page-breaks",
    }
}

fn parse_unsupported(code: &str) -> Option<Unsupported> {
    ALL_UNSUPPORTED
        .into_iter()
        .find(|unsupported| unsupported_code(*unsupported) == code)
}

impl ViewRecord {
    fn of(view: ViewState) -> ViewRecord {
        ViewRecord {
            active_row: view.selection.active.row.get(),
            active_col: view.selection.active.col.get(),
            corner_row: view.selection.corner.row.get(),
            corner_col: view.selection.corner.col.get(),
            top: view.top.get(),
            left: view.left.get(),
        }
    }

    fn view(self) -> ViewState {
        let pos = |row: u32, col: u16| {
            CellPos::new(
                RowIdx::clamped(i64::from(row)),
                ColIdx::clamped(i64::from(col)),
            )
        };
        ViewState {
            selection: Selection {
                active: pos(self.active_row, self.active_col),
                corner: pos(self.corner_row, self.corner_col),
            },
            top: RowIdx::clamped(i64::from(self.top)),
            left: ColIdx::clamped(i64::from(self.left)),
        }
    }
}

pub fn record_of(link: &Link, active: bool) -> FileRecord {
    FileRecord {
        path: link
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        recovery: link
            .recovery
            .as_ref()
            .and_then(|file| file.file_name())
            .map(|name| name.to_string_lossy().into_owned()),
        untitled: link.untitled,
        dirty: link.dirty,
        read_only: link.read_only,
        kind: link.kind,
        mode: link.mode,
        base: link.base,
        file_modified: link
            .path
            .as_ref()
            .zip(link.recovery.as_ref())
            .and(link.rested.as_ref())
            .map(|rested| {
                rested
                    .file
                    .stamp
                    .map_or(rested.file.at, |stamp| stamp.modified())
            }),
        pdf: link.pdf,
        text_zoom: link.text_zoom,
        unsupported: link
            .unsupported
            .iter()
            .map(|unsupported| unsupported_code(*unsupported).to_string())
            .collect(),
        active,
        sheet: link.sheet.0,
        view: ViewRecord::of(link.view),
    }
}

// Only a plain `autosave-*` name with a recovery extension is trusted, so a tampered session cannot point the
// recovery machinery (which moves and deletes these files) at any other file.
fn recovery_name(name: &str) -> Option<&str> {
    let mut parts = Path::new(name).components();
    let Some(Component::Normal(only)) = parts.next() else {
        return None;
    };
    let plain = parts.next().is_none() && only.to_str() == Some(name);
    (plain
        && name.starts_with("autosave-")
        && recovery::has_recovery_extension(name)
        && !name.contains(':'))
    .then_some(name)
}

// A guard code this version does not know forces Save As, the safe side of the doubt.
pub fn link_of(
    record: &FileRecord,
    id: WorkbookId,
    space: SpaceId,
    recovery_directory: Option<&Path>,
) -> Link {
    let known: Vec<Unsupported> = record
        .unsupported
        .iter()
        .filter_map(|code| parse_unsupported(code))
        .collect();
    let recovery = record
        .recovery
        .as_deref()
        .and_then(recovery_name)
        .zip(recovery_directory)
        .map(|(name, directory)| directory.join(name));
    Link {
        id,
        space,
        untitled: record.untitled,
        path: record.path.as_ref().map(PathBuf::from),
        recovery_lost: record.recovery.is_some() && recovery.is_none(),
        recovery,
        dirty: record.dirty,
        read_only: record.read_only || known.len() != record.unsupported.len(),
        unsupported: known,
        sheet: SheetId(record.sheet),
        view: record.view.view(),
        size: None,
        status: LinkStatus::NotLoaded,
        origin: None,
        kind: record.kind,
        mode: record.mode,
        base: record.base,
        pdf: record.pdf,
        text_zoom: record.text_zoom,
        rested: record.file_modified.map(|modified| {
            Box::new(Rested {
                file: FileSeen {
                    stamp: None,
                    at: modified,
                },
                hibernation: None,
            })
        }),
    }
}

pub fn file_in(directory: &Path) -> PathBuf {
    directory.join(FILE_NAME)
}

pub fn load(directory: &Path) -> Loaded {
    let file = file_in(directory);
    let mut text = String::new();
    let read = std::fs::File::open(&file)
        .and_then(|opened| opened.take(MAX_BYTES + 1).read_to_string(&mut text));
    match read {
        Ok(length) if length as u64 > MAX_BYTES => {
            tracing::warn!("the session file is too large");
            return set_aside(&file);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Loaded::Absent,
        Err(error) => {
            tracing::warn!(%error, "could not read the session file");
            return set_aside(&file);
        }
    }
    match serde_json::from_str::<Session>(&text) {
        Ok(session) if session.version == VERSION && session.within_limits() => {
            Loaded::Restored(session)
        }
        Ok(session) => {
            tracing::warn!(version = session.version, "unknown session file version");
            set_aside(&file)
        }
        Err(error) => {
            tracing::warn!(%error, "the session file is corrupt");
            set_aside(&file)
        }
    }
}

fn set_aside(file: &Path) -> Loaded {
    let moved = zenkai_types::aside::free_aside_name(file, "unreadable")
        .ok_or_else(|| std::io::Error::other("no free name for the .unreadable copy"))
        .and_then(|aside| std::fs::rename(file, aside));
    match moved {
        Ok(()) => Loaded::Unreadable,
        Err(error) => {
            tracing::warn!(%error, "could not move the unreadable session file aside");
            KEPT_IN_PLACE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(file.to_path_buf());
            Loaded::KeptInPlace
        }
    }
}

// Files that could not be set aside; a save would replace the only copy, so none is written.
static KEPT_IN_PLACE: std::sync::Mutex<std::collections::BTreeSet<PathBuf>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

// Written next to the file and renamed over it, so a crash never leaves half a session.
// Taken when a snapshot is made. Writes may run on any thread, so a write whose snapshot is
// older than one already on disk is skipped instead of overwriting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SaveOrder(u64);

static NEXT_ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static LAST_WRITTEN: std::sync::Mutex<std::collections::BTreeMap<PathBuf, SaveOrder>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

impl SaveOrder {
    pub fn next() -> SaveOrder {
        SaveOrder(NEXT_ORDER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(test)]
pub fn save(directory: &Path, session: &Session) -> std::io::Result<()> {
    save_in_order(directory, session, SaveOrder::next())
}

pub fn save_in_order(directory: &Path, session: &Session, order: SaveOrder) -> std::io::Result<()> {
    let mut last = LAST_WRITTEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let file = file_in(directory);
    if KEPT_IN_PLACE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&file)
    {
        return Err(std::io::Error::other(
            "the unreadable session file could not be set aside and is kept",
        ));
    }
    if last.get(&file).is_some_and(|written| order < *written) {
        return Ok(());
    }
    std::fs::create_dir_all(directory)?;
    let temp = file.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    let mut written = std::fs::File::create(&temp)?;
    written.write_all(&bytes)?;
    written.sync_all()?;
    drop(written);
    std::fs::rename(&temp, &file)?;
    last.insert(file, order);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel_width::{Panel, Shown};

    fn record() -> FileRecord {
        FileRecord {
            path: Some("C:\\data\\q3.xlsx".to_string()),
            recovery: Some("autosave-1-2-3.xlsx".to_string()),
            untitled: 0,
            dirty: true,
            read_only: false,
            kind: Default::default(),
            mode: Default::default(),
            base: None,
            file_modified: None,
            pdf: Default::default(),
            text_zoom: Default::default(),
            unsupported: vec!["charts".to_string(), "macros".to_string()],
            active: true,
            sheet: 2,
            view: ViewRecord {
                active_row: 4,
                active_col: 3,
                corner_row: 6,
                corner_col: 5,
                top: 100,
                left: 2,
            },
        }
    }

    fn session() -> Session {
        Session::new(
            true,
            vec![SpaceRecord {
                name: "Q3 close".to_string(),
                collapsed: false,
                color: Default::default(),
                appearance: Default::default(),
                files: vec![record()],
            }],
        )
    }

    #[test]
    fn the_places_in_pdfs_survive_the_session_round_trip() {
        let place = PdfPlace {
            path: "a.pdf".to_string(),
            position: PdfPosition {
                page: 12,
                zoom: crate::pdf::layout::ZoomMode::Percent(150),
            },
        };
        let kept = session().with_pdf_places(vec![place.clone()]);
        let text = serde_json::to_string(&kept).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back.pdf_places, vec![place]);
    }

    #[test]
    fn a_session_without_pdf_places_still_loads() {
        let mut value = serde_json::to_value(session()).unwrap();
        value.as_object_mut().unwrap().remove("pdf_places");
        let back: Session = serde_json::from_value(value).unwrap();
        assert!(back.pdf_places.is_empty());
    }

    #[test]
    fn panel_widths_survive_the_session_round_trip() {
        let widths = PanelWidths::default().resized(
            Panel::Chat,
            600.0,
            3000.0,
            Shown {
                sidebar: true,
                chat: true,
            },
        );
        let kept = session().with_panel_widths(widths);
        let text = serde_json::to_string(&kept).unwrap();
        let back = serde_json::from_str::<Session>(&text).unwrap();
        assert_eq!(back.panel_widths, widths);
        assert_eq!(back.panel_widths.width_of(Panel::Chat), 600.0);
    }

    #[test]
    fn the_pdf_panel_width_survives_the_session_round_trip() {
        let widths = PanelWidths::default().with_pdf_width(300.0);
        let text = serde_json::to_string(&session().with_panel_widths(widths)).unwrap();
        let back = serde_json::from_str::<Session>(&text).unwrap();
        assert_eq!(back.panel_widths.pdf_width(), 300.0);
    }

    #[test]
    fn a_session_without_panel_widths_gets_the_defaults() {
        let mut value = serde_json::to_value(session()).unwrap();
        value.as_object_mut().unwrap().remove("panel_widths");
        let back = serde_json::from_value::<Session>(value).unwrap();
        assert_eq!(back.panel_widths, PanelWidths::default());
    }

    #[test]
    fn a_session_with_unusable_panel_widths_still_loads_and_falls_back() {
        let mut value = serde_json::to_value(session()).unwrap();
        value["panel_widths"] = serde_json::json!({"sidebar": -4.0, "chat": "wide"});
        let back = serde_json::from_value::<Session>(value).unwrap();
        assert_eq!(back.panel_widths.validated(), PanelWidths::default());
    }

    #[test]
    fn a_session_survives_the_json_round_trip() {
        let text = serde_json::to_string(&session()).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&text).unwrap(), session());
    }

    #[test]
    fn the_appearance_of_spaces_survives_the_json_round_trip() {
        use crate::space_appearance::{
            ApplyTo, Custom, Intensity, Look, NewSpaceColor, Opacity, Rgba, SpaceStyle,
        };
        let mut custom = session();
        custom.spaces[0].color = SpaceColor::Pink;
        custom.spaces[0].appearance = SpaceOverride::Custom(Custom {
            look: Look {
                style: SpaceStyle::FullTint,
                intensity: Intensity::from(24),
                apply_to: ApplyTo::WorkbooksOnly,
            },
            color: Some(Rgba::new(0x336699, Opacity::from(80))),
        });
        let custom = custom.with_space_appearance(SpaceAppearance {
            look: Look {
                style: SpaceStyle::Border,
                ..Look::default()
            },
            new_space_color: NewSpaceColor::None,
        });
        let text = serde_json::to_string(&custom).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&text).unwrap(), custom);
    }

    #[test]
    fn a_session_written_before_the_appearance_options_still_loads() {
        let old = r#"{"version":1,"sidebar_visible":true,"spaces":[
            {"name":"Q3","collapsed":false,"color":"teal","files":[]},
            {"name":"Plain","collapsed":true,"files":[]}]}"#;
        let loaded: Session = serde_json::from_str(old).unwrap();
        assert_eq!(loaded.space_appearance, SpaceAppearance::default());
        assert_eq!(loaded.spaces[0].color, SpaceColor::Teal);
        assert_eq!(loaded.spaces[0].appearance, SpaceOverride::Default);
        assert_eq!(loaded.spaces[1].color, SpaceColor::Default);
    }

    #[test]
    fn saving_and_loading_goes_through_the_file() {
        let directory = tempfile::tempdir().unwrap();
        save(directory.path(), &session()).unwrap();
        let Loaded::Restored(loaded) = load(directory.path()) else {
            panic!("the session was not restored");
        };
        assert_eq!(loaded, session());
        assert!(
            !file_in(directory.path())
                .with_extension("json.tmp")
                .exists()
        );
    }

    #[test]
    fn saves_from_several_threads_never_trip_over_the_temp_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_path_buf();
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || save(&path, &session()))
            })
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        assert!(matches!(load(&path), Loaded::Restored(_)));
    }

    #[test]
    fn an_older_snapshot_never_overwrites_a_newer_one() {
        let directory = tempfile::tempdir().unwrap();
        let older = SaveOrder::next();
        let newer = SaveOrder::next();
        let mut kept = session();
        kept.sidebar_visible = !kept.sidebar_visible;
        save_in_order(directory.path(), &kept, newer).unwrap();
        save_in_order(directory.path(), &session(), older).unwrap();
        let Loaded::Restored(loaded) = load(directory.path()) else {
            panic!("the session was not restored");
        };
        assert_eq!(loaded, kept);
    }

    #[test]
    fn interleaved_orders_for_different_files_both_land() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let older = SaveOrder::next();
        let newer = SaveOrder::next();
        save_in_order(first.path(), &session(), newer).unwrap();
        save_in_order(second.path(), &session(), older).unwrap();
        assert!(matches!(load(first.path()), Loaded::Restored(_)));
        assert!(matches!(load(second.path()), Loaded::Restored(_)));
    }

    #[test]
    fn a_missing_file_is_absent() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(load(directory.path()), Loaded::Absent));
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_not_overwritten_or_lost() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(file_in(directory.path()), "{ not json").unwrap();
        assert!(matches!(load(directory.path()), Loaded::Unreadable));
        assert!(!file_in(directory.path()).exists());
        assert!(
            file_in(directory.path())
                .with_extension("json.unreadable")
                .exists()
        );
        assert!(matches!(load(directory.path()), Loaded::Absent));
    }

    #[test]
    fn a_second_corrupt_file_keeps_the_first_moved_aside() {
        let directory = tempfile::tempdir().unwrap();
        let file = file_in(directory.path());
        std::fs::write(&file, "{ first").unwrap();
        load(directory.path());
        std::fs::write(&file, "{ second").unwrap();
        load(directory.path());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("session.json.unreadable")).unwrap(),
            "{ first"
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("session.json.1.unreadable")).unwrap(),
            "{ second"
        );
    }

    #[test]
    fn a_corrupt_file_that_cannot_be_moved_aside_is_never_overwritten_by_a_save() {
        let directory = tempfile::tempdir().unwrap();
        let file = file_in(directory.path());
        std::fs::write(&file, "{ damaged").unwrap();
        for attempt in 0..zenkai_types::aside::ASIDE_ATTEMPTS {
            let name = match attempt {
                0 => "session.json.unreadable".to_string(),
                _ => format!("session.json.{attempt}.unreadable"),
            };
            std::fs::write(directory.path().join(name), "earlier").unwrap();
        }
        assert!(matches!(load(directory.path()), Loaded::KeptInPlace));
        assert!(save(directory.path(), &session()).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ damaged");
    }

    #[test]
    fn another_version_is_treated_as_unreadable() {
        let directory = tempfile::tempdir().unwrap();
        let mut future = session();
        future.version = VERSION + 1;
        std::fs::write(
            file_in(directory.path()),
            serde_json::to_string(&future).unwrap(),
        )
        .unwrap();
        assert!(matches!(load(directory.path()), Loaded::Unreadable));
    }

    #[test]
    fn a_record_becomes_a_link_and_back_without_losing_the_save_guard() {
        let directory = Path::new("recovery");
        let link = link_of(&record(), WorkbookId(7), SpaceId(1), Some(directory));
        assert_eq!(link.id, WorkbookId(7));
        assert_eq!(link.space, SpaceId(1));
        assert_eq!(link.recovery, Some(directory.join("autosave-1-2-3.xlsx")));
        assert_eq!(link.unsupported, [Unsupported::Charts, Unsupported::Macros]);
        assert!(link.dirty && !link.read_only);
        assert_eq!(link.sheet, SheetId(2));
        assert_eq!(record_of(&link, true), record());
    }

    #[test]
    fn an_unknown_guard_code_makes_the_workbook_read_only() {
        let mut unknown = record();
        unknown.unsupported.push("holograms".to_string());
        let link = link_of(&unknown, WorkbookId(0), SpaceId(0), None);
        assert!(link.read_only);
        assert_eq!(link.unsupported.len(), 2);
        assert_eq!(link.recovery, None);
    }

    #[test]
    fn every_guard_survives_its_code() {
        for unsupported in ALL_UNSUPPORTED {
            assert_eq!(
                parse_unsupported(unsupported_code(unsupported)),
                Some(unsupported)
            );
        }
    }

    #[test]
    fn the_recovery_files_a_session_points_to_are_listed() {
        assert_eq!(session().recovery_files(), ["autosave-1-2-3.xlsx"]);
    }

    #[test]
    fn a_tampered_recovery_name_is_never_joined_to_the_recovery_folder() {
        let directory = Path::new("recovery");
        for name in [
            r"C:\Users\victim\autosave-1.xlsx",
            r"..\autosave-1.xlsx",
            "../autosave-1.xlsx",
            r"\\host\share\autosave-1.xlsx",
            r"sub\autosave-1.xlsx",
            "autosave-1.xlsx:stream",
            "notes.xlsx",
            "autosave-1.docx",
            "",
        ] {
            let mut tampered = record();
            tampered.recovery = Some(name.to_string());
            let link = link_of(&tampered, WorkbookId(0), SpaceId(0), Some(directory));
            assert_eq!(link.recovery, None, "{name}");
            assert!(link.recovery_lost, "{name}");
        }
        let honest = link_of(&record(), WorkbookId(0), SpaceId(0), Some(directory));
        assert!(!honest.recovery_lost);
    }

    #[test]
    fn a_record_written_before_documents_had_a_kind_is_a_workbook_in_preview() {
        let mut value = serde_json::to_value(record()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("kind");
        object.remove("mode");
        object.remove("text_zoom");
        object.remove("file_modified");
        let old: FileRecord = serde_json::from_value(value).unwrap();
        assert_eq!(old.kind, DocumentKind::Workbook);
        assert_eq!(old.mode, ViewMode::Preview);
        assert_eq!(old.text_zoom, TextZoom::default());
        assert_eq!(old.base, None);
        assert_eq!(old.file_modified, None);
    }

    #[test]
    fn the_stamp_a_text_was_based_on_survives_the_session() {
        let stamp = crate::text_file::stamp_of(&std::env::temp_dir()).unwrap();
        let mut text = record();
        text.base = Some(stamp.record());
        let link = link_of(&text, WorkbookId(1), SpaceId(0), None);
        assert_eq!(link.base, text.base);
        let json = serde_json::to_string(&record_of(&link, true)).unwrap();
        let back: FileRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.base, text.base);
    }

    #[test]
    fn a_markdown_record_keeps_its_kind_and_mode_through_a_link_and_back() {
        let mut text = record();
        text.path = Some("C:\\notes\\plan.md".to_string());
        text.recovery = Some("autosave-1-2-3.md".to_string());
        text.kind = DocumentKind::Markdown;
        text.mode = ViewMode::Split;
        text.text_zoom = TextZoom::nearest(175);
        text.unsupported.clear();
        text.sheet = 0;
        let directory = Path::new("recovery");
        let link = link_of(&text, WorkbookId(4), SpaceId(0), Some(directory));
        assert_eq!(link.kind, DocumentKind::Markdown);
        assert_eq!(link.mode, ViewMode::Split);
        assert_eq!(link.text_zoom.percent(), 175);
        assert_eq!(link.recovery, Some(directory.join("autosave-1-2-3.md")));
        assert_eq!(record_of(&link, true), text);
    }

    #[test]
    fn a_recovery_name_with_the_text_extension_is_trusted_only_in_its_plain_form() {
        assert_eq!(
            recovery_name("autosave-1-2-3.md"),
            Some("autosave-1-2-3.md")
        );
        for name in [
            "autosave-1.md:stream",
            "notes.md",
            "..\\autosave-1.md",
            "sub/autosave-1.md",
            "autosave-1.markdown",
        ] {
            assert_eq!(recovery_name(name), None, "{name}");
        }
    }

    #[test]
    fn an_oversized_session_file_is_set_aside_unread() {
        let directory = tempfile::tempdir().unwrap();
        let padding = " ".repeat(usize::try_from(MAX_BYTES).unwrap() + 1);
        std::fs::write(file_in(directory.path()), padding).unwrap();
        assert!(matches!(load(directory.path()), Loaded::Unreadable));
    }

    #[test]
    fn a_session_with_absurd_counts_is_set_aside() {
        let directory = tempfile::tempdir().unwrap();
        let crowded = Session::new(
            false,
            (0..=MAX_SPACES)
                .map(|n| SpaceRecord {
                    name: n.to_string(),
                    collapsed: false,
                    color: Default::default(),
                    appearance: Default::default(),
                    files: Vec::new(),
                })
                .collect(),
        );
        save(directory.path(), &crowded).unwrap();
        assert!(matches!(load(directory.path()), Loaded::Unreadable));
    }
}
