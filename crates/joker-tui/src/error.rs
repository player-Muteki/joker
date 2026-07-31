//! Error types for the TUI crate.
//!
//! Wraps terminal I/O errors, agent failures, config errors, and a
//! channel-closed variant into [`TuiError`].

use thiserror::Error;

/// Errors that can occur during TUI operation.
#[derive(Debug, Error)]
pub enum TuiError {
    /// Terminal setup or mode-switching failed.
    #[error("terminal error: {0}")]
    Terminal(String),
    /// I/O error from ratatui or crossterm.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Agent construction or run failed.
    #[error("agent run failed: {0}")]
    Agent(String),
    /// Configuration parsing or loading error.
    #[error("config error: {0}")]
    Config(#[from] joker_config::ConfigError),
    /// Invalid command-line usage or non-interactive preflight failure.
    #[error("{0}")]
    Cli(String),
    /// The internal event channel was closed unexpectedly.
    #[error("event channel closed")]
    ChannelClosed,
}
