//! OpenAI-compatible chat completions provider.
//!
//! Implements the [`Model`](joker::Model) trait for any provider that speaks the
//! OpenAI `/v1/chat/completions` wire format. Includes SSE stream parsing and
//! tool-call streaming.

use std::{
    collections::BTreeMap,
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Stream;
use futures_util::StreamExt;
use joker::{
    Content, Message, Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent,
    ModelStream, Role, StopReason, ToolCall, ToolDefinition, ToolResult, Usage,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{error, info, trace, warn};

use crate::sse::{SseEvent, SseTokenizer};
use crate::transform;

/// Configuration for an OpenAI-compatible provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCompatibleConfig {
    /// Human-readable provider name (used in error messages and defaults).
    pub provider_name: String,
    /// Base URL of the provider API (without the `/chat/completions` suffix).
    pub base_url: String,
    /// Model identifier sent in the request body.
    pub model: String,
    /// Optional in-memory API key.
    pub api_key: Option<String>,
    /// Optional environment variable name that holds the API key.
    pub api_key_env: Option<String>,
    /// Whether a non-empty API key is required to construct the client.
    pub require_api_key: bool,
    /// Additional JSON body fields merged into the request (e.g. `enable_thinking`, `top_p`).
    pub extra_body: Option<serde_json::Value>,
    /// Whether the model produces reasoning content (drives reasoning-block
    /// normalization); `None` falls back to model-name heuristics.
    pub reasoning: Option<bool>,
    /// HTTP headers merged into every request, including the resolved
    /// authorization header from the route's auth scheme.
    pub headers: Vec<(String, String)>,
}

impl OpenAiCompatibleConfig {
    /// Build the full `/chat/completions` URL from [`base_url`](OpenAiCompatibleConfig::base_url).
    #[must_use]
    pub fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    /// Compute the effective extra body, merging provider-specific defaults with user overrides.
    #[must_use]
    pub fn effective_extra_body(&self) -> Option<Value> {
        let mut defaults = match self.provider_name.to_lowercase().as_str() {
            "alibaba" => provider_defaults_alibaba(&self.model),
            "zhipuai" => provider_defaults_zhipuai(&self.model),
            _ => None,
        };

        match (defaults.as_mut(), self.extra_body.as_ref()) {
            (Some(d), Some(user)) => {
                if let (Value::Object(d_map), Value::Object(u_map)) = (d, user) {
                    for (k, v) in u_map {
                        d_map.insert(k.clone(), v.clone());
                    }
                }
                defaults
            }
            (None, Some(user)) => Some(user.clone()),
            _ => defaults,
        }
    }
}

fn provider_defaults_alibaba(model: &str) -> Option<Value> {
    let ml = model.to_lowercase();
    if ml.contains("qwq") || ml.contains("kimi-k2") || ml.contains("deepseek-r1") {
        Some(json!({"enable_thinking": true}))
    } else {
        None
    }
}

fn provider_defaults_zhipuai(model: &str) -> Option<Value> {
    let ml = model.to_lowercase();
    if ml.contains("glm") {
        Some(json!({"thinking": {"type": "enabled", "clear_thinking": false}}))
    } else {
        None
    }
}

/// Errors specific to the OpenAI-compatible provider.
#[derive(Debug, Error)]
pub enum OpenAiProviderError {
    /// The provider requires an API key but none was provided.
    #[error("{provider} API key is missing; set {env}")]
    MissingApiKey {
        /// Provider name shown in the error message.
        provider: String,
        /// Environment variable name shown in the error message.
        env: String,
    },
    /// The API key could not be parsed as a valid HTTP header value.
    #[error("invalid authorization header")]
    InvalidAuthorizationHeader,
    /// A configured header name is not a valid HTTP header name.
    #[error("invalid header name: {0}")]
    InvalidHeader(String),
}

/// A model backed by an OpenAI-compatible chat completions endpoint.
#[derive(Clone, Debug)]
pub struct OpenAiCompatibleModel {
    config: OpenAiCompatibleConfig,
    client: reqwest::Client,
}

impl OpenAiCompatibleModel {
    /// Create a new [`OpenAiCompatibleModel`].
    ///
    /// Validates the API key (if required) and builds the HTTP client with
    /// default headers. Returns an error if the key is missing or malformed.
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, OpenAiProviderError> {
        if config.require_api_key && config.api_key.as_deref().unwrap_or_default().is_empty() {
            return Err(OpenAiProviderError::MissingApiKey {
                provider: config.provider_name,
                env: config.api_key_env.unwrap_or_else(|| "API_KEY".into()),
            });
        }

        let mut headers = HeaderMap::new();
        for (name, value) in &config.headers {
            let parsed = name
                .parse::<HeaderName>()
                .map_err(|_| OpenAiProviderError::InvalidHeader(name.clone()))?;
            headers.insert(
                parsed,
                HeaderValue::from_str(value)
                    .map_err(|_| OpenAiProviderError::InvalidAuthorizationHeader)?,
            );
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("reqwest client with static configuration should build");

        Ok(Self { config, client })
    }

    /// Fetch available models from the provider's `/v1/models` endpoint.
    ///
    /// Returns model IDs sorted alphabetically. Falls back to the configured
    /// model name if the endpoint is unreachable.
    pub async fn detect_models(&self) -> Vec<String> {
        let models_url = format!("{}/models", self.config.base_url.trim_end_matches('/'));
        match self.client.get(&models_url).send().await {
            Ok(response) => match response.json::<Value>().await {
                Ok(body) => {
                    if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                        let mut ids: Vec<String> = data
                            .iter()
                            .filter_map(|entry| entry.get("id").and_then(|id| id.as_str()))
                            .filter(|id| !id.contains("embed") && !id.contains("moderation"))
                            .map(String::from)
                            .collect();
                        ids.sort();
                        if !ids.is_empty() {
                            return ids;
                        }
                    }
                    vec![self.config.model.clone()]
                }
                Err(_) => vec![self.config.model.clone()],
            },
            Err(_) => vec![self.config.model.clone()],
        }
    }
}

impl Model for OpenAiCompatibleModel {
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_> {
        let config_model = self.config.model.clone();
        let client = self.client.clone();
        let url = self.config.chat_url();
        let extra = self.config.effective_extra_body();
        let provider_name = self.config.provider_name.clone();
        let reasoning = self.config.reasoning;

        Box::pin(async move {
            info!(target: "provider", provider = %provider_name, model = %config_model, "streaming request");
            let body = chat_request_body(&config_model, &request, extra.as_ref(), reasoning);
            let mut last_error = None;
            for attempt in 1..=2 {
                match client.post(&url).json(&body).send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            let status = response.status().as_u16();
                            let body = response.text().await.unwrap_or_default();
                            let kind = crate::classify_error(status, &body, &provider_name);
                            return Err(ModelError::Classified {
                                kind,
                                message: format!(
                                    "{provider_name} request failed with {status}: {body}"
                                ),
                            });
                        }
                        let (tx, rx) = mpsc::unbounded_channel();
                        let provider = provider_name.clone();
                        tokio::spawn(async move {
                            parse_response_stream(response, tx, &provider).await;
                        });
                        return Ok(Box::new(ReceiverModelStream { rx }) as ModelStream);
                    }
                    Err(error) => {
                        warn!(target: "provider", provider = %provider_name, attempt, max_attempts = 2, error = %error, "request failed, retrying");
                        last_error = Some(error);
                    }
                }
            }
            Err(ModelError::Stream(last_error.unwrap().to_string()))
        })
    }
}

