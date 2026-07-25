pub mod client;
pub mod protocol;
pub mod tool_adapter;
pub mod transport;

pub use client::{McpClient, McpError};
pub use protocol::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, JsonRpcRequest,
    JsonRpcResponse, ListToolsResult, McpToolDef, ServerCapabilities,
};
pub use tool_adapter::{McpToolAdapter, McpToolConfig, connect_and_discover};
pub use transport::{McpTransport, StdioTransport, TransportError};
