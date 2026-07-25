//! Google Gemini API provider.
//!
//! Ported from opencode's `gemini.ts` protocol implementation.
//! Supports streaming text, thinking, tool calls, and usage via the
//! Gemini `streamGenerateContent` SSE endpoint.

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
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
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::transform;

/// Default base URL for the Gemini API.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// API version prefix.
pub const API_VERSION: &str = "v1beta";

/// Configuration for the Google/Gemini provider.
#[derive(Clone, Debug)]
pub struct GoogleConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl GoogleConfig {
    fn stream_url(&self) -> String {
        format!(
            "{}/{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/'),
            API_VERSION,
            self.model,
        )
    }
}

/// Errors specific to the Google provider.
#[derive(Debug, Error)]
pub enum GoogleProviderError {
    #[error("Google API key is missing; set GOOGLE_GENERATIVE_AI_API_KEY")]
    MissingApiKey,
    #[error("invalid authorization header")]
    InvalidAuthHeader,
    #[error("google api error: {0}")]
    Api(String),
}

/// A model backed by the Google Gemini API.
#[derive(Clone, Debug)]
pub struct GoogleModel {
    config: GoogleConfig,
    client: reqwest::Client,
}

impl GoogleModel {
    /// Create a new `GoogleModel`.
    pub fn new(config: GoogleConfig) -> Result<Self, GoogleProviderError> {
        if config.api_key.is_empty() {
            return Err(GoogleProviderError::MissingApiKey);
        }

        let mut headers = HeaderMap::new();
        let mut auth_header = HeaderValue::from_str(&config.api_key)
            .map_err(|_| GoogleProviderError::InvalidAuthHeader)?;
        auth_header.set_sensitive(true);
        headers.insert("x-goog-api-key", auth_header);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("reqwest client should build");

        Ok(Self { config, client })
    }
}

