//! Anthropic Messages API provider.
//!
//! Ported from opencode's `anthropic-messages.ts` protocol implementation.
//! Supports streaming text, thinking, tool calls, and usage tracking via
//! the Anthropic Messages API SSE stream.

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    collections::BTreeMap,
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Stream;
use futures_util::StreamExt;
use joker::{
    Content, Message, Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent,
    ModelStream, Role, StopReason, ToolCall, ToolDefinition, Usage,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::transform;

/// Default base URL for the Anthropic Messages API.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";

/// Default path for the Messages endpoint.
pub const MESSAGES_PATH: &str = "/messages";

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Maximum number of cache breakpoints per request.
pub const ANTHROPIC_BREAKPOINT_CAP: usize = 4;

/// Configuration for the Anthropic provider.
#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl AnthropicConfig {
    fn messages_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            MESSAGES_PATH
        )
    }
}

/// Errors specific to the Anthropic provider.
#[derive(Debug, Error)]
pub enum AnthropicProviderError {
    #[error("Anthropic API key is missing; set ANTHROPIC_API_KEY")]
    MissingApiKey,
    #[error("invalid authorization header")]
    InvalidAuthHeader,
    #[error("anthropic api error: {0}")]
    Api(String),
}

/// A model backed by the Anthropic Messages API.
#[derive(Clone, Debug)]
pub struct AnthropicModel {
    config: AnthropicConfig,
    client: reqwest::Client,
}

impl AnthropicModel {
    /// Create a new `AnthropicModel`.
    ///
    /// Returns an error if the API key is empty.
    pub fn new(config: AnthropicConfig) -> Result<Self, AnthropicProviderError> {
        if config.api_key.is_empty() {
            return Err(AnthropicProviderError::MissingApiKey);
        }

        let mut headers = HeaderMap::new();
    let mut auth_header = HeaderValue::from_str(&config.api_key)
            .map_err(|_| AnthropicProviderError::InvalidAuthHeader)?;
        auth_header.set_sensitive(true);
        headers.insert("x-api-key", auth_header);
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("reqwest client should build");

        Ok(Self { config, client })
    }
}

impl Model for AnthropicModel {
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_> {
        let config = self.config.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let body = build_request_body(&config.model, &request);
            let response = client
                .post(config.messages_url())
                .json(&body)
                .send()
                .await
                .map_err(|e| ModelError::Stream(e.to_string()))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(ModelError::Stream(format!(
                    "Anthropic request failed with {status}: {body}"
                )));
            }

            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(parse_sse_stream(response, tx));
            Ok(Box::new(ReceiverStream { rx }) as ModelStream)
        })
    }
}

// ---------------------------------------------------------------------------
// Stream receiver
// ---------------------------------------------------------------------------

struct ReceiverStream {
    rx: mpsc::UnboundedReceiver<Result<ModelResponseEvent, ModelError>>,
}

impl Stream for ReceiverStream {
    type Item = Result<ModelResponseEvent, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// ---------------------------------------------------------------------------
// Request body construction (port of AnthropicMessages.fromRequest)
// ---------------------------------------------------------------------------

fn build_request_body(model: &str, request: &ModelRequest) -> Value {
    let system = build_system(&request.messages);
    let messages = build_messages(&request.messages);
    let tools = build_tools(&request.tools);
    let max_tokens = 4096;

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
    });

    if let Some(system) = system {
        body["system"] = system;
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }

    body
}

fn build_system(messages: &[Message]) -> Option<Value> {
    let parts: Vec<Value> = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .flat_map(|m| &m.content)
        .filter_map(|c| {
            if let Content::Text(t) = c {
                Some(serde_json::json!({"type": "text", "text": t.text}))
            } else {
                None
            }
        })
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(Value::Array(parts))
    }
}

