/// Drain mode for a pending message queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainMode {
    /// Drain all pending messages at once.
    All,
    /// Drain one message at a time.
    OneAtATime,
}

/// A thread-safe queue of pending string messages.
///
/// Used by [`AgentRuntime`](crate::AgentRuntime) for steer and follow-up queues.
#[derive(Debug)]
pub struct PendingMessageQueue {
    messages: std::sync::Mutex<Vec<String>>,
}

impl PendingMessageQueue {
    /// Create an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self { messages: std::sync::Mutex::new(Vec::new()) }
    }

    /// Push a message onto the queue.
    pub fn enqueue(&self, message: impl Into<String>) {
        self.messages.lock().unwrap().push(message.into());
    }

    /// Drain messages according to the given [`DrainMode`].
    #[must_use]
    pub fn drain(&self, mode: DrainMode) -> Vec<String> {
        let mut guard = self.messages.lock().unwrap();
        match mode {
            DrainMode::All => std::mem::take(&mut *guard),
            DrainMode::OneAtATime => {
                if guard.is_empty() {
                    Vec::new()
                } else {
                    vec![guard.remove(0)]
                }
            }
        }
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }

    /// Number of pending messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.lock().unwrap().len()
    }
}

impl Clone for PendingMessageQueue {
    fn clone(&self) -> Self {
        let guard = self.messages.lock().unwrap();
        Self { messages: std::sync::Mutex::new(guard.clone()) }
    }
}

impl Default for PendingMessageQueue {
    fn default() -> Self {
        Self::new()
    }
}