impl Model for GoogleModel {
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_> {
        let config = self.config.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let body = build_request_body(&config.model, &request);
            let response = client
                .post(config.stream_url())
                .json(&body)
                .send()
                .await
                .map_err(|e| ModelError::Stream(e.to_string()))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(ModelError::Stream(format!(
                    "Google request failed with {status}: {body}"
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
// Request body construction (port of Gemini protocol)
// ---------------------------------------------------------------------------

fn build_request_body(model: &str, request: &ModelRequest) -> Value {
    let msgs = transform::normalize_messages(&request.messages, "google", model);
    let contents = build_contents(&msgs);
    let system_instruction = build_system_instruction(&msgs);
    let tools = build_tools(&request.tools);

    let mut body = json!({
        "contents": contents,
    });

    if let Some(si) = system_instruction {
        body["systemInstruction"] = si;
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }

    body
}

fn gemini_role(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant | Role::Tool => "model",
        Role::System => "user",
        _ => "user",
    }
}

fn build_contents(messages: &[Message]) -> Vec<Value> {
    let mut contents: Vec<Value> = Vec::new();

    for msg in messages {
        if msg.role == Role::System {
            continue;
        }

        let mut parts: Vec<Value> = Vec::new();
        for content in &msg.content {
            match content {
                Content::Text(t) => {
                    parts.push(json!({"text": t.text}));
                }
                Content::Reasoning(r) => {
                    // Gemini uses `thought: true` for thinking/reasoning blocks
                    parts.push(json!({"text": r.text, "thought": true}));
                }
                Content::ToolCall(tc) => {
                    parts.push(json!({
                        "functionCall": {
                            "name": tc.name,
                            "args": tc.arguments,
                        }
                    }));
                }
                Content::ToolResult(tr) => {
                    parts.push(json!({
                        "functionResponse": {
                            "name": tr.name,
                            "response": {
                                "name": tr.name,
                                "content": tr.output,
                            },
                        }
                    }));
                }
                _ => {}
            }
        }

        if !parts.is_empty() {
            contents.push(json!({
                "role": gemini_role(&msg.role),
                "parts": parts,
            }));
        }
    }

    contents
}

fn build_system_instruction(messages: &[Message]) -> Option<Value> {
    let text: String = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .flat_map(|m| &m.content)
        .filter_map(|c| {
            if let Content::Text(t) = c {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if text.is_empty() {
        None
    } else {
        Some(json!({
            "parts": [{"text": text}]
        }))
    }
}

fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    if tools.is_empty() {
        return vec![];
    }

    vec![json!({
        "functionDeclarations": tools.iter().map(|t| {
            json!({
                "name": t.name.as_str(),
                "description": t.description,
                "parameters": t.input_schema,
            })
        }).collect::<Vec<_>>()
    })]
}

// ---------------------------------------------------------------------------
// SSE stream parser
// ---------------------------------------------------------------------------

/// Parsed chunk of Gemini streaming response.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiResponse {
    candidates: Option<Vec<Candidate>>,
    usage_metadata: Option<UsageMetadata>,
    #[serde(rename = "error")]
    error: Option<GeminiError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Candidate {
    content: Option<CandidateContent>,
    finish_reason: Option<String>,
    index: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CandidateContent {
    parts: Option<Vec<Part>>,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Part {
    #[serde(rename = "text")]
    text: Option<String>,
    #[serde(rename = "thought")]
    thought: Option<bool>,
    #[serde(rename = "functionCall")]
    function_call: Option<FunctionCall>,
    #[serde(rename = "functionResponse")]
    function_response: Option<FunctionResponse>,
}

#[derive(Debug, Deserialize)]
struct FunctionCall {
    name: Option<String>,
    args: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FunctionResponse {
    name: Option<String>,
    response: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UsageMetadata {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
    total_token_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiError {
    code: Option<u64>,
    message: Option<String>,
    status: Option<String>,
}

// ---------------------------------------------------------------------------
// SSE parser
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct GeminiSseParser {
    buffer: String,
}

impl GeminiSseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    fn push(&mut self, chunk: &str) -> Vec<ParsedEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(delim) = self.buffer.find("\n\n") {
            let raw = self.buffer[..delim].to_string();
            self.buffer.drain(..=delim + 1);

            // Gemini SSE uses `data: {...}` lines
            for line in raw.lines() {
                let data = line.trim();
                if let Some(json_str) = data.strip_prefix("data:") {
                    let json_str = json_str.trim();
                    if json_str.is_empty() || json_str == "{}" {
                        continue;
                    }
                    match self.parse_chunk(json_str) {
                        Ok(Some(ev)) => events.push(ev),
                        Ok(None) => {}
                        Err(e) => {
                            events.push(ParsedEvent::Error(e));
                            return events;
                        }
                    }
                }
            }
        }

        events
    }

    fn parse_chunk(&mut self, json_str: &str) -> Result<Option<ParsedEvent>, String> {
        let response: GeminiResponse =
            serde_json::from_str(json_str).map_err(|e| format!("invalid Gemini JSON: {e}: {json_str}"))?;

        // Check for error
        if let Some(err) = &response.error {
            let msg = err
                .message
                .clone()
                .unwrap_or_else(|| "unknown gemini error".into());
            return Err(msg);
        }

        let Some(candidates) = &response.candidates else {
            return Ok(None);
        };

        let Some(candidate) = candidates.first() else {
            return Ok(None);
        };

        let mut events = Vec::new();

        // Process parts
        if let Some(content) = &candidate.content
            && let Some(parts) = &content.parts {
                for part in parts {
                    if let Some(text) = &part.text
                        && !text.is_empty() {
                            if part.thought.unwrap_or(false) {
                                events.push(ParsedEvent::Event(ModelResponseEvent::ReasoningDelta(
                                    text.clone(),
                                )));
                            } else {
                                events.push(ParsedEvent::Event(ModelResponseEvent::TextDelta(
                                    text.clone(),
                                )));
                            }
                        }

                    if let Some(fc) = &part.function_call {
                        let name = fc.name.clone().unwrap_or_default();
                        let args = fc.args.clone().unwrap_or(Value::Null);
                        events.push(ParsedEvent::Event(ModelResponseEvent::ToolCall(
                            ToolCall {
                                id: format!("fc_{}", name),
                                name,
                                arguments: args,
                            },
                        )));
                    }
                }
            }

        // Check finish reason
        if let Some(finish_reason) = &candidate.finish_reason {
            let stop_reason = map_finish_reason(finish_reason);
            let usage = response.usage_metadata.as_ref().map(|u| Usage {
                input_tokens: u.prompt_token_count.unwrap_or(0),
                output_tokens: u.candidates_token_count.unwrap_or(0),
            });

            events.push(ParsedEvent::Finish {
                stop_reason,
                usage: usage.unwrap_or_default(),
            });
        }

        Ok(events.into_iter().next())
    }

    #[allow(dead_code)]
    fn finish(&mut self) -> Vec<ParsedEvent> {
        // Nothing to flush for Google (all content is event-based)
        vec![]
    }
}

enum ParsedEvent {
    Event(ModelResponseEvent),
    Finish {
        stop_reason: StopReason,
        usage: Usage,
    },
    Error(String),
}

fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        "FUNCTION_CALL" | "TOOL_CALL" => StopReason::ToolUse,
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
    let mut parser = GeminiSseParser::new();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use joker::{ToolAnnotations, ToolName};

    #[test]
    fn builds_google_request_body() {
        let request = ModelRequest {
            messages: vec![
                Message {
                    role: Role::System,
                    content: vec![Content::text("You are Gemini.")],
                },
                Message::user("hello"),
            ],
            tools: vec![ToolDefinition {
                name: ToolName::new("read_file"),
                description: "read a file".into(),
                input_schema: json!({"type": "object"}),
                annotations: ToolAnnotations::default(),
            }],
        };

        let body = build_request_body("gemini-2-5-flash", &request);
        assert!(body.get("contents").is_some());
        assert!(body.get("systemInstruction").is_some());
        assert!(body.get("tools").is_some());
    }

    #[test]
    fn parses_text_chunk() {
        let mut parser = GeminiSseParser::new();
        let events = parser.push("data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}\n\n");

        assert!(events
            .iter()
            .any(|e| matches!(e, ParsedEvent::Event(ModelResponseEvent::TextDelta(t)) if t == "Hello")));
    }

    #[test]
    fn parses_tool_call() {
        let mut parser = GeminiSseParser::new();
        let events = parser.push("data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"/tmp/test\"}}}],\"role\":\"model\"},\"finishReason\":\"FUNCTION_CALL\"}]}\n\n");

        assert!(events.iter().any(|e| matches!(e, ParsedEvent::Event(ModelResponseEvent::ToolCall(tc)) if tc.name == "read_file")));
    }

    #[test]
    fn parses_thought_part() {
        let mut parser = GeminiSseParser::new();
        let events = parser.push("data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Let me think...\",\"thought\":true}]}}]}\n\n");

        assert!(events
            .iter()
            .any(|e| matches!(e, ParsedEvent::Event(ModelResponseEvent::ReasoningDelta(t)) if t == "Let me think...")));
    }
}
