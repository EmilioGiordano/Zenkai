use async_channel::{Receiver, Sender};

use crate::tools::error::ToolError;
use crate::tools::reply::ToolReply;
use crate::tools::request::ToolRequest;

// Calls waiting for the UI thread; past this the caller waits to send, so a runaway
// agent cannot queue unbounded work.
const QUEUED_CALLS: usize = 16;

pub type ToolResult = Result<ToolReply, ToolError>;

pub struct ToolCall {
    pub request: ToolRequest,
    // How the client named itself in the handshake, already escaped for display.
    pub client: Option<String>,
    reply: Sender<ToolResult>,
}

impl ToolCall {
    pub fn respond(self, result: ToolResult) {
        if self.reply.try_send(result).is_err() {
            tracing::debug!("the tool caller stopped waiting before the reply");
        }
    }
}

#[derive(Clone)]
pub struct ToolEndpoint {
    calls: Sender<ToolCall>,
}

impl ToolEndpoint {
    pub async fn call(&self, request: ToolRequest) -> ToolResult {
        self.call_from(request, None).await
    }

    pub async fn call_from(&self, request: ToolRequest, client: Option<String>) -> ToolResult {
        let (reply, response) = async_channel::bounded(1);
        self.calls
            .send(ToolCall {
                request,
                client,
                reply,
            })
            .await
            .map_err(|_| ToolError::Closed)?;
        response.recv().await.map_err(|_| ToolError::Closed)?
    }
}

pub fn channel() -> (ToolEndpoint, Receiver<ToolCall>) {
    let (calls, received) = async_channel::bounded(QUEUED_CALLS);
    (ToolEndpoint { calls }, received)
}
