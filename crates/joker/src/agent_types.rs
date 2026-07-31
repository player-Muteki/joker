use tokio_util::sync::CancellationToken;

use crate::{Conversation, SharedApprovalChannel, StopReason, ToolCall};

/// Result of a complete agent [`run`](crate::Agent::run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    /// The final conversation state after all turns.
    pub conversation: Conversation,
    /// Why the run stopped.
    pub stop_reason: StopReason,
}

/// Outcome of a single agent turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    /// Why the model stopped producing output.
    pub stop_reason: StopReason,
    /// Whether the model issued tool calls.
    pub has_tool_calls: bool,
    /// Number of tool calls in this turn.
    pub tool_calls_count: usize,
    /// Tool calls awaiting execution.
    pub pending_tool_calls: Vec<ToolCall>,
}

/// Input to [`Agent::run`](crate::Agent::run).
pub struct RunRequest {
    /// Existing conversation history.  Empty for a fresh request.
    pub conversation: Conversation,
    /// Optional initial user message (used when `conversation` is empty).
    pub input: Option<String>,
    /// Channel for interactive approval prompts.
    pub approval_channel: Option<SharedApprovalChannel>,
    /// Token that signals cancellation of the run.
    pub cancellation_token: Option<CancellationToken>,
}

impl RunRequest {
    /// Create a request with a single user input and empty conversation.
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            conversation: Conversation::new(),
            input: Some(input.into()),
            approval_channel: None,
            cancellation_token: None,
        }
    }

    /// Create a request from an existing [`Conversation`] with no additional input.
    #[must_use]
    pub fn with_conversation(conversation: Conversation) -> Self {
        Self {
            conversation,
            input: None,
            approval_channel: None,
            cancellation_token: None,
        }
    }

    /// Attach an approval channel for interactive tool approval.
    #[must_use]
    pub fn with_approval_channel(mut self, channel: SharedApprovalChannel) -> Self {
        self.approval_channel = Some(channel);
        self
    }

    /// Attach a cancellation token.
    #[must_use]
    pub fn with_cancellation_token(mut self, cancellation_token: CancellationToken) -> Self {
        self.cancellation_token = Some(cancellation_token);
        self
    }
}
