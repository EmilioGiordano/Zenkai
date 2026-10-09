use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use zenkai_engine::Unsupported;
use zenkai_grid::{Selection, ViewState};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId, WorkbookId};

use crate::entry::{Link, LinkStatus};
use crate::space_appearance::{SpaceAppearance, SpaceOverride};
use crate::spaces::{SpaceColor, SpaceId};

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
    pub spaces: Vec<SpaceRecord>,
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
}

impl Session {
    pub fn new(sidebar_visible: bool, spaces: Vec<SpaceRecord>) -> Session {
        Session {
            version: VERSION,
            sidebar_visible,
            space_appearance: SpaceAppearance::default(),
            spaces,
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

const ALL_UNSUPPORTED: [Unsupported; 11] = [
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

// Only a plain `autosave-*.xlsx` name is trusted, so a tampered session cannot point the
// recovery machinery (which moves and deletes these files) at any other file.
fn recovery_name(name: &str) -> Option<&str> {
    let mut parts = Path::new(name).components();
    let Some(Component::Normal(only)) = parts.next() else {
        return None;
    };
    let plain = parts.next().is_none() && only.to_str() == Some(name);
    (plain && name.starts_with("autosave-") && name.ends_with(".xlsx") && !name.contains(':'))
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
    let aside = file.with_extension("json.unreadable");
    if let Err(error) = std::fs::rename(file, &aside) {
        tracing::warn!(%error, "could not move the unreadable session file aside");
    }
    Loaded::Unreadable
}

// Written next to the file and renamed over it, so a crash never leaves half a session.
pub fn save(directory: &Path, session: &Session) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let file = file_in(directory);
    let temp = file.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    let mut written = std::fs::File::create(&temp)?;
    written.write_all(&bytes)?;
    written.sync_all()?;
    drop(written);
    std::fs::rename(&temp, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> FileRecord {
        FileRecord {
            path: Some("C:\\data\\q3.xlsx".to_string()),
            recovery: Some("autosave-1-2-3.xlsx".to_string()),
            untitled: 0,
            dirty: true,
            read_only: false,
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
