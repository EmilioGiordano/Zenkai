use std::path::PathBuf;

use zenkai_types::{SheetId, WorkbookId};

use crate::entry::{Link, LinkStatus};
use crate::pdf::PdfPosition;
use crate::spaces::SpaceId;
use crate::text_document::{DocumentKind, ViewMode};

// A PDF tab as the list sees it. The document itself lives in the reader thread and the view
// entity; this keeps what outlives them: where the tab is and where the user was reading.
pub struct PdfTab {
    pub id: WorkbookId,
    pub space: SpaceId,
    pub path: PathBuf,
    pub position: PdfPosition,
}

impl PdfTab {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    }

    pub fn to_link(&self) -> Link {
        Link {
            id: self.id,
            space: self.space,
            untitled: 0,
            path: Some(self.path.clone()),
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
            kind: DocumentKind::Pdf,
            mode: ViewMode::default(),
            base: None,
            pdf: self.position,
        }
    }
}
