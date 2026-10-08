mod approval_text;
mod channel;
mod error;
#[cfg(any(test, feature = "test-support"))]
mod local_host;
mod read;
mod reply;
mod request;
#[cfg(test)]
mod tests;
mod write;

pub use channel::{ToolCall, ToolEndpoint, ToolResult, channel};
pub use error::{AgentAccess, ReadOnlyReason, ToolError};
#[cfg(any(test, feature = "test-support"))]
pub use local_host::LocalHost;
pub use read::{LONG_TEXT_CHARS, MAX_FIND_RESULTS, MAX_READ_CELLS, read};
pub use reply::{
    CellPage, FindResult, FoundCell, HiddenContent, Nonce, SheetSummary, ToolReply,
    UNTRUSTED_NOTICE, WorkbookSummary, WriteSummary,
};
pub use request::{
    Alignment, Borders, Find, FormatChange, FormatRange, GetSelection, ListSheets, ListWorkbooks,
    NumberFormatName, ReadRange, ReadRequest, SetFormula, ToolRequest, WriteCells, WriteRequest,
};
pub use write::{MAX_CELL_CHARS, MAX_FORMAT_CELLS, MAX_WRITE_CELLS, PlannedWrite, plan_write};

use zenkai_types::SheetInfo;
pub use zenkai_types::WorkbookId;

// A call names the workbook it means; anything but the open document is refused, so a
// call made before the user opened another file never lands in the new one.
pub fn check_workbook(open: WorkbookId, requested: WorkbookId) -> Result<(), ToolError> {
    if open == requested {
        Ok(())
    } else {
        Err(ToolError::UnknownWorkbook(requested))
    }
}

pub fn workbook_summary(
    id: WorkbookId,
    name: &str,
    sheets: &[SheetInfo],
    access: AgentAccess,
) -> WorkbookSummary {
    WorkbookSummary {
        id,
        name: name.to_string(),
        sheets: sheets.len(),
        access,
    }
}
