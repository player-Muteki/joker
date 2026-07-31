use std::{collections::VecDeque, sync::Mutex};

use futures_core::Stream;
use futures_util::stream;
use serde_json::Value;
use thiserror::Error;

use crate::{
    Content, Message, StopReason, ToolCall, ToolDefinition, Usage, error::BoxFutureResult,
};

/// Boxed, sendable, unpinned stream of [`ModelResponseEvent`] items.
pub type ModelStream =
    Box<dyn Stream<Item = Result<ModelResponseEvent, ModelError>> + Send + Unpin>;
/// Future returned by [`Model::stream`] that resolves to a [`ModelStream`] or
/// [`ModelError`].
pub type ModelFuture<'a> = BoxFutureResult<'a, ModelStream, ModelError>;

/// Trait for LLM providers.
///
/// Implementors send a [`ModelRequest`] and receive a stream of
/// [`ModelResponseEvent`] items.
pub trait Model: Send + Sync {
    /// Send a request and return a stream of response events.
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_>;
}

/// Input to a [`Model`]: conversation history and available tool definitions.
#[derive(Clone, Debug)]
pub struct ModelRequest {
    /// Messages forming the conversation so far.
    pub messages: Vec<Message>,
    /// Tool definitions the model may call.
    pub tools: Vec<ToolDefinition>,
}

/// An event emitted during model streaming.
///
/// Streams yield zero or more `TextDelta` / `ReasoningDelta` / `ToolCall`
/// events followed by a single `Finished` event.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelResponseEvent {
    /// A delta of text content.
    TextDelta(String),
    /// A delta of reasoning / chain-of-thought content.
    ReasoningDelta(String),
    /// A tool call requested by the model.
    ToolCall(ToolCall),
    /// Stream finished with the given [`StopReason`] and [`Usage`].
    Finished {
        /// Why the model stopped.
        stop_reason: StopReason,
        /// Token usage for this request.
        usage: Usage,
    },
    /// Retry notification — the model is reconnecting after a failure.
    /// Only emitted when no output has been produced yet.
    Retrying {
        /// Which retry attempt this is (1-indexed).
        attempt: u32,
        /// Maximum retries configured.
        max_retries: u32,
        /// Human-readable reason for the retry.
        reason: String,
    },
}

/// Classification of a model-stream failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelErrorKind {
    /// Authentication failed (bad or missing API key).
    Auth,
    /// The provider rate-limited the request (HTTP 429).
    RateLimited,
    /// Quota exhausted or billing problem (HTTP 402, `insufficient_quota`).
    Quota,
    /// The requested model does not exist.
    ModelNotFound,
    /// The request exceeded the model's context window.
    ContextLength,
    /// Transport or server failure (network error, HTTP 5xx).
    Network,
    /// Protocol-level failure (malformed SSE, unexpected payload).
    Protocol,
    /// No classification matched.
    Unknown,
}

/// Errors that can occur during model streaming.
#[derive(Debug, Error)]
pub enum ModelError {
    /// The stream failed with the given message.
    #[error("model stream failed: {0}")]
    Stream(String),
    /// The stream failed with a classified error kind.
    #[error("{message}")]
    Classified {
        /// Classification of the failure.
        kind: ModelErrorKind,
        /// Human-readable failure message.
        message: String,
    },
    /// The request was cancelled.
    #[error("model was cancelled")]
    Cancelled,
}

impl ModelError {
    /// Whether this error is worth retrying.
    ///
    /// Only network/transport failures are retried; authentication, quota,
    /// rate-limit, and context-length failures will repeat and are surfaced
    /// immediately. Unclassified stream errors are retried to preserve legacy
    /// behavior.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            ModelError::Classified { kind, .. } => {
                matches!(kind, ModelErrorKind::Network)
            }
            ModelError::Stream(_) => true,
            ModelError::Cancelled => false,
        }
    }
}

/// A [`Model`] that returns pre-scripted [`ScriptedStep`]s.
///
/// Useful for testing — each call to [`Model::stream`] pops the next step and
/// either yields events or returns an error.
#[derive(Debug)]
pub struct ScriptedModel {
    steps: Mutex<VecDeque<ScriptedStep>>,
}

impl ScriptedModel {
    /// Create a new [`ScriptedModel`] from an iterator of [`ScriptedStep`]s.
    #[must_use]
    pub fn new(steps: impl IntoIterator<Item = ScriptedStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
        }
    }
}

impl Model for ScriptedModel {
    fn stream(&self, _request: ModelRequest) -> ModelFuture<'_> {
        Box::pin(async move {
            let step = self
                .steps
                .lock()
                .expect("scripted model mutex poisoned")
                .pop_front()
                .ok_or_else(|| {
                    ModelError::Stream("scripted model has no remaining steps".into())
                })?;

            match step {
                ScriptedStep::Events(events) => {
                    Ok(Box::new(stream::iter(events.into_iter().map(Ok))) as ModelStream)
                }
                ScriptedStep::Error(message) => Err(ModelError::Stream(message)),
                ScriptedStep::Cancelled => Err(ModelError::Cancelled),
            }
        })
    }
}

/// A single step in a [`ScriptedModel`] script.
#[derive(Clone, Debug)]
pub enum ScriptedStep {
    /// Emit these events (typically ending with `Finished`).
    Events(Vec<ModelResponseEvent>),
    /// Return a [`ModelError::Stream`] with this message.
    Error(String),
    /// Return a [`ModelError::Cancelled`].
    Cancelled,
}

impl ScriptedStep {
    /// Create a step that yields a single text delta followed by `Finished`
    /// with [`StopReason::Stop`].
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Events(vec![
            ModelResponseEvent::TextDelta(text.into()),
            ModelResponseEvent::Finished {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
            },
        ])
    }

    /// Create a step that yields a single [`ToolCall`] followed by `Finished`
    /// with [`StopReason::ToolUse`].
    #[must_use]
    pub fn tool_call(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self::Events(vec![
            ModelResponseEvent::ToolCall(ToolCall {
                id: id.into(),
                name: name.into(),
                arguments,
            }),
            ModelResponseEvent::Finished {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
        ])
    }

    /// Create a step from [`Content`] items.
    ///
    /// Text, reasoning, and tool-call items are converted to the corresponding
    /// delta events; tool-result items are ignored. The step always ends with
    /// `Finished` carrying the given [`StopReason`].
    #[must_use]
    pub fn message(content: Vec<Content>, stop_reason: StopReason) -> Self {
        let mut events = Vec::new();
        for item in content {
            match item {
                Content::Text(text) => events.push(ModelResponseEvent::TextDelta(text.text)),
                Content::Reasoning(reasoning) => {
                    events.push(ModelResponseEvent::ReasoningDelta(reasoning.text));
                }
                Content::ToolCall(tool_call) => {
                    events.push(ModelResponseEvent::ToolCall(tool_call))
                }
                Content::ToolResult(_) => {}
            }
        }
        events.push(ModelResponseEvent::Finished {
            stop_reason,
            usage: Usage::default(),
        });
        Self::Events(events)
    }
}
