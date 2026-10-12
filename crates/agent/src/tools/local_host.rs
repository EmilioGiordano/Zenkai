use async_channel::Receiver;
use zenkai_engine::{Engine, Workbook, open_xlsx};
use zenkai_types::{Range, SheetId};

use crate::tools::channel::{ToolCall, ToolResult};
use crate::tools::error::{AgentAccess, ToolError};
use crate::tools::reply::{OpenedWorkbook, Opening, ToolReply};
use crate::tools::request::ToolRequest;
use crate::tools::{
    WorkbookPlace, check_workbook, create_workbook_file, plan_write, read, relative_text,
    workbook_summary,
};
use zenkai_types::WorkbookId;

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
                let workbook = open_xlsx(path.path())
                    .map_err(|error| ToolError::Engine(error.to_string()))?
                    .workbook;
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
