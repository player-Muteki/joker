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
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
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
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC 2.0 notification (no id).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

// ── MCP-specific types ─────────────────────────────────────────────────────

/// Client capabilities sent during initialization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootsCapability {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

/// Parameters for the `initialize` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Result of the `initialize` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Parameters for the `tools/list` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListToolsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Result of the `tools/list` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// An MCP tool definition as received from the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Parameters for the `tools/call` request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// Result of the `tools/call` request — the tool's output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
}

/// A single piece of content returned by an MCP tool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

// ── Helpers ────────────────────────────────────────────────────────────────

impl InitializeParams {
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
        let result: InitializeResult =
            serde_json::from_value(resp.result.unwrap()).unwrap();
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
        let result: ListToolsResult =
            serde_json::from_value(resp.result.unwrap()).unwrap();
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
