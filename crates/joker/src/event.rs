use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{StopReason, ToolResult, error::BoxFutureResult};

/// Future type returned by [`Observer::observe`].
pub type ObserverFuture<'a> = BoxFutureResult<'a, (), std::convert::Infallible>;

/// Trait for consuming [`Event`]s emitted during agent execution.
///
/// Implementations must be [`Send`] + [`Sync`] and are called for every event
/// produced by a turn.
pub trait Observer: Send + Sync {
    /// Observe a single [`Event`].
    ///
    /// The returned future is awaited by the runtime; errors are infallible.
    fn observe(&self, event: Event) -> ObserverFuture<'_>;
}

/// Core event enum emitted during an agent turn.
///
/// Every variant represents a point in the turn lifecycle: start/finish,
/// text deltas, reasoning deltas, tool invocations, usage accounting,
/// compaction, permission/approval flows, errors, and retries.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Turn has started.
    RunStarted,

    /// Turn has finished with the given [`StopReason`].
    RunFinished {
        /// Reason the turn stopped.
        stop_reason: StopReason,
    },

    /// A new turn has started within a session.
    TurnStarted {
        /// Session identifier.
        session_id: String,
        /// Turn identifier.
        turn_id: String,
        /// Name of the agent handling the turn.
        agent_name: String,
        /// Model identifier used for the turn.
        model_id: String,
    },

    /// A turn has completed.
    TurnDone {
        /// Turn identifier.
        turn_id: String,
        /// Reason the turn stopped.
        stop_reason: StopReason,
    },

    /// Model invocation has started.
    ModelStarted,

    /// Deprecated: use [`TextDelta`](Event::TextDelta) and
    /// [`ReasoningDelta`](Event::ReasoningDelta) instead.
    #[deprecated(note = "use Event::TextDelta and Event::ReasoningDelta")]
    ModelDelta {
        /// Raw delta string.
        delta: String,
    },

    /// Model invocation has finished.
    ModelFinished {
        /// Reason the model stopped.
        stop_reason: StopReason,
    },

    /// A text delta chunk produced by the model.
    TextDelta {
        /// Partial text content.
        delta: String,
    },

    /// A reasoning delta chunk produced by the model.
    ReasoningDelta {
        /// Partial reasoning content.
        delta: String,
    },

    /// A tool call has started.
    ToolStarted {
        /// Unique call identifier.
        call_id: String,
        /// Name of the tool being called.
        name: String,
    },

    /// A tool call produced a delta chunk.
    ToolDelta {
        /// Call identifier matching [`ToolStarted`](Event::ToolStarted).
        call_id: String,
        /// Partial output value.
        delta: Value,
    },

    /// A tool call produced incremental text output.
    ToolProgress {
        /// Call identifier matching [`ToolStarted`](Event::ToolStarted).
        call_id: String,
        /// Partial text output accumulated so far.
        partial_output: String,
    },

    /// A tool dispatch has been prepared (arguments preview available).
    ToolDispatch {
        /// Unique call identifier.
        call_id: String,
        /// Name of the tool being dispatched.
        tool_name: String,
        /// Preview of the tool arguments.
        args_preview: Value,
    },

    /// A tool call has finished with the given [`ToolResult`].
    ToolFinished {
        /// Result of the tool execution.
        result: ToolResult,
    },

    /// Token usage for the turn.
    Usage {
        /// Input tokens consumed.
        input_tokens: u64,
        /// Output tokens generated.
        output_tokens: u64,
        /// Tokens served from cache.
        cache_hit_tokens: u64,
    },

    /// Context compaction has started.
    CompactionStarted {
        /// Reason compaction was triggered.
        trigger: String,
        /// Number of tokens before compaction.
        current_tokens: usize,
        /// Token threshold that triggered compaction.
        threshold: usize,
    },

    /// Context compaction has completed.
    CompactionDone {
        /// Token count before compaction.
        tokens_before: usize,
        /// Token count after compaction.
        tokens_after: usize,
    },

    /// The active agent was switched.
    AgentSwitched {
        /// Name of the previous agent.
        from: String,
        /// Name of the new agent.
        to: String,
    },

    /// The active model was switched.
    ModelSwitched {
        /// Name of the previous model.
        from: String,
        /// Name of the new model.
        to: String,
    },

    /// A limit was reached during execution.
    LimitReached {
        /// Description of the limit that was reached.
        reason: String,
    },

    /// An approval request has been issued (OUTLINE 2.1).
    ApprovalRequest {
        /// Unique request identifier.
        request_id: String,
        /// Tool requiring approval.
        tool_name: String,
        /// Subject or description of the requested operation.
        subject: String,
        /// Reason approval is required.
        reason: String,
    },

    /// A permission check was requested (OUTLINE 2.2).
    PermissionRequested {
        /// Unique request identifier.
        request_id: String,
        /// Tool requiring permission.
        tool_name: String,
        /// Subject or description of the operation.
        subject: String,
        /// Reason the permission check was triggered.
        reason: String,
    },

    /// A permission request was resolved.
    PermissionResolved {
        /// Request identifier matching [`PermissionRequested`](Event::PermissionRequested).
        request_id: String,
        /// Whether the permission was granted.
        approved: bool,
        /// Optional explanation for the resolution.
        reason: Option<String>,
    },

    /// An error occurred during execution.
    Error {
        /// Category or kind of error.
        kind: String,
        /// Human-readable error message.
        message: String,
        /// Whether the error can be recovered from.
        recoverable: bool,
    },

    /// A retry is being attempted after a recoverable error.
    Retrying {
        /// Current attempt number (1-indexed).
        attempt: usize,
        /// Maximum number of attempts permitted.
        max_attempts: usize,
        /// Reason for the retry.
        reason: String,
    },
}

/// No-op implementation of [`Observer`] that discards all events.
#[derive(Default)]
pub struct NoopObserver;

impl Observer for NoopObserver {
    fn observe(&self, _event: Event) -> ObserverFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

/// Observer that records all [`Event`]s into an in-memory buffer for later
/// inspection or replay.
#[derive(Clone, Default)]
pub struct RecordingObserver {
    events: Arc<Mutex<Vec<Event>>>,
}

impl RecordingObserver {
    /// Create a new [`RecordingObserver`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a snapshot of all recorded [`Event`]s.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .expect("recording observer mutex poisoned")
            .clone()
    }
}

impl Observer for RecordingObserver {
    fn observe(&self, event: Event) -> ObserverFuture<'_> {
        Box::pin(async move {
            self.events
                .lock()
                .expect("recording observer mutex poisoned")
                .push(event);
            Ok(())
        })
    }
}
