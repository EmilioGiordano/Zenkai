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
    // The client closed the connection or gave up while this call waited for the user.
    pub fn is_abandoned(&self) -> bool {
        self.reply.is_closed()
    }

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

#[cfg(test)]
mod tests {
    use futures_lite::future::{block_on, poll_once};

    use super::*;

    #[test]
    fn a_call_whose_caller_went_away_is_abandoned() {
        let (endpoint, calls) = channel();
        let mut waiting = Box::pin(endpoint.call(ToolRequest::ListWorkbooks));
        assert!(block_on(poll_once(&mut waiting)).is_none());
        let call = calls.try_recv().unwrap();
        assert!(!call.is_abandoned());
        drop(waiting);
        assert!(call.is_abandoned());
    }
}
