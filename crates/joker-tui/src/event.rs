//! Event types that flow through the TUI event loop.
//!
//! [`UiEvent`] is the single message type for the mpsc channel that
//! connects input threads, the agent observer, and the main loop.

use tracing::*;

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

impl UiEvent {
    /// Log a trace-level message for this event.
    pub fn log_trace(&self) {
        match self {
            UiEvent::Agent(_) => trace!("ui event: agent"),
            UiEvent::RunCompleted(_) => trace!("ui event: run_completed"),
            UiEvent::ModelDiscoveryCompleted(_) => trace!("ui event: model_discovery_completed"),
            UiEvent::Terminal(_) => trace!("ui event: terminal"),
            UiEvent::Tick => trace!("ui event: tick"),
        }
    }
}
