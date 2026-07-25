use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{StopReason, ToolResult, error::BoxFutureResult};

pub type ObserverFuture<'a> = BoxFutureResult<'a, (), std::convert::Infallible>;

pub trait Observer: Send + Sync {
    fn observe(&self, event: Event) -> ObserverFuture<'_>;
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    // ── Lifecycle ──────────────────────────────────────────────────────
    RunStarted,
    RunFinished { stop_reason: StopReason },

    // ── Turn boundaries (OUTLINE 2.4) ──────────────────────────────────
    TurnStarted {
        session_id: String,
        turn_id: String,
        agent_name: String,
        model_id: String,
    },
    TurnDone {
        turn_id: String,
        stop_reason: StopReason,
    },

    // ── Model output (OUTLINE 2.4: TextDelta, ReasoningDelta distinct) ─
    ModelStarted,
    /// Deprecated: use `TextDelta` and `ReasoningDelta` instead.
    #[deprecated(note = "use Event::TextDelta and Event::ReasoningDelta")]
    ModelDelta { delta: String },
    ModelFinished { stop_reason: StopReason },
    TextDelta { delta: String },
    ReasoningDelta { delta: String },

    // ── Tool lifecycle (OUTLINE 2.4: ToolDispatch, ToolProgress) ──────
    ToolStarted { call_id: String, name: String },
    ToolDelta { call_id: String, delta: Value },
    ToolProgress { call_id: String, partial_output: String },
    ToolDispatch {
        call_id: String,
        tool_name: String,
        args_preview: Value,
    },
    ToolFinished { result: ToolResult },

    // ── Token usage (OUTLINE 2.4) ──────────────────────────────────────
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cache_hit_tokens: u64,
    },

    // ── Context compaction (OUTLINE 2.4) ───────────────────────────────
    CompactionStarted {
        trigger: String,
        current_tokens: usize,
        threshold: usize,
    },
    CompactionDone {
        tokens_before: usize,
        tokens_after: usize,
    },

    // ── Agent / model switching (OUTLINE 2.4) ──────────────────────────
    AgentSwitched { from: String, to: String },
    ModelSwitched { from: String, to: String },

    // ── Limits (OUTLINE 2.4) ───────────────────────────────────────────
    LimitReached { reason: String },

    // ── Permission (OUTLINE 2.2) ───────────────────────────────────────
    PermissionRequested {
        request_id: String,
        tool_name: String,
        subject: String,
        reason: String,
    },
    PermissionResolved {
        request_id: String,
        approved: bool,
        reason: Option<String>,
    },

    // ── Error / retry (OUTLINE 2.1: loop fault tolerance) ──────────────
    Error {
        kind: String,
        message: String,
        recoverable: bool,
    },
    Retrying {
        attempt: usize,
        max_attempts: usize,
        reason: String,
    },
}

#[derive(Default)]
pub struct NoopObserver;

impl Observer for NoopObserver {
    fn observe(&self, _event: Event) -> ObserverFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone, Default)]
pub struct RecordingObserver {
    events: Arc<Mutex<Vec<Event>>>,
}

impl RecordingObserver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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
