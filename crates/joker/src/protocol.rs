use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    #[must_use]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Content>,
}

impl Message {
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Content::text(text)],
        }
    }

    #[must_use]
    pub fn assistant(content: Vec<Content>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    #[must_use]
    pub fn tool(results: Vec<ToolResult>) -> Self {
        Self {
            role: Role::Tool,
            content: results.into_iter().map(Content::ToolResult).collect(),
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text(TextContent),
    Reasoning(ReasoningContent),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

impl Content {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent { text: text.into() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningContent {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub output: Value,
    pub is_error: bool,
}

impl ToolResult {
    #[must_use]
    pub fn ok(call_id: impl Into<String>, name: impl Into<String>, output: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            output,
            is_error: false,
        }
    }

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    ToolUse,
    Length,
    Cancelled,
    LimitReached,
}
