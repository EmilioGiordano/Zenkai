use async_channel::Receiver;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{Range, SheetId};

use crate::tools::channel::{ToolCall, ToolResult};
use crate::tools::error::{AgentAccess, ToolError};
use crate::tools::reply::ToolReply;
use crate::tools::request::ToolRequest;
use crate::tools::{check_workbook, plan_write, read, workbook_summary};
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
        }
    }

    pub fn serve(mut self, calls: Receiver<ToolCall>) -> LocalHost {
        while let Ok(call) = calls.recv_blocking() {
            let result = self.handle(&call.request);
            call.respond(result);
        }
        self
    }
}