struct ReceiverModelStream {
    rx: mpsc::UnboundedReceiver<Result<ModelResponseEvent, ModelError>>,
}

impl Stream for ReceiverModelStream {
    type Item = Result<ModelResponseEvent, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

async fn parse_response_stream(
    response: reqwest::Response,
    tx: mpsc::UnboundedSender<Result<ModelResponseEvent, ModelError>>,
    provider: &str,
) {
    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                error!(target: "provider", provider, error = %error, "stream chunk error");
                let _ = tx.send(Err(ModelError::Stream(error.to_string())));
                return;
            }
        };

        for item in parser.push(&String::from_utf8_lossy(&chunk)) {
            match item {
                ParsedSse::Done => return,
                ParsedSse::Event(event) => {
                    trace!(target: "provider", provider, event = ?event, "stream event");
                    if tx.send(Ok(event)).is_err() {
                        return;
                    }
                }
                ParsedSse::Error(error) => {
                    error!(target: "provider", provider, error = %error, "stream parse error");
                    let _ = tx.send(Err(ModelError::Stream(error)));
                    return;
                }
            }
        }
    }

    for item in parser.finish() {
        match item {
            ParsedSse::Done => return,
            ParsedSse::Event(event) => {
                trace!(target: "provider", provider, event = ?event, "stream event");
                if tx.send(Ok(event)).is_err() {
                    return;
                }
            }
            ParsedSse::Error(error) => {
                error!(target: "provider", provider, error = %error, "stream parse error");
                let _ = tx.send(Err(ModelError::Stream(error)));
                return;
            }
        }
    }
}

