//! MCP (Model Context Protocol) client and protocol types for Joker.
//!
//! This crate provides JSON-RPC 2.0 wire types for MCP, a transport-agnostic
//! [`McpClient`], a stdio-based [`StdioTransport`], and an adapter that wraps
//! MCP tools as Joker [`Tool`](joker::Tool) implementations.

#![warn(missing_docs)]

pub mod client;
pub mod protocol;
pub mod tool_adapter;
pub mod transport;

#[doc(inline)]
pub use client::{McpClient, McpError};
#[doc(inline)]
pub use protocol::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, JsonRpcRequest,
    JsonRpcResponse, ListToolsResult, McpToolDef, ServerCapabilities,
};
#[doc(inline)]
pub use tool_adapter::{McpToolAdapter, McpToolConfig, connect_and_discover};
#[doc(inline)]
pub use transport::{McpTransport, StdioTransport, TransportError};
