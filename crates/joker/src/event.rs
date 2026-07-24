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
    RunStarted,
    RunFinished { stop_reason: StopReason },
    ModelStarted,
    ModelDelta { delta: String },
    ModelFinished { stop_reason: StopReason },
    ToolStarted { call_id: String, name: String },
    ToolDelta { call_id: String, delta: Value },
    ToolFinished { result: ToolResult },
    LimitReached { reason: String },
    /// Emitted when the agent encounters an `Ask` decision from the policy.
    PermissionRequested {
        request_id: String,
        tool_name: String,
        subject: String,
        reason: String,
    },
    /// Emitted when a permission request is resolved (approved or denied).
    PermissionResolved {
        request_id: String,
        approved: bool,
        reason: Option<String>,
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