#[derive(Default)]
struct SseParser {
    tokenizer: SseTokenizer,
    tool_calls: BTreeMap<u64, PartialToolCall>,
    usage: Usage,
}

impl SseParser {
    fn push(&mut self, chunk: &str) -> Vec<ParsedSse> {
        self.tokenizer
            .push(chunk)
            .into_iter()
            .flat_map(|event| self.parse_event(&event))
            .collect()
    }

    fn finish(&mut self) -> Vec<ParsedSse> {
        self.tokenizer
            .finish()
            .into_iter()
            .flat_map(|event| self.parse_event(&event))
            .collect()
    }

    fn parse_event(&mut self, event: &SseEvent) -> Vec<ParsedSse> {
        let data = event.data.trim();
        if data.is_empty() {
            return Vec::new();
        }
        if data == "[DONE]" {
            return vec![ParsedSse::Done];
        }
        parse_chat_chunk(data, &mut self.tool_calls, &mut self.usage)
    }
}

enum ParsedSse {
    Event(ModelResponseEvent),
    Done,
    Error(String),
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

fn parse_chat_chunk(
    data: &str,
    tool_calls: &mut BTreeMap<u64, PartialToolCall>,
    usage: &mut Usage,
) -> Vec<ParsedSse> {
    let chunk = match serde_json::from_str::<ChatChunk>(data) {
        Ok(chunk) => chunk,
        Err(error) => return vec![ParsedSse::Error(format!("invalid SSE JSON: {error}: {data}"))],
    };

    if let Some(chunk_usage) = chunk.usage {
        usage.input_tokens = chunk_usage.prompt_tokens.unwrap_or(usage.input_tokens);
        usage.output_tokens = chunk_usage.completion_tokens.unwrap_or(usage.output_tokens);
    }

    let Some(choice) = chunk.choices.into_iter().next() else {
        // Usage-only trailing chunk (the stream_options include_usage final event).
        return Vec::new();
    };

    let mut events = Vec::new();

    if let Some(delta) = choice.delta {
        if let Some(text) = delta.content.filter(|value| !value.is_empty()) {
            events.push(ParsedSse::Event(ModelResponseEvent::TextDelta(text)));
        } else if let Some(text) = delta.reasoning_content.filter(|value| !value.is_empty()) {
            events.push(ParsedSse::Event(ModelResponseEvent::ReasoningDelta(text)));
        }
        if let Some(calls) = delta.tool_calls {
            let mut completed = Vec::new();
            for call in calls {
                let entry = tool_calls.entry(call.index).or_default();
                if let Some(id) = call.id {
                    entry.id = id;
                }
                if let Some(function) = call.function {
                    if let Some(name) = function.name {
                        entry.name = name;
                    }
                    if let Some(arguments) = function.arguments {
                        entry.arguments.push_str(&arguments);
                    }
                }
                if !entry.id.is_empty()
                    && !entry.name.is_empty()
                    && is_complete_json(&entry.arguments)
                {
                    completed.push(call.index);
                }
            }
            for index in completed {
                let call = tool_calls.remove(&index).expect("completed key exists");
                let arguments =
                    serde_json::from_str(&call.arguments).unwrap_or(Value::String(call.arguments));
                events.push(ParsedSse::Event(ModelResponseEvent::ToolCall(ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments,
                })));
            }
        }
    }

