//! Transport abstractions for MCP communication.
//!
//! Provides the [`McpTransport`] trait and a [`StdioTransport`] implementation
//! that communicates with a child process over stdin/stdout.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Errors that can occur during transport operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// An underlying I/O error from the process or channel.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The child process exited before the response was received.
    #[error("process exited unexpectedly")]
    ProcessExited,
    /// The transport has not been connected yet.
    #[error("transport not connected")]
    NotConnected,
    /// A JSON serialization or deserialization error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Abstract transport for MCP communication.
///
/// Implementations include stdio (subprocess) and HTTP-based transports.
#[async_trait::async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and return the matching response.
    async fn send_request(&mut self, request: &JsonRpcRequest) -> Result<JsonRpcResponse, TransportError>;

    /// Send a JSON-RPC notification (fire-and-forget, no response expected).
    async fn send_notification(&mut self, notification: &serde_json::Value) -> Result<(), TransportError>;

    /// Close the transport and clean up resources.
    async fn close(&mut self) -> Result<(), TransportError>;
}

// ── Stdio transport ────────────────────────────────────────────────────────

/// Transport that communicates with an MCP server via stdin/stdout.
///
/// Launches a child process and communicates with it using newline-delimited
/// JSON-RPC messages over stdio.
pub struct StdioTransport {
    child: Option<Child>,
    stdin: Option<tokio::process::ChildStdin>,
    reader: Option<BufReader<tokio::process::ChildStdout>>,
    next_id: u64,
}

impl StdioTransport {
    /// Start an MCP server subprocess and establish stdio communication.
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self, TransportError> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child.stdin.take().ok_or(TransportError::NotConnected)?;
        let stdout = child.stdout.take().ok_or(TransportError::NotConnected)?;
        let reader = BufReader::new(stdout);

        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            reader: Some(reader),
            next_id: 1,
        })
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn read_line(&mut self) -> Result<String, TransportError> {
        let reader = self.reader.as_mut().ok_or(TransportError::NotConnected)?;
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        if line.is_empty() {
            return Err(TransportError::ProcessExited);
        }
        Ok(line)
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(&mut self, request: &JsonRpcRequest) -> Result<JsonRpcResponse, TransportError> {
        let new_id = self.next_id();
        let stdin = self.stdin.as_mut().ok_or(TransportError::NotConnected)?;
        let mut request = request.clone();
        if request.id.is_null() {
            request.id = serde_json::Value::Number(new_id.into());
        }
        let mut body = serde_json::to_vec(&request)?;
        body.push(b'\n');
        stdin.write_all(&body).await?;

        // Read response — MCP may interleave notifications; skip them
        loop {
            let line = self.read_line().await?;
            let resp: JsonRpcResponse = serde_json::from_str(&line)?;
            // If the response has no id, it's a notification — skip and read next
            if resp.id.is_some() {
                return Ok(resp);
            }
        }
    }

    async fn send_notification(&mut self, notification: &serde_json::Value) -> Result<(), TransportError> {
        let stdin = self.stdin.as_mut().ok_or(TransportError::NotConnected)?;
        let mut body = serde_json::to_vec(notification)?;
        body.push(b'\n');
        stdin.write_all(&body).await?;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin); // Close stdin to signal EOF
        }
        if let Some(mut child) = self.child.take() {
            child.wait().await?;
        }
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Ensure child process is cleaned up
        self.stdin = None;
    }
}
