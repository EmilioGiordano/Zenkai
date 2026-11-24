use std::io::ErrorKind;

use async_channel::Receiver;
use zenkai_engine::{Engine, Workbook, open_xlsx, workbook_from_rows};
use zenkai_formats::{
    CsvError, CsvReadError, MAX_CSV_BYTES, apply, guess, parse_csv, prepare_import_inputs,
    read_csv_bytes, tsv_delimiter,
};
use zenkai_types::{Range, SheetId, WorkbookId};

use crate::tools::channel::{ToolCall, ToolResult};
use crate::tools::error::{AgentAccess, ToolError};
use crate::tools::folder::InsidePath;
use crate::tools::reply::{OpenedWorkbook, Opening, ToolReply};
use crate::tools::request::ToolRequest;
use crate::tools::{
    WorkbookPlace, check_workbook, create_workbook_file, plan_write, read, relative_text,
    workbook_summary,
};

// Serves tools straight on a workbook, writes approved automatically: what Zenkai's
// UI thread does, minus the window, so tools can be tested without one.
pub struct LocalHost {
    pub id: WorkbookId,
    pub name: String,
    pub workbook: Workbook,
    pub selection: (SheetId, Range),
    pub access: AgentAccess,
}

impl LocalHost {
    pub fn handle(&mut self, request: &ToolRequest) -> ToolResult {
        match request {
            ToolRequest::ListWorkbooks => Ok(ToolReply::Workbooks(vec![workbook_summary(
                self.id,
                &self.name,
                &WorkbookPlace {
                    space: "Workbooks",
                    location: "not saved",
                },
                &self.workbook.sheets(),
                self.access,
            )])),
            ToolRequest::GetSelection(id) => {
                check_workbook(self.id, *id)?;
                let (sheet, range) = self.selection;
                let name = self
                    .workbook
                    .sheets()
                    .into_iter()
                    .find(|info| info.id == sheet)
                    .map(|info| info.name)
                    .ok_or_else(|| ToolError::UnknownSheet(format!("#{}", sheet.0 + 1)))?;
                Ok(ToolReply::Selection { sheet: name, range })
            }
            ToolRequest::Read(id, request) => {
                check_workbook(self.id, *id)?;
                read(request, &self.workbook)
            }
            ToolRequest::Write(id, request) => {
                check_workbook(self.id, *id)?;
                self.access.check_write()?;
                let plan = plan_write(request, &self.workbook.sheets())?;
                let summary = plan.summary();
                plan.apply(&mut self.workbook)
                    .map_err(|error| ToolError::Engine(error.to_string()))?;
                Ok(ToolReply::Written(summary))
            }
            ToolRequest::CreateWorkbook(new) => {
                let workbook = create_workbook_file(new)?;
                Ok(self.replace(workbook, relative_text(&new.path), Opening::Created))
            }
            ToolRequest::OpenWorkbook(path) => {
                let workbook = open_file(path)?;
                Ok(self.replace(workbook, relative_text(path), Opening::Opened))
            }
        }
    }

    // The host holds one workbook: a created or opened one takes its place under a new id.
    fn replace(&mut self, workbook: Workbook, path: String, how: Opening) -> ToolReply {
        self.id = WorkbookId(self.id.0 + 1);
        self.name = path.clone();
        self.workbook = workbook;
        ToolReply::Opened(OpenedWorkbook {
            id: self.id,
            how,
            path,
            sheets: self
                .workbook
                .sheets()
                .into_iter()
                .map(|sheet| sheet.name)
                .collect(),
        })
    }

    pub fn serve(mut self, calls: Receiver<ToolCall>) -> LocalHost {
        while let Ok(call) = calls.recv_blocking() {
            let result = self.handle(&call.request);
            call.respond(result);
        }
        self
    }
}

fn open_file(path: &InsidePath) -> Result<Workbook, ToolError> {
    if matches!(path.extension().as_deref(), Some("csv" | "tsv")) {
        open_csv(path)
    } else {
        open_xlsx(path.path())
            .map(|opened| opened.workbook)
            .map_err(|error| ToolError::Engine(error.to_string()))
    }
}

fn open_csv(path: &InsidePath) -> Result<Workbook, ToolError> {
    let bytes = read_csv_bytes(path.path(), MAX_CSV_BYTES).map_err(|error| match error {
        CsvReadError::Io(source) if source.kind() == ErrorKind::NotFound => {
            ToolError::FileNotFound(relative_text(path))
        }
        CsvReadError::Io(source) => ToolError::File(source.to_string()),
        CsvReadError::TooLarge(megabytes) => {
            ToolError::File(CsvError::TooLarge(megabytes).to_string())
        }
    })?;
    let parsed = parse_csv(&bytes, tsv_delimiter(path.path()))
        .map_err(|error| ToolError::File(error.to_string()))?;
    let guessed = guess(&parsed);
    let mut rows = parsed.rows;
    apply(guessed.decimal_mark, guessed.date_order, &mut rows);
    prepare_import_inputs(&mut rows);
    workbook_from_rows(rows).map_err(|error| ToolError::Engine(error.to_string()))
}
