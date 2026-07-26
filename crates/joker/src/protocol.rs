use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A sequence of [`Message`]s forming a dialogue between a user and an assistant.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    /// Create an empty [`Conversation`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a [`Conversation`] from an existing [`Vec<Message>`].
    #[must_use]
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    /// Borrow the list of [`Message`]s.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Append a [`Message`] to the conversation.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Consume self and return the inner [`Vec<Message>`].
    #[must_use]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

/// A single message in a [`Conversation`], consisting of a [`Role`] and content blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The sender's role.
    pub role: Role,
    /// The message body — one or more [`Content`] blocks.
    pub content: Vec<Content>,
}

impl Message {
    /// Build a user [`Message`] from a plain text string.
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Content::text(text)],
        }
    }

    /// Build an assistant [`Message`] from a list of [`Content`] blocks.
    #[must_use]
    pub fn assistant(content: Vec<Content>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    /// Build a tool [`Message`] from a list of [`ToolResult`]s.
    #[must_use]
    pub fn tool(results: Vec<ToolResult>) -> Self {
        Self {
            role: Role::Tool,
            content: results.into_iter().map(Content::ToolResult).collect(),
        }
    }
}

/// The participant who sent a [`Message`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// System-level instruction.
    System,
    /// The human user.
    User,
    /// The AI model.
    Assistant,
    /// A tool invoked by the model.
    Tool,
}

/// A block of content within a [`Message`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// Plain text content.
    Text(TextContent),
    /// Model reasoning / chain-of-thought.
    Reasoning(ReasoningContent),
    /// A tool invocation request.
    ToolCall(ToolCall),
    /// A tool invocation result.
    ToolResult(ToolResult),
}

impl Content {
    /// Create a [`Content::Text`] variant from a string.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent { text: text.into() })
    }
}

/// A plain-text content block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContent {
    /// The text body.
    pub text: String,
}

/// A reasoning / chain-of-thought content block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningContent {
    /// The reasoning text.
    pub text: String,
}

/// A request from the model to invoke a tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this call.
    pub id: String,
    /// The tool's name.
    pub name: String,
    /// JSON arguments for the tool.
    pub arguments: Value,
}

/// The result of a [`ToolCall`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Matches the [`ToolCall::id`] this result corresponds to.
    pub call_id: String,
    /// The tool's name.
    pub name: String,
    /// JSON output from the tool.
    pub output: Value,
    /// Whether the tool invocation failed.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful [`ToolResult`] with the given output.
    #[must_use]
    pub fn ok(call_id: impl Into<String>, name: impl Into<String>, output: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            output,
            is_error: false,
        }
    }

    /// Create a failed [`ToolResult`] with an error message.
    #[must_use]
    pub fn error(
        call_id: impl Into<String>,
        name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            output: Value::String(message.into()),
            is_error: true,
        }
    }
}

/// Token usage statistics for a model response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens consumed.
    pub input_tokens: u64,
    /// Output tokens generated.
    pub output_tokens: u64,
    /// Tokens served from the prompt cache.
    pub cache_hit_tokens: u64,
}

/// Why the model stopped generating.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model reached a natural stopping point.
    Stop,
    /// The model requested a tool invocation.
    ToolUse,
    /// The output reached the maximum token limit.
    Length,
    /// Generation was cancelled.
    Cancelled,
    /// A provider-specific limit was reached.
    LimitReached,
}
