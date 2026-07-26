use std::{future::Future, pin::Pin};

use thiserror::Error;

use crate::{ContextError, ModelError};

/// Errors that can occur during an agent run.
#[derive(Debug, Error)]
pub enum RunError {
    /// Context construction failed.
    #[error("context build failed: {0}")]
    Context(#[from] ContextError),
    /// Model streaming or invocation failed.
    #[error("model failed: {0}")]
    Model(#[from] ModelError),
    /// Run was cancelled (via cancellation token or [`Op::Cancel`]).
    #[error("run was cancelled")]
    Cancelled,
    /// A run limit (steps or tool calls) was exceeded.
    #[error("run limit reached: {0}")]
    LimitReached(&'static str),
    /// Runtime received [`Op::Shutdown`].
    #[error("run was shut down via Op")]
    Shutdown,
    /// Agent is already executing a run.
    #[error("agent is busy — another run is in progress")]
    Busy,
}

pub(crate) type BoxFutureResult<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;
