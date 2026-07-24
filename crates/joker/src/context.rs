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

/// Context builder that uses a summary string when conversations grow large.
///
/// Keeps the most recent messages and prepends a summary of earlier ones
/// as a system message when the total message count exceeds the threshold.
pub struct SummaryContextBuilder {
    max_recent_messages: usize,
    inner: Box<dyn ContextBuilder>,
}

impl SummaryContextBuilder {
    #[must_use]
    pub fn new(max_recent_messages: usize, inner: Box<dyn ContextBuilder>) -> Self {
        Self {
            max_recent_messages,
            inner,
        }
    }

    /// Summarize a conversation into a compact string.
    /// This is a heuristic summary — in production you'd use an LLM call.
    pub fn summarize_conversation(conversation: &Conversation) -> String {
        let messages = conversation.messages();
        if messages.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();
        let mut user_msgs = 0usize;
        let mut assistant_msgs = 0usize;
        let mut tool_calls = 0usize;
        let mut tool_results = 0usize;

        for msg in messages {
            match msg.role {
                crate::Role::User => user_msgs += 1,
                crate::Role::Assistant => {
                    assistant_msgs += 1;
                    for content in &msg.content {
                        if matches!(content, Content::ToolCall(_)) {
                            tool_calls += 1;
                        }
                    }
                }
                crate::Role::Tool => tool_results += 1,
                crate::Role::System => {}
            }
        }

        // Extract first user message as context clue
        let first_user = messages
            .iter()
            .find(|m| m.role == crate::Role::User)
            .and_then(|m| {
                m.content
                    .iter()
                    .find_map(|c| match c {
                        Content::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
            })
            .unwrap_or_default();

        parts.push(format!(
            "This conversation has {} user messages, {} assistant messages, {} tool calls, and {} tool results.",
            user_msgs, assistant_msgs, tool_calls, tool_results
        ));

        if !first_user.is_empty() {
            parts.push(format!(
                "The initial request was: \"{}\"",
                Self::truncate_text(&first_user, 200)
            ));
        }

        parts.push("Earlier messages have been summarized. Key context is preserved above.".into());
        parts.join("\n")
    }

    fn truncate_text(text: &str, max: usize) -> String {
        if text.len() <= max {
            text.to_string()
        } else {
            format!("{}...", &text[..max])
        }
    }
}

impl ContextBuilder for SummaryContextBuilder {
    fn build<'a>(&'a self, input: ContextInput<'a>) -> ContextFuture<'a> {
        Box::pin(async move {
            let messages = input.conversation.messages();

            // If conversation is small enough, passthrough to inner builder
            if messages.len() <= self.max_recent_messages {
                return self.inner.build(input).await;
            }

            // Build a summary of older messages
            let cutoff = messages.len() - self.max_recent_messages;
            let older_msgs = &messages[..cutoff];
            let recent_msgs = &messages[cutoff..];

            // Create a temporary conversation for summarization
            let older_conv = Conversation::from_messages(older_msgs.to_vec());
            let summary = Self::summarize_conversation(&older_conv);

            // Prepend summary as system message
            let mut built = Vec::new();
            if !summary.is_empty() {
                built.push(Message {
                    role: crate::Role::System,
                    content: vec![Content::text(format!(
                        "[Summary of earlier conversation]:\n{summary}"
                    ))],
                });
            }
            built.extend_from_slice(recent_msgs);

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
