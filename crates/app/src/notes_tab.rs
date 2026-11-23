use zenkai_i18n::t;
use zenkai_types::{SheetId, WorkbookId};

use crate::entry::{Link, LinkStatus};
use crate::spaces::SpaceId;
use crate::text_document::{DocumentKind, ViewMode};

// The What's new tab as the list sees it. The notes themselves live in the view entity and
// come from the update check, so there is nothing here to save, recover or restore.
pub struct NotesTab {
    pub id: WorkbookId,
    pub space: SpaceId,
}

impl NotesTab {
    pub fn name(&self) -> String {
        t!("whats_new.name").to_string()
    }

    pub fn to_link(&self) -> Link {
        Link {
            id: self.id,
            space: self.space,
            untitled: 0,
            path: None,
            recovery: None,
            dirty: false,
            read_only: true,
            unsupported: Vec::new(),
            sheet: SheetId(0),
            view: zenkai_grid::ViewState::default(),
            size: None,
            status: LinkStatus::NotLoaded,
            recovery_lost: false,
            origin: None,
            kind: DocumentKind::WhatsNew,
            mode: ViewMode::default(),
            csv: None,
            base: None,
            pdf: Default::default(),
            text_zoom: Default::default(),
            rested: None,
        }
    }
}
