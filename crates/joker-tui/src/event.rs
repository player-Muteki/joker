//! Event types that flow through the TUI event loop.
//!
//! [`UiEvent`] is the single message type for the mpsc channel that
//! connects input threads, the agent observer, and the main loop.

/// Events that flow through the TUI event loop.
#[derive(Debug)]
pub enum UiEvent {
    /// Event forwarded from the agent runtime.
    Agent(joker::Event),
    /// An agent run finished with the given outcome or error.
    RunCompleted(Result<joker::RunOutcome, String>),
    /// Model-discovery request completed.
    ModelDiscoveryCompleted(Result<Vec<String>, String>),
    /// Raw terminal event (key, resize, etc.).
    Terminal(crossterm::event::Event),
    /// Periodic tick used for timed screen updates.
    Tick,
}