    if let Some(reason) = choice.finish_reason {
        events.push(ParsedSse::Event(ModelResponseEvent::Finished {
            stop_reason: map_finish_reason(&reason),
            usage: std::mem::take(usage),
        }));
    }

    events
}

fn is_complete_json(value: &str) -> bool {
    !value.trim().is_empty() && serde_json::from_str::<Value>(value).is_ok()
}

fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" => StopReason::Length,
        _ => StopReason::Stop,
    }
}

/// Build the JSON request body for a chat completions call.
///
/// Requests `stream_options.include_usage` so the final stream chunk carries
/// token usage. `reasoning` (`None` when the model is unknown) drives
/// reasoning-block normalization via [`transform::normalize_messages`].
pub fn chat_request_body(
    model: &str,
    request: &ModelRequest,
    extra_body: Option<&Value>,
    reasoning: Option<bool>,
) -> Value {
    let messages = transform::normalize_messages(&request.messages, "openai", model, reasoning);
    let mut body = json!({
        "model": model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages.iter().map(openai_message).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(openai_tool).collect::<Vec<_>>(),
    });

    if let Some(extra) = extra_body
        && let Value::Object(extra_map) = extra
        && let Value::Object(ref mut body_map) = body
    {
        for (k, v) in extra_map {
            body_map.insert(k.clone(), v.clone());
        }
    }

    body
}

fn openai_message(message: &Message) -> Value {
    match message.role {
        Role::System => json!({"role": "system", "content": text_content(&message.content)}),
        Role::User => json!({"role": "user", "content": text_content(&message.content)}),
        Role::Assistant => {
            let tool_calls = message
                .content
                .iter()
                .filter_map(|content| match content {
                    Content::ToolCall(call) => Some(json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments.to_string(),
                        }
                    })),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let content = text_content(&message.content);
            if tool_calls.is_empty() {
                json!({"role": "assistant", "content": content})
            } else {
                json!({"role": "assistant", "content": content, "tool_calls": tool_calls})
            }
        }
        Role::Tool => {
            let result = message.content.iter().find_map(|content| match content {
                Content::ToolResult(result) => Some(result),
                _ => None,
            });
            match result {
                Some(result) => openai_tool_result(result),
                None => json!({"role": "tool", "content": ""}),
            }
        }
        _ => json!({"role": "user", "content": text_content(&message.content)}),
    }
}

fn openai_tool_result(result: &ToolResult) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": result.call_id,
        "name": result.name,
        "content": result.output.to_string(),
    })
}

