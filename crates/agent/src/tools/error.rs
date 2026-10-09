use std::fmt;

use zenkai_datagen::DatagenError;
use zenkai_types::WorkbookId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnlyReason {
    Settings,
    ProtectedView,
    ValuesOnlyFile,
}

impl fmt::Display for ReadOnlyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ReadOnlyReason::Settings => "the user set agents to read only in Zenkai's settings",
            ReadOnlyReason::ProtectedView => {
                "the file came from the internet (Protected View), so agents may only read it"
            }
            ReadOnlyReason::ValuesOnlyFile => {
                "Zenkai opened this file read-only because it could only read its values"
            }
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAccess {
    Editable,
    ReadOnly(ReadOnlyReason),
}

impl AgentAccess {
    pub fn check_write(self) -> Result<(), ToolError> {
        match self {
            AgentAccess::Editable => Ok(()),
            AgentAccess::ReadOnly(reason) => Err(ToolError::ReadOnly(reason)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ToolError {
    #[error("no open workbook has id {}; call list_workbooks for the current ids", .0.0)]
    UnknownWorkbook(WorkbookId),
    #[error("workbook {} is in the sidebar but not loaded; the user must open it in Zenkai first", .0.0)]
    NotLoaded(WorkbookId),
    #[error("the workbook has no sheet named \"{0}\"; call list_sheets")]
    UnknownSheet(String),
    #[error("\"{0}\" is not a cell or range in A1 notation")]
    BadAddress(String),
    #[error("{cells} cells is more than the {limit} allowed in one call; use a smaller range")]
    TooManyCells { cells: u64, limit: u64 },
    #[error("page {page} does not exist; this range has {pages} page(s), numbered from 0")]
    NoSuchPage { page: u32, pages: u32 },
    #[error("the text to find is empty")]
    EmptySearch,
    #[error("every row must have the same number of entries")]
    NotRectangular,
    #[error("there is nothing to write")]
    NothingToWrite,
    #[error("the block does not fit in the sheet")]
    OutsideSheet,
    #[error("a formula must start with \"=\"")]
    NotAFormula,
    #[error("an entry has {chars} characters; Excel allows at most {limit} in a cell")]
    TextTooLong { chars: usize, limit: usize },
    #[error("the generation spec cannot be used: {0}")]
    Generation(DatagenError),
    #[error("writing is not allowed: {0}")]
    ReadOnly(ReadOnlyReason),
    #[error("the user is editing a cell in Zenkai; try again when they finish")]
    UserEditing,
    #[error("the user declined this change")]
    Declined,
    #[error("the user did not answer in time; the change was not made")]
    ApprovalTimedOut,
    #[error("another change is waiting for the user's approval; try again after it")]
    AwaitingApproval,
    #[error("Zenkai is still calculating; try again in a moment")]
    Busy,
    #[error("Zenkai refused the change: {0}")]
    Engine(String),
    #[error("Zenkai is closing or no longer serving tools")]
    Closed,
    #[error("could not create a random marker for the reply: {0}")]
    Random(String),
}
