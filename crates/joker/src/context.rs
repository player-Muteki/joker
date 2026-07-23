use thiserror::Error;

use crate::{Content, Conversation, Message, error::BoxFutureResult};

pub type ContextFuture<'a> = BoxFutureResult<'a, BuiltContext, ContextError>;

pub trait ContextBuilder: Send + Sync {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a>;
}

#[derive(Clone, Copy, Debug)]
pub struct ContextInput<'a> {
    pub conversation: &'a Conversation,
    pub limits: ContextLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltContext {
    pub messages: Vec<Message>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextLimits {
    pub max_messages: usize,
    pub max_text_bytes: usize,
    pub max_tool_result_bytes: usize,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            max_messages: 64,
            max_text_bytes: 64 * 1024,
            max_tool_result_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("context limit exceeded: {0}")]
    LimitExceeded(&'static str),
}

#[derive(Default)]
pub struct PassthroughContextBuilder;

impl ContextBuilder for PassthroughContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let messages = input.conversation.messages().to_vec();
            enforce_limits(&messages, input.limits)?;
            Ok(BuiltContext { messages })
        })
    }
}

#[derive(Clone, Debug)]
pub struct FixedWindowContextBuilder {
    max_messages: usize,
}

impl FixedWindowContextBuilder {
    #[must_use]
    pub fn new(max_messages: usize) -> Self {
        Self { max_messages }
    }
}

impl ContextBuilder for FixedWindowContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let max_messages = self.max_messages.min(input.limits.max_messages);
            let messages = input.conversation.messages();
            let start = messages.len().saturating_sub(max_messages);
            let built = messages[start..].to_vec();
            enforce_limits(&built, input.limits)?;
            Ok(BuiltContext { messages: built })
        })
    }
}

fn enforce_limits(messages: &[Message], limits: ContextLimits) -> Result<(), ContextError> {
    if messages.len() > limits.max_messages {
        return Err(ContextError::LimitExceeded("messages"));
    }

    let mut text_bytes = 0usize;
    let mut tool_result_bytes = 0usize;
    for message in messages {
        for content in &message.content {
            match content {
                Content::Text(text) => text_bytes += text.text.len(),
                Content::Reasoning(reasoning) => text_bytes += reasoning.text.len(),
                Content::ToolResult(result) => tool_result_bytes += result.output.to_string().len(),
                Content::ToolCall(call) => text_bytes += call.arguments.to_string().len(),
            }
        }
    }

    if text_bytes > limits.max_text_bytes {
        return Err(ContextError::LimitExceeded("text bytes"));
    }
    if tool_result_bytes > limits.max_tool_result_bytes {
        return Err(ContextError::LimitExceeded("tool result bytes"));
    }
    Ok(())
}
