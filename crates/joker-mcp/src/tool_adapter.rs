//! Adapter that wraps an MCP tool as a Joker `Tool` implementation.
//!
//! Each MCP tool discovered from a server gets wrapped in an `McpToolAdapter`
//! so it can be registered in Joker's `ToolRegistry`.

use std::sync::Arc;

use joker::{
    ApprovalRequirement, Tool, ToolAnnotations, ToolCapability, ToolDefinition, ToolError,
    ToolExecution, ToolFuture, ToolInvocation, ToolName, ToolOutput,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::client::{McpClient, McpError};

/// Configuration for an MCP server connection used by tool adapters.
///
/// Multiple tools from the same server share a single [`McpClient`] via `Arc<Mutex<>>`.
#[derive(Clone)]
pub struct McpToolConfig {
    /// Path or name of the MCP server executable.
    pub command: String,
    /// Command-line arguments passed to the server process.
    pub args: Vec<String>,
}

/// Adapter that wraps an MCP tool from a remote server as a Joker `Tool`.
///
/// The adapter holds a shared reference to an initialized `McpClient`
/// and the tool's name + schema from the server.
pub struct McpToolAdapter {
    name: ToolName,
    description: String,
    input_schema: Value,
    client: Arc<Mutex<McpClient>>,
}

impl McpToolAdapter {
    /// Create a new adapter for a single MCP tool.
    #[must_use]
    pub fn new(
        tool_def: &crate::protocol::McpToolDef,
        client: Arc<Mutex<McpClient>>,
    ) -> Self {
        Self {
            name: ToolName::new(format!("mcp_{}", tool_def.name)),
            description: tool_def
                .description
                .clone()
                .unwrap_or_else(|| format!("MCP tool: {}", tool_def.name)),
            input_schema: tool_def.input_schema.clone(),
            client,
        }
    }

    /// Create multiple adapters from a list of MCP tool definitions sharing one client.
    pub fn from_tools(
        tools: &[crate::protocol::McpToolDef],
        client: Arc<Mutex<McpClient>>,
    ) -> Vec<Self> {
        tools
            .iter()
            .map(|def| Self::new(def, client.clone()))
            .collect()
    }
}

impl Tool for McpToolAdapter {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: Some(std::time::Duration::from_secs(30)),
                capabilities: vec![ToolCapability::Network],
                default_approval: ApprovalRequirement::Auto,
            },
        }
    }

    fn call(&self, invocation: ToolInvocation) -> ToolFuture<'_> {
        Box::pin(async move {
            let arguments = if invocation.arguments.is_null() {
                None
            } else {
                Some(invocation.arguments.clone())
            };

            let mut client = self.client.lock().await;
            match client.call_tool(&self.name.as_str().replace("mcp_", ""), arguments).await {
                Ok(result) => {
                    let output_text: String = result
                        .content
                        .iter()
                        .filter_map(|c| c.text.clone())
                        .collect::<Vec<_>>()
                        .join("\n");

                    let is_error = result.is_error.unwrap_or(false);
                    Ok(ToolOutput::new(if is_error {
                        serde_json::json!({ "error": output_text })
                    } else {
                        serde_json::json!({ "result": output_text })
                    }))
                }
                Err(e) => Err(ToolError::Execution(format!("MCP tool error: {e}"))),
            }
        })
    }
}

/// Connect to an MCP server, initialize, list tools, and return adapters.
///
/// This is the main entry point for setting up MCP tools. Returns the
/// client handle (for lifecycle management) and a list of tool adapters.
pub async fn connect_and_discover(
    config: &McpToolConfig,
) -> Result<
    (
        Arc<Mutex<McpClient>>,
        Vec<McpToolAdapter>,
    ),
    McpError,
> {
    let transport = crate::transport::StdioTransport::spawn(&config.command, &config.args)
        .await
        .map_err(McpError::Transport)?;

    let mut client = McpClient::new(Box::new(transport));
    client.initialize("joker", "0.0.1").await?;

    let tools = client.list_tools().await?;

    let client = Arc::new(Mutex::new(client));
    let adapters = McpToolAdapter::from_tools(&tools, client.clone());

    Ok((client, adapters))
}
