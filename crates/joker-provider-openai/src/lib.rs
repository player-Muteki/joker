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
    ModelStream, Role, StopReason, ToolCall, ToolDefinition, ToolResult, Usage,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCompatibleConfig {
    pub provider_name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub require_api_key: bool,
}

impl OpenAiCompatibleConfig {
    #[must_use]
    pub fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[derive(Debug, Error)]
pub enum OpenAiProviderError {
    #[error("{provider} API key is missing; set {env}")]
    MissingApiKey { provider: String, env: String },
    #[error("invalid authorization header")]
    InvalidAuthorizationHeader,
}

#[derive(Clone, Debug)]
pub struct OpenAiCompatibleModel {
    config: OpenAiCompatibleConfig,
    client: reqwest::Client,
}

impl OpenAiCompatibleModel {
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, OpenAiProviderError> {
        if config.require_api_key && config.api_key.as_deref().unwrap_or_default().is_empty() {
            return Err(OpenAiProviderError::MissingApiKey {
                provider: config.provider_name,
                env: config.api_key_env.unwrap_or_else(|| "API_KEY".into()),
            });
        }

        let mut headers = HeaderMap::new();
        if let Some(api_key) = config.api_key.as_deref().filter(|value| !value.is_empty()) {
            let auth_value = format!("Bearer {api_key}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth_value)
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
}

impl Model for OpenAiCompatibleModel {
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_> {
        Box::pin(async move {
            let body = chat_request_body(&self.config.model, &request);
            let response = self
                .client
                .post(self.config.chat_url())
                .json(&body)
                .send()
                .await
                .map_err(|error| ModelError::Stream(error.to_string()))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(ModelError::Stream(format!(
                    "{} request failed with {status}: {body}",
                    self.config.provider_name
                )));
            }

            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(parse_response_stream(response, tx));
            Ok(Box::new(ReceiverModelStream { rx }) as ModelStream)
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
) {
    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = tx.send(Err(ModelError::Stream(error.to_string())));
                return;
            }
        };

        for item in parser.push(&String::from_utf8_lossy(&chunk)) {
            match item {
                ParsedSse::Done => return,
                ParsedSse::Event(event) => {
                    if tx.send(Ok(event)).is_err() {
                        return;
                    }
                }
                ParsedSse::Error(error) => {
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
                if tx.send(Ok(event)).is_err() {
                    return;
                }
            }
            ParsedSse::Error(error) => {
                let _ = tx.send(Err(ModelError::Stream(error)));
                return;
            }
        }
    }
}

#[derive(Default)]
struct SseParser {
    pending: String,
    tool_calls: BTreeMap<u64, PartialToolCall>,
}

impl SseParser {
    fn push(&mut self, chunk: &str) -> Vec<ParsedSse> {
        self.pending.push_str(chunk);
        let mut parsed = Vec::new();

        while let Some(index) = self.pending.find('\n') {
            let mut line = self.pending.drain(..=index).collect::<String>();
            line.truncate(line.trim_end_matches(['\r', '\n']).len());
            if let Some(item) = self.parse_line(&line) {
                parsed.push(item);
            }
        }

        parsed
    }

    fn finish(&mut self) -> Vec<ParsedSse> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let line = std::mem::take(&mut self.pending);
        self.parse_line(&line).into_iter().collect()
    }

    fn parse_line(&mut self, line: &str) -> Option<ParsedSse> {
        let data = line.strip_prefix("data:")?.trim();
        if data.is_empty() {
            return None;
        }
        if data == "[DONE]" {
            return Some(ParsedSse::Done);
        }
        Some(parse_chat_chunk(data, &mut self.tool_calls))
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

fn parse_chat_chunk(data: &str, tool_calls: &mut BTreeMap<u64, PartialToolCall>) -> ParsedSse {
    let chunk = match serde_json::from_str::<ChatChunk>(data) {
        Ok(chunk) => chunk,
        Err(error) => return ParsedSse::Error(format!("invalid SSE JSON: {error}: {data}")),
    };
    let Some(choice) = chunk.choices.into_iter().next() else {
        return ParsedSse::Error("chat chunk had no choices".into());
    };

    if let Some(delta) = choice.delta {
        if let Some(text) = delta.content.filter(|value| !value.is_empty()) {
            return ParsedSse::Event(ModelResponseEvent::TextDelta(text));
        }
        if let Some(text) = delta.reasoning_content.filter(|value| !value.is_empty()) {
            return ParsedSse::Event(ModelResponseEvent::ReasoningDelta(text));
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
            if let Some(index) = completed.into_iter().next() {
                let call = tool_calls.remove(&index).expect("completed key exists");
                let arguments =
                    serde_json::from_str(&call.arguments).unwrap_or(Value::String(call.arguments));
                return ParsedSse::Event(ModelResponseEvent::ToolCall(ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments,
                }));
            }
        }
    }

    if let Some(reason) = choice.finish_reason {
        return ParsedSse::Event(ModelResponseEvent::Finished {
            stop_reason: map_finish_reason(&reason),
            usage: Usage::default(),
        });
    }

    ParsedSse::Error("chat chunk had no supported delta or finish reason".into())
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

pub fn chat_request_body(model: &str, request: &ModelRequest) -> Value {
    json!({
        "model": model,
        "stream": true,
        "messages": request.messages.iter().map(openai_message).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(openai_tool).collect::<Vec<_>>(),
    })
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

#[derive(Debug, Serialize)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_key_env: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
}

pub const DEEPSEEK: ProviderDescriptor = ProviderDescriptor {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com",
    api_key_env: "DEEPSEEK_API_KEY",
    default_model: "deepseek-v4-flash",
    models: &["deepseek-v4-flash", "deepseek-v4-pro"],
};

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

        let body = chat_request_body("deepseek-v4-flash", &request);

        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn parses_text_and_finish_chunks() {
        let mut parser = SseParser::default();
        let events = parser.push(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\
             data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\
             data: [DONE]\n",
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
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"echo\",\"arguments\":\"{\\\"text\\\":\"}}]}}]}\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"hi\\\"}\"}}]}}]}\n",
        );

        assert!(matches!(
            events.last(),
            Some(ParsedSse::Event(ModelResponseEvent::ToolCall(call)))
                if call.id == "call-1" && call.name == "echo" && call.arguments == json!({"text":"hi"})
        ));
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
        });

        assert!(model.is_ok());
    }
}
