use std::collections::HashMap;
use std::path::PathBuf;

use zenkai_engine::{Engine, EngineError, Unsupported, Workbook};
use zenkai_grid::GridCell;
use zenkai_types::{CellPos, Range, SheetId, SheetInfo};

pub type Edit = Box<dyn FnOnce(&mut Workbook) -> Result<(), EngineError> + Send>;

pub struct Document {
    workbook: Option<Workbook>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub sheet: SheetId,
    pub sheets: Vec<SheetInfo>,
    pub unsupported: Vec<Unsupported>,
    pending: Vec<Edit>,
}

impl Document {
    pub fn new(
        workbook: Workbook,
        path: Option<PathBuf>,
        unsupported: Vec<Unsupported>,
    ) -> Document {
        let sheets = workbook.sheets();
        Document {
            workbook: Some(workbook),
            path,
            dirty: false,
            sheet: SheetId(0),
            sheets,
            unsupported,
            pending: Vec::new(),
        }
    }

    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Book1".to_string(), |n| n.to_string_lossy().into_owned());
        let marker = if self.dirty { "• " } else { "" };
        format!("{marker}{name} - Zenkai")
    }

    pub fn workbook(&self) -> Option<&Workbook> {
        self.workbook.as_ref()
    }

    pub fn queue(&mut self, edit: Edit) {
        self.pending.push(edit);
    }

    pub fn take(&mut self) -> Option<Workbook> {
        self.workbook.take()
    }

    pub fn take_batch(&mut self) -> Option<(Workbook, Vec<Edit>)> {
        if self.pending.is_empty() {
            return None;
        }
        let workbook = self.workbook.take()?;
        Some((workbook, std::mem::take(&mut self.pending)))
    }

    pub fn restore(&mut self, workbook: Workbook) {
        self.sheets = workbook.sheets();
        let last = u32::try_from(self.sheets.len().saturating_sub(1)).unwrap_or(0);
        if self.sheet.0 > last {
            self.sheet = SheetId(last);
        }
        self.workbook = Some(workbook);
    }

    pub fn cells(&self, range: Range) -> HashMap<CellPos, GridCell> {
        let Some(workbook) = &self.workbook else {
            return HashMap::new();
        };
        let end = workbook.used_end(self.sheet);
        let clipped = Range::new(
            range.start,
            CellPos::new(
                range.end.row.min(end.row.offset(1)),
                range.end.col.min(end.col.offset(1)),
            ),
        );
        if clipped.start.row > clipped.end.row || clipped.start.col > clipped.end.col {
            return HashMap::new();
        }
        clipped
            .positions()
            .filter_map(|pos| {
                let view = workbook.cell(self.sheet, pos);
                let blank = view.text.is_empty()
                    && view.style.fill.is_none()
                    && !view.style.border_bottom
                    && !view.style.border_right;
                (!blank).then(|| {
                    (
                        pos,
                        GridCell {
                            text: view.text.into(),
                            kind: view.kind,
                            style: view.style,
                        },
                    )
                })
            })
            .collect()
    }
}

pub fn run_batch(workbook: &mut Workbook, edits: Vec<Edit>) -> Vec<EngineError> {
    edits
        .into_iter()
        .filter_map(|edit| edit(workbook).err())
        .collect()
}