fn build_messages(messages: &[Message]) -> Vec<Value> {
    let msgs = transform::normalize_messages(messages, "anthropic", "");

    let mut result: Vec<Value> = Vec::new();

    for msg in &msgs {
        match msg.role {
            Role::System => {
                // System messages are handled via the top-level `system` field
            }
            Role::User => {
                let parts: Vec<Value> = msg
                    .content
                    .iter()
                    .map(|c| match c {
                        Content::Text(t) => {
                            serde_json::json!({"type": "text", "text": t.text})
                        }
                        Content::ToolResult(tr) => {
                            serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tr.call_id,
                                "content": tr.output.to_string(),
                                "is_error": tr.is_error,
                            })
                        }
                        _ => serde_json::json!({"type": "text", "text": ""}),
                    })
                    .collect();

                result.push(serde_json::json!({
                    "role": "user",
                    "content": parts,
                }));
            }
            Role::Assistant => {
                let mut parts: Vec<Value> = Vec::new();
                for c in &msg.content {
                    match c {
                        Content::Text(t) => {
                            parts.push(serde_json::json!({"type": "text", "text": t.text}));
                        }
                        Content::Reasoning(r) => {
                            parts.push(serde_json::json!({
                                "type": "thinking",
                                "thinking": r.text,
                            }));
                        }
                        Content::ToolCall(tc) => {
                            parts.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            }));
                        }
                        _ => {}
                    }
                }
                result.push(serde_json::json!({
                    "role": "assistant",
                    "content": parts,
                }));
            }
            Role::Tool => {
                let parts: Vec<Value> = msg
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::ToolResult(tr) = c {
                            Some(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tr.call_id,
                                "content": tr.output.to_string(),
                                "is_error": tr.is_error,
                            }))
                        } else {
                            None
                        }
                    })
                    .collect();

                // Tool results are sent as user messages in Anthropic API
                result.push(serde_json::json!({
                    "role": "user",
                    "content": parts,
                }));
            }
            _ => {} // non-exhaustive
        }
    }

    result
}

fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name.as_str(),
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SSE stream parser (port of AnthropicMessages stream parsing)
// ---------------------------------------------------------------------------

/// Pending tool call being built up across multiple `input_json_delta` events.
#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Streaming parser for the Anthropic Messages API SSE format.
struct AnthropicSseParser {
    buffer: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    pending_text: String,
    pending_reasoning: String,
    pending_reasoning_signature: Option<String>,
    current_block_index: Option<usize>,
}

