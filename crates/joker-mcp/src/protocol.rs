//! JSON-RPC 2.0 types for MCP (Model Context Protocol).
//!
//! MCP uses JSON-RPC 2.0 as its wire format. This module defines the
//! request, response, and notification types used in the protocol handshake
//! and tool interactions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC version string (`"2.0"`).
    pub jsonrpc: String,
    /// Request identifier used to correlate responses.
    pub id: Value,
    /// Name of the method to invoke.
    pub method: String,
    /// Optional method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new request with the given id, method, and optional params.
    #[must_use]
    pub fn new(id: u64, method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: Value::Number(id.into()),
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 successful response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// JSON-RPC version string (`"2.0"`).
    pub jsonrpc: String,
    /// Request identifier (mirrors the originating request's `id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Successful result payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object if the request failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code (negative values are reserved by JSON-RPC).
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC 2.0 notification (no id).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// JSON-RPC version string (`"2.0"`).
    pub jsonrpc: String,
    /// Name of the notification method.
    pub method: String,
    /// Optional notification parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

// ── MCP-specific types ─────────────────────────────────────────────────────

/// Client capabilities sent during initialization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Capability for root resource support.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
}

/// Capability for MCP root resources (file-system roots).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootsCapability {
    /// Whether the server supports `roots/listChanged` notifications.
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        Self {
            roots: Some(RootsCapability {
                list_changed: false,
            }),
        }
    }
}

/// Server capabilities returned after initialization.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Details about tool-related capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

/// Capability indicating whether the server supports tool discovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsCapability {
    /// Whether the server sends `notifications/tools/listChanged`.
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

/// Parameters for the `initialize` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitializeParams {
    /// MCP protocol version (e.g. `"2024-11-05"`).
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Client capabilities advertised to the server.
    pub capabilities: ClientCapabilities,
    /// Identifying information about the client.
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

/// Identifying information about the client application.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Human-readable client name.
    pub name: String,
    /// Client version string.
    pub version: String,
}

/// Result of the `initialize` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitializeResult {
    /// Negotiated protocol version.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Server capabilities advertised by the server.
    pub capabilities: ServerCapabilities,
    /// Identifying information about the server.
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

/// Identifying information about the MCP server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Human-readable server name.
    pub name: String,
    /// Server version string.
    pub version: String,
}

/// Parameters for the `tools/list` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListToolsParams {
    /// Pagination cursor returned by a previous `tools/list` response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Result of the `tools/list` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListToolsResult {
    /// List of tool definitions available on the server.
    pub tools: Vec<McpToolDef>,
    /// Cursor for paginating through additional tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// An MCP tool definition as received from the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Name of the tool.
    pub name: String,
    /// Human-readable description of what the tool does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the tool's input parameters.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Parameters for the `tools/call` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallToolParams {
    /// Name of the tool to invoke.
    pub name: String,
    /// Arguments to pass to the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// Result of the `tools/call` request — the tool's output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallToolResult {
    /// List of content items returned by the tool.
    pub content: Vec<ToolContent>,
    /// Whether the tool execution resulted in an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
}

/// A single piece of content returned by an MCP tool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolContent {
    /// Content type discriminator (e.g. `"text"`, `"image"`).
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text content when the type is `"text"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Base64-encoded binary data when the type is `"image"` etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// MIME type of the content.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

// ── Helpers ────────────────────────────────────────────────────────────────

impl InitializeParams {
    /// Create `initialize` params for the given client name and version.
    #[must_use]
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            protocol_version: "2024-11-05".into(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: name.into(),
                version: version.into(),
            },
        }
    }
}

/// Parse a JSON-RPC message from a line of text (stdio transport).
/// Returns `None` if the line is empty or not valid JSON-RPC.
pub fn parse_message(line: &str) -> Option<Result<JsonRpcResponse, String>> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    match serde_json::from_str::<JsonRpcResponse>(line) {
        Ok(resp) => Some(Ok(resp)),
        Err(e) => Some(Err(format!("parse error: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_initialize_request() {
        let params = InitializeParams::new("joker", "0.0.1");
        let request = JsonRpcRequest::new(
            0,
            "initialize",
            Some(serde_json::to_value(&params).unwrap()),
        );
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"initialize\""));
        assert!(json.contains("\"protocolVersion\":\"2024-11-05\""));
        assert!(json.contains("\"joker\""));
    }

    #[test]
    fn parse_initialize_result() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 0,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": true }
                },
                "serverInfo": {
                    "name": "test-server",
                    "version": "1.0.0"
                }
            }
        }"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_none());
        let result: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.server_info.name, "test-server");
        assert!(result.capabilities.tools.is_some());
    }

    #[test]
    fn parse_tools_list_result() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo back input",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" }
                            }
                        }
                    }
                ]
            }
        }"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        let result: ListToolsResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "echo");
    }

    #[test]
    fn parse_error_response() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        }"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn parse_message_skips_empty() {
        assert!(parse_message("").is_none());
        assert!(parse_message("   ").is_none());
    }

    #[test]
    fn parse_message_returns_ok() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let result = parse_message(json);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }

    #[test]
    fn serialize_call_tool_params() {
        let params = CallToolParams {
            name: "echo".into(),
            arguments: Some(serde_json::json!({"text": "hello"})),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"name\":\"echo\""));
        assert!(json.contains("\"text\":\"hello\""));
    }
}
