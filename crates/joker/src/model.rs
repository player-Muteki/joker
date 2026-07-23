use std::{collections::VecDeque, sync::Mutex};

use futures_core::Stream;
use futures_util::stream;
use serde_json::Value;
use thiserror::Error;

use crate::{
    Content, Message, StopReason, ToolCall, ToolDefinition, Usage, error::BoxFutureResult,
};

pub type ModelStream =
    Box<dyn Stream<Item = Result<ModelResponseEvent, ModelError>> + Send + Unpin>;
pub type ModelFuture<'a> = BoxFutureResult<'a, ModelStream, ModelError>;

pub trait Model: Send + Sync {
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_>;
}

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelResponseEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCall(ToolCall),
    Finished {
        stop_reason: StopReason,
        usage: Usage,
    },
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("model stream failed: {0}")]
    Stream(String),
    #[error("model was cancelled")]
    Cancelled,
}

#[derive(Debug)]
pub struct ScriptedModel {
    steps: Mutex<VecDeque<ScriptedStep>>,
}

impl ScriptedModel {
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

#[derive(Clone, Debug)]
pub enum ScriptedStep {
    Events(Vec<ModelResponseEvent>),
    Error(String),
    Cancelled,
}

impl ScriptedStep {
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
