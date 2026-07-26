use tokio_util::sync::CancellationToken;

use crate::{Conversation, SharedApprovalChannel, StopReason, ToolCall};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    pub conversation: Conversation,
    pub stop_reason: StopReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    pub stop_reason: StopReason,
    pub has_tool_calls: bool,
    pub tool_calls_count: usize,
    pub pending_tool_calls: Vec<ToolCall>,
}

pub struct RunRequest {
    pub conversation: Conversation,
    pub input: Option<String>,
    pub approval_channel: Option<SharedApprovalChannel>,
    pub cancellation_token: Option<CancellationToken>,
}

impl RunRequest {
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            conversation: Conversation::new(),
            input: Some(input.into()),
            approval_channel: None,
            cancellation_token: None,
        }
    }

    #[must_use]
    pub fn with_conversation(conversation: Conversation) -> Self {
        Self { conversation, input: None, approval_channel: None, cancellation_token: None }
    }

    #[must_use]
    pub fn with_approval_channel(mut self, channel: SharedApprovalChannel) -> Self {
        self.approval_channel = Some(channel);
        self
    }

    #[must_use]
    pub fn with_cancellation_token(mut self, cancellation_token: CancellationToken) -> Self {
        self.cancellation_token = Some(cancellation_token);
        self
    }
}
