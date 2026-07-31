//! Stream reconnection wrapper.
//!
//! [`ReconnectingModel`] wraps any [`Model`](joker::Model) and automatically
//! retries the stream on connection failure, but only when no output events
//! have been emitted yet.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use joker::{Model, ModelError, ModelFuture, ModelRequest, ModelResponseEvent, ModelStream};
use tokio::time::sleep;

/// Wraps an inner [`Model`](joker::Model) and retries the stream on connection failure,
/// but only when no output has been emitted yet.
pub struct ReconnectingModel {
    inner: Arc<dyn Model>,
    max_retries: u32,
    base_delay_ms: u64,
}

impl ReconnectingModel {
    /// Create a new [`ReconnectingModel`] wrapping `inner`.
    ///
    /// Defaults to 3 retries with a 1-second base delay.
    #[must_use]
    pub fn new(inner: Arc<dyn Model>) -> Self {
        Self {
            inner,
            max_retries: 3,
            base_delay_ms: 1000,
        }
    }

    /// Set the maximum number of retry attempts.
    #[must_use]
    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// Set the base delay in milliseconds (doubled each retry via exponential backoff).
    #[must_use]
    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.base_delay_ms = ms;
        self
    }
}

impl Model for ReconnectingModel {
    fn stream(&self, request: ModelRequest) -> ModelFuture<'_> {
        let inner = self.inner.clone();
        let max_retries = self.max_retries;
        let base_delay_ms = self.base_delay_ms;

        Box::pin(async move {
            let stream = inner.stream(request.clone()).await?;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let reconnector = ReconnectDetector {
                inner: stream,
                tx,
                attempt: 0,
                max_retries,
                base_delay_ms,
                request: request.clone(),
                model: inner.clone(),
                has_emitted: false,
            };
            tokio::spawn(reconnector.run());
            Ok(Box::new(ReceiverStream { rx }) as ModelStream)
        })
    }
}

struct ReconnectDetector {
    inner: ModelStream,
    tx: tokio::sync::mpsc::UnboundedSender<Result<ModelResponseEvent, ModelError>>,
    attempt: u32,
    max_retries: u32,
    base_delay_ms: u64,
    request: ModelRequest,
    model: Arc<dyn Model>,
    has_emitted: bool,
}

impl ReconnectDetector {
    async fn run(mut self) {
        use futures_util::StreamExt;
        loop {
            match self.inner.next().await {
                Some(Ok(event)) => {
                    self.has_emitted = true;
                    if self.tx.send(Ok(event)).is_err() {
                        return;
                    }
                }
                Some(Err(error)) if !self.has_emitted && self.attempt < self.max_retries => {
                    self.attempt += 1;
                    let delay = self.base_delay_ms * (1u64 << (self.attempt - 1));
                    let _ = self.tx.send(Ok(ModelResponseEvent::Retrying {
                        attempt: self.attempt,
                        max_retries: self.max_retries,
                        reason: error.to_string(),
                    }));
                    sleep(Duration::from_millis(delay)).await;
                    match self.model.stream(self.request.clone()).await {
                        Ok(new_stream) => {
                            self.inner = new_stream;
                            self.has_emitted = false;
                        }
                        Err(e) => {
                            let _ = self.tx.send(Err(e));
                            return;
                        }
                    }
                }
                Some(Err(error)) => {
                    let _ = self.tx.send(Err(error));
                    return;
                }
                None => {
                    let _ = self
                        .tx
                        .send(Err(ModelError::Stream("stream ended unexpectedly".into())));
                    return;
                }
            }
        }
    }
}

struct ReceiverStream {
    rx: tokio::sync::mpsc::UnboundedReceiver<Result<ModelResponseEvent, ModelError>>,
}

impl Stream for ReceiverStream {
    type Item = Result<ModelResponseEvent, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}
