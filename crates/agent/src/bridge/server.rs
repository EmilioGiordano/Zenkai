use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;

use crate::bridge::catalog;
use crate::tools::{Nonce, ToolEndpoint, UNTRUSTED_NOTICE, WorkingFolder, client_label};

const CLIENT_CHECK: std::time::Duration = std::time::Duration::from_millis(500);

const INSTRUCTIONS: &str = "Zenkai is a spreadsheet. These tools read and change the \
     workbooks the user has open, live: changes show at once, recalculate, can be undone by \
     the user and are never saved by Zenkai. Start with list_workbooks.";

fn named_client(
    chat_agent: Option<&str>,
    handshake: impl FnOnce() -> Option<String>,
) -> Option<String> {
    chat_agent.map(str::to_string).or_else(handshake)
}

#[derive(Clone)]
pub struct ZenkaiServer {
    pub endpoint: ToolEndpoint,
    // Only the chat's own bridge has one; external agents cannot create or open files.
    pub folder: Option<WorkingFolder>,
    // The agent the chat launched; its calls are named after it, whatever it calls itself.
    pub chat_agent: Option<String>,
}

impl ServerHandler for ZenkaiServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("zenkai", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!("{INSTRUCTIONS} {UNTRUSTED_NOTICE}"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(catalog::tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let failed = |message: String| CallToolResult::error(vec![ContentBlock::text(message)]);
        let call = match catalog::parse_call(&request.name, request.arguments, self.folder.as_ref())
        {
            Ok(call) => call,
            Err(error) => return Ok(failed(error.to_string()).into()),
        };
        let nonce = match Nonce::random() {
            Ok(nonce) => nonce,
            Err(error) => return Ok(failed(error.to_string()).into()),
        };
        let client = named_client(self.chat_agent.as_deref(), || {
            context
                .peer
                .peer_info()
                .and_then(|info| client_label(&info.client_info.name, &info.client_info.version))
        });
        // rmcp keeps this handler running after the client leaves; dropping the pending call
        // is what tells Zenkai nobody waits for the user's answer any more.
        let call = self.endpoint.call_from(call, client);
        tokio::pin!(call);
        let outcome = loop {
            tokio::select! {
                result = &mut call => break Some(result),
                () = context.ct.cancelled() => break None,
                () = tokio::time::sleep(CLIENT_CHECK) => {
                    if context.peer.is_transport_closed() {
                        break None;
                    }
                }
            }
        };
        let result = match outcome {
            Some(Ok(reply)) => {
                CallToolResult::success(vec![ContentBlock::text(reply.render(&nonce))])
            }
            Some(Err(error)) => failed(error.to_string()),
            None => failed("the client closed the connection".to_string()),
        };
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use super::named_client;

    #[test]
    fn calls_from_the_chat_are_named_after_its_agent() {
        let client = named_client(Some("Claude"), || Some("zenkai-mcp 1".to_string()));
        assert_eq!(client.as_deref(), Some("Claude"));
    }

    #[test]
    fn an_external_client_keeps_its_handshake_name() {
        let client = named_client(None, || Some("Codex".to_string()));
        assert_eq!(client.as_deref(), Some("Codex"));
    }
}