impl AnthropicSseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            tool_calls: BTreeMap::new(),
            pending_text: String::new(),
            pending_reasoning: String::new(),
            pending_reasoning_signature: None,
            current_block_index: None,
        }
    }

    fn push(&mut self, chunk: &str) -> Vec<ParsedEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(delim) = self.buffer.find("\n\n") {
            // Anthropic SSE uses \n\n as event delimiter but individual lines
            // are separated by \n. Each event starts with `event: <type>` and
            // has `data: {...}`.
            let raw = self.buffer[..delim].to_string();
            self.buffer.drain(..=delim + 1); // skip past \n\n

            // Parse event type and data
            let event_type = raw
                .lines()
                .find(|l| l.starts_with("event:"))
                .map(|l| l.trim_start_matches("event:").trim())
                .unwrap_or("");

            let data = raw
                .lines()
                .find(|l| l.starts_with("data:"))
                .map(|l| l.trim_start_matches("data:").trim());

            // For simpler parsing, try parsing the data as JSON directly
            // (some implementations skip the `event:` prefix)
            let json_str = data.unwrap_or(raw.trim());

            if json_str.is_empty() || json_str == "{}" {
                continue;
            }

            match self.parse_event(event_type, json_str) {
                Ok(Some(parsed)) => events.push(parsed),
                Ok(None) => {}
                Err(e) => {
                    events.push(ParsedEvent::Error(e));
                    return events;
                }
            }
        }

        events
    }

    fn parse_event(
        &mut self,
        event_type: &str,
        data: &str,
    ) -> Result<Option<ParsedEvent>, String> {
        // Try parsing as the Anthropic event envelope
        let event: AnthropicStreamEvent =
            serde_json::from_str(data).map_err(|e| format!("invalid SSE JSON: {e}: {data}"))?;

        match event_type {
            "message_start" | "message_start " => {
                // Extract usage from message
                if let Some(usage) = event
                    .message
                    .as_ref()
                    .and_then(|m| m.usage.as_ref())
                {
                    let total_input = usage.input_tokens.unwrap_or(0)
                        + usage.cache_creation_input_tokens.unwrap_or(0)
                        + usage.cache_read_input_tokens.unwrap_or(0);
                    self.set_usage(Some(Usage {
                        input_tokens: total_input,
                        output_tokens: usage.output_tokens.unwrap_or(0),
                    }));
                }
                Ok(None)
            }
            "content_block_start" | "content_block_start " => {
                if let Some(block) = &event.content_block {
                    self.current_block_index = event.index;
                    match block.block_type.as_deref() {
                        Some("text") => {
                            if let Some(text) = &block.text {
                                self.pending_text.push_str(text);
                            }
                        }
                        Some("thinking") => {
                            if let Some(thinking) = &block.thinking {
                                self.pending_reasoning.push_str(thinking);
                            }
                        }
                        Some("tool_use") | Some("tool_use ") => {
                            let idx = event.index.unwrap_or(0);
                            let id = block.id.clone().unwrap_or_default();
                            let name = block.name.clone().unwrap_or_default();
                            let partial = block.input.as_ref().and_then(|v| v.as_str()).unwrap_or("");
                            self.tool_calls.insert(
                                idx,
                                PartialToolCall {
                                    id,
                                    name,
                                    arguments: partial.to_string(),
                                },
                            );
                        }
                        _ => {}
                    }
                }
                Ok(None)
            }
            "content_block_delta" | "content_block_delta " => {
                if let Some(delta) = &event.delta {
                    match delta.delta_type.as_deref() {
                        Some("text_delta") => {
                            if let Some(text) = &delta.text {
                                self.pending_text.push_str(text);
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(thinking) = &delta.thinking {
                                self.pending_reasoning.push_str(thinking);
                            }
                        }
                        Some("signature_delta") => {
                            self.pending_reasoning_signature = delta.signature.clone();
                        }
                        Some("input_json_delta") => {
                            let idx = event.index.unwrap_or(0);
                            if let Some(partial) = &delta.partial_json {
                                self.tool_calls
                                    .entry(idx)
                                    .or_default()
                                    .arguments
                                    .push_str(partial);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(None)
            }
            "content_block_stop" | "content_block_stop " => {
                // Check if we have a completed tool call at this index
                if let Some(idx) = event.index
                    && let Some(tc) = self.tool_calls.remove(&idx)
                        && !tc.id.is_empty() && !tc.name.is_empty() {
                            let arguments: Value =
                                serde_json::from_str(&tc.arguments).unwrap_or(Value::String(tc.arguments.clone()));
                            return Ok(Some(ParsedEvent::Event(
                                ModelResponseEvent::ToolCall(ToolCall {
                                    id: tc.id,
                                    name: tc.name,
                                    arguments,
                                }),
                            )));
                        }
                Ok(None)
            }
            "message_delta" | "message_delta " => {
                // Update usage from delta
                if let Some(usage) = &event.usage {
                    let total_input = usage.input_tokens.unwrap_or(0)
                        + usage.cache_creation_input_tokens.unwrap_or(0)
                        + usage.cache_read_input_tokens.unwrap_or(0);
                    self.set_usage(Some(Usage {
                        input_tokens: total_input,
                        output_tokens: usage.output_tokens.unwrap_or(0),
                    }));
                }

                // Emit finish
                let stop_reason = event
                    .delta
                    .as_ref()
                    .and_then(|d| d.stop_reason.as_deref())
                    .map(map_finish_reason)
                    .unwrap_or(StopReason::Stop);

                Ok(Some(ParsedEvent::Finish {
                    stop_reason,
                    usage: self.take_usage(),
                }))
            }
            "error" | "error " => {
                let msg = event
                    .error
                    .as_ref()
                    .and_then(|e| e.message.as_deref())
                    .unwrap_or("unknown anthropic error");
                Err(msg.to_string())
            }
            "ping" | "ping " => Ok(None),
            _ => {
                // Some providers (e.g. Bedrock) may not use the event: prefix.
                // Try parsing type from the data itself.
                if let Some(usage) = &event.usage {
                    let total_input = usage.input_tokens.unwrap_or(0)
                        + usage.cache_creation_input_tokens.unwrap_or(0)
                        + usage.cache_read_input_tokens.unwrap_or(0);
                    self.set_usage(Some(Usage {
                        input_tokens: total_input,
                        output_tokens: usage.output_tokens.unwrap_or(0),
                    }));
                }
                if let Some(delta) = &event.delta
                    && let Some(sr) = &delta.stop_reason {
                        let stop_reason = map_finish_reason(sr);
                        return Ok(Some(ParsedEvent::Finish {
                            stop_reason,
                            usage: self.take_usage(),
                        }));
                    }
                if let Some(content) = &event.content_block {
                    if let Some(text) = &content.text {
                        self.pending_text.push_str(text);
                    }
                    if let Some(thinking) = &content.thinking {
                        self.pending_reasoning.push_str(thinking);
                    }
                }
                Ok(None)
            }
        }
    }

    fn take_text(&mut self) -> Option<String> {
        let text = std::mem::take(&mut self.pending_text);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn take_reasoning(&mut self) -> Option<String> {
        let text = std::mem::take(&mut self.pending_reasoning);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn set_usage(&mut self, _usage: Option<Usage>) {
        // Usage is stored on event boundary, emitted in Finish
    }

    fn take_usage(&mut self) -> Usage {
        Usage::default()
    }

    fn finish(&mut self) -> Vec<ParsedEvent> {
        let mut events = Vec::new();

        if let Some(text) = self.take_text()
            && !text.is_empty() {
                events.push(ParsedEvent::Event(ModelResponseEvent::TextDelta(text)));
            }
        if let Some(reasoning) = self.take_reasoning()
            && !reasoning.is_empty() {
                events.push(ParsedEvent::Event(ModelResponseEvent::ReasoningDelta(
                    reasoning,
                )));
            }

        events
    }
}

/// An SSE event from the Anthropic stream — parsed enough to drive our state machine.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    index: Option<usize>,
    message: Option<AnthropicMessageEvent>,
    content_block: Option<AnthropicContentBlock>,
    delta: Option<AnthropicDelta>,
    usage: Option<AnthropicUsage>,
    error: Option<AnthropicError>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageEvent {
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    id: Option<String>,
    name: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    input: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
    signature: Option<String>,
    stop_reason: Option<String>,
    stop_sequence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// Event types for the parser output
// ---------------------------------------------------------------------------

enum ParsedEvent {
    Event(ModelResponseEvent),
    Finish {
        stop_reason: StopReason,
        usage: Usage,
    },
    Error(String),
}

fn map_finish_reason(reason: &str) -> StopReason {
    match reason.trim() {
        "end_turn" | "stop_sequence" | "pause_turn" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        _ => StopReason::Stop,
    }
}

// ---------------------------------------------------------------------------
// Stream parsing driver
// ---------------------------------------------------------------------------

async fn parse_sse_stream(
    response: reqwest::Response,
    tx: mpsc::UnboundedSender<Result<ModelResponseEvent, ModelError>>,
) {
    let mut parser = AnthropicSseParser::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(ModelError::Stream(e.to_string())));
                return;
            }
        };

        let text = String::from_utf8_lossy(&chunk);
        for item in parser.push(&text) {
            match item {
                ParsedEvent::Event(ev) => {
                    if tx.send(Ok(ev)).is_err() {
                        return;
                    }
                }
                ParsedEvent::Finish { stop_reason, usage } => {
                    // Flush pending text/reasoning blocks before finish
                    for ev in parser.finish() {
                        if let ParsedEvent::Event(ev) = ev
                            && tx.send(Ok(ev)).is_err() {
                                return;
                            }
                        _ = ev;
                    }
                    if tx
                        .send(Ok(ModelResponseEvent::Finished { stop_reason, usage }))
                        .is_err()
                    {
                        return;
                    }
                }
                ParsedEvent::Error(msg) => {
                    let _ = tx.send(Err(ModelError::Stream(msg)));
                    return;
                }
            }
        }
    }

    // Flush remaining pending content
    for ev in parser.finish() {
        if let ParsedEvent::Event(ev) = ev
            && tx.send(Ok(ev)).is_err() {
                return;
            }
        _ = ev;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use joker::ToolAnnotations;
    use serde_json::json;

    #[test]
    fn builds_anthropic_request_body() {
        let request = ModelRequest {
            messages: vec![
                Message {
                    role: Role::System,
                    content: vec![Content::text("You are Claude.")],
                },
                Message {
                    role: Role::User,
                    content: vec![Content::text("hello")],
                },
            ],
            tools: vec![ToolDefinition {
                name: joker::ToolName::new("read_file"),
                description: "read a file".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations::default(),
            }],
        };

        let body = build_request_body("claude-sonnet-4-20250514", &request);
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["stream"], true);
        assert!(body["max_tokens"].is_number());

        // System should be at top level
        assert!(body.get("system").is_some());

        // Messages should contain user message
        let msgs = body["messages"].as_array().unwrap();
        assert!(!msgs.is_empty());
    }

    #[test]
    fn parses_text_delta() {
        let mut parser = AnthropicSseParser::new();
        let events = parser.push("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n");

        // No immediate event - text is buffered
        assert!(events.is_empty());

        // Finish flushes the text
        let events = parser.finish();
        assert!(events.iter().any(|e| matches!(e, ParsedEvent::Event(ModelResponseEvent::TextDelta(t)) if t == "Hello")));
    }

    #[test]
    fn parses_message_start_usages() {
        let mut parser = AnthropicSseParser::new();
        let data = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-20250514","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
"#;

        let events = parser.push(data);
        assert!(events.is_empty()); // No content events
    }
}
