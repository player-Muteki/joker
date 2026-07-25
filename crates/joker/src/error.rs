use std::{future::Future, pin::Pin};

use thiserror::Error;

use crate::{ContextError, ModelError};

#[derive(Debug, Error)]
pub enum RunError {
    #[error("context build failed: {0}")]
    Context(#[from] ContextError),
    #[error("model failed: {0}")]
    Model(#[from] ModelError),
    #[error("run was cancelled")]
    Cancelled,
    #[error("run limit reached: {0}")]
    LimitReached(&'static str),
    #[error("run was shut down via Op")]
    Shutdown,
    #[error("agent is busy — another run is in progress")]
    Busy,
}

pub(crate) type BoxFutureResult<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;