fn text_content(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|content| match content {
            Content::Text(text) => Some(text.text.as_str()),
            Content::Reasoning(reasoning) => Some(reasoning.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn openai_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name.to_string(),
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    delta: Option<ChatDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ChatToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    index: u64,
    id: Option<String>,
    function: Option<ChatFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use joker::{Conversation, Message, ToolAnnotations, ToolName};

    #[test]
    fn serializes_chat_request_with_tools() {
        let request = ModelRequest {
            messages: vec![Message::user("hello")],
            tools: vec![ToolDefinition {
                name: ToolName::new("read_file"),
                description: "read a file".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations::default(),
            }],
        };

        let body = chat_request_body("deepseek-v4-flash", &request, None, None);

        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn parses_text_and_finish_chunks() {
        let mut parser = SseParser::default();
        let events = parser.push(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
             data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n\
             data: [DONE]\n\n",
        );

        assert!(matches!(
            events[0],
            ParsedSse::Event(ModelResponseEvent::TextDelta(ref text)) if text == "hi"
        ));
        assert!(matches!(
            events[1],
            ParsedSse::Event(ModelResponseEvent::Finished {
                stop_reason: StopReason::Stop,
                ..
            })
        ));
    }

    #[test]
    fn parses_streamed_tool_call_arguments() {
        let mut parser = SseParser::default();
        let events = parser.push(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"echo\",\"arguments\":\"{\\\"text\\\":\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"hi\\\"}\"}}]}}]}\n\n",
        );

        assert!(matches!(
            events.last(),
            Some(ParsedSse::Event(ModelResponseEvent::ToolCall(call)))
                if call.id == "call-1" && call.name == "echo" && call.arguments == json!({"text":"hi"})
        ));
    }

    #[test]
    fn emits_all_completed_tool_calls_in_one_chunk() {
        let mut parser = SseParser::default();
        let events = parser.push(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-a\",\"function\":{\"name\":\"tool_a\",\"arguments\":\"{}\"}},{\"index\":1,\"id\":\"call-b\",\"function\":{\"name\":\"tool_b\",\"arguments\":\"{}\"}}]}}]}\n\n",
        );

        let calls: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ParsedSse::Event(ModelResponseEvent::ToolCall(call)) => Some(call.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, vec!["tool_a", "tool_b"]);
    }

    #[test]
    fn captures_usage_from_usage_chunk() {
        let mut parser = SseParser::default();
        let events = parser.push(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
             data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        );

        let finished = events.iter().find_map(|event| match event {
            ParsedSse::Event(ModelResponseEvent::Finished { usage, .. }) => Some(usage),
            _ => None,
        });
        assert_eq!(finished.expect("finished event").input_tokens, 12);
        assert_eq!(finished.expect("finished event").output_tokens, 3);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ParsedSse::Event(ModelResponseEvent::TextDelta(t)) if t == "hi"))
        );
    }

    #[test]
    fn ignores_role_only_delta_chunk() {
        let mut parser = SseParser::default();
        let events =
            parser.push("data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n");
        assert!(events.is_empty(), "role-only chunk is a no-op, not an error");
    }

    #[test]
    fn serializes_tool_result_message() {
        let conversation = Conversation::from_messages(vec![Message::tool(vec![ToolResult::ok(
            "call-1",
            "echo",
            json!({"text":"hi"}),
        )])]);
        let message = openai_message(&conversation.messages()[0]);

        assert_eq!(message["role"], "tool");
        assert_eq!(message["tool_call_id"], "call-1");
        assert_eq!(message["name"], "echo");
    }

    #[test]
    fn required_api_key_reports_env_name() {
        let error = OpenAiCompatibleModel::new(OpenAiCompatibleConfig {
            provider_name: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-flash".into(),
            api_key: None,
            api_key_env: Some("DEEPSEEK_API_KEY".into()),
            require_api_key: true,
            extra_body: None,
            reasoning: None,
            headers: vec![],
        })
        .unwrap_err();

        assert!(error.to_string().contains("DEEPSEEK_API_KEY"));
    }

    #[test]
    fn custom_endpoint_can_run_without_api_key() {
        let model = OpenAiCompatibleModel::new(OpenAiCompatibleConfig {
            provider_name: "local".into(),
            base_url: "http://localhost:8000/v1".into(),
            model: "local-model".into(),
            api_key: None,
            api_key_env: None,
            require_api_key: false,
            extra_body: None,
            reasoning: None,
            headers: vec![],
        });

        assert!(model.is_ok());
    }
}
