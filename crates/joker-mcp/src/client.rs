//! MCP client — connects to a server, performs handshake, and discovers/invokes tools.

use serde_json::Value;

use crate::protocol::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, JsonRpcRequest,
    ListToolsParams, ListToolsResult,
};
use crate::transport::{McpTransport, TransportError};

/// High-level MCP client that wraps a transport and manages the session.
pub struct McpClient {
    transport: Box<dyn McpTransport>,
    server_info: Option<crate::protocol::ServerInfo>,
    capabilities: Option<crate::protocol::ServerCapabilities>,
    initialized: bool,
}

impl McpClient {
    /// Create a new client with the given transport.
    #[must_use]
    pub fn new(transport: Box<dyn McpTransport>) -> Self {
        Self {
            transport,
            server_info: None,
            capabilities: None,
            initialized: false,
        }
    }

    /// Perform the MCP initialization handshake.
    ///
    /// Sends `initialize` and then `notifications/initialized` to complete
    /// the protocol handshake.
    pub async fn initialize(
        &mut self,
        client_name: &str,
        client_version: &str,
    ) -> Result<(), McpError> {
        let params = InitializeParams::new(client_name, client_version);
        let request = JsonRpcRequest::new(
            0,
            "initialize",
            Some(
                serde_json::to_value(&params)
                    .map_err(|e| McpError::Protocol(format!("serialize initialize params: {e}")))?,
            ),
        );

        let response = self.transport.send_request(&request).await?;

        if let Some(err) = response.error {
            return Err(McpError::Server(err.message));
        }

        let result: InitializeResult = serde_json::from_value(response.result.unwrap_or_default())
            .map_err(|e| McpError::Protocol(format!("parse initialize result: {e}")))?;

        self.server_info = Some(result.server_info);
        self.capabilities = Some(result.capabilities);

        // Send initialized notification
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });
        self.transport.send_notification(&notification).await?;
        self.initialized = true;

        Ok(())
    }

    /// List tools available from the MCP server.
    pub async fn list_tools(&mut self) -> Result<Vec<crate::protocol::McpToolDef>, McpError> {
        if !self.initialized {
            return Err(McpError::Protocol(
                "client not initialized — call initialize() first".into(),
            ));
        }

        let params = ListToolsParams { cursor: None };
        let request = JsonRpcRequest::new(
            1,
            "tools/list",
            Some(
                serde_json::to_value(&params)
                    .map_err(|e| McpError::Protocol(format!("serialize list_tools params: {e}")))?,
            ),
        );

        let response = self.transport.send_request(&request).await?;

        if let Some(err) = response.error {
            return Err(McpError::Server(err.message));
        }

        let result: ListToolsResult =
            serde_json::from_value(response.result.unwrap_or_default())
                .map_err(|e| McpError::Protocol(format!("parse list_tools result: {e}")))?;

        Ok(result.tools)
    }

    /// Call an MCP tool by name with arguments.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<CallToolResult, McpError> {
        if !self.initialized {
            return Err(McpError::Protocol(
                "client not initialized — call initialize() first".into(),
            ));
        }

        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };
        let request = JsonRpcRequest::new(
            2,
            "tools/call",
            Some(
                serde_json::to_value(&params)
                    .map_err(|e| McpError::Protocol(format!("serialize call_tool params: {e}")))?,
            ),
        );

        let response = self.transport.send_request(&request).await?;

        if let Some(err) = response.error {
            return Err(McpError::Server(err.message));
        }

        let result: CallToolResult = serde_json::from_value(response.result.unwrap_or_default())
            .map_err(|e| McpError::Protocol(format!("parse call_tool result: {e}")))?;

        Ok(result)
    }

    /// Close the connection to the MCP server.
    pub async fn close(&mut self) -> Result<(), McpError> {
        self.transport.close().await?;
        self.initialized = false;
        Ok(())
    }

    /// The server info reported during initialization.
    #[must_use]
    pub fn server_info(&self) -> Option<&crate::protocol::ServerInfo> {
        self.server_info.as_ref()
    }
}

/// Errors that can occur during MCP client operations.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// An error from the underlying transport layer.
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    /// A protocol-level error (e.g. serialization, missing initialization).
    #[error("protocol error: {0}")]
    Protocol(String),
    /// An error returned by the MCP server itself.
    #[error("server error: {0}")]
    Server(String),
    /// A JSON serialization or deserialization error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
