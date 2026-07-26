//! Joker TUI — terminal user interface for the Joker agent kernel.
//!
//! Provides a full-screen terminal application built with
//! [`ratatui`](https://ratatui.rs) for interacting with AI agents.
//! The crate is structured around an event loop ([`terminal::run_tui`])
//! that dispatches keyboard input through [`app::App`] and drives agent
//! runs via [`driver::AgentDriver`].

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

pub mod app;
pub mod cli;
pub mod commands;
pub mod driver;
pub mod error;
pub mod event;
pub mod terminal;
pub mod widgets;

/// Re-export of [`error::TuiError`].
pub use error::TuiError;
/// Re-export of [`terminal::TuiOptions`].
pub use terminal::TuiOptions;
/// Re-export of [`terminal::run_tui`].
pub use terminal::run_tui;
