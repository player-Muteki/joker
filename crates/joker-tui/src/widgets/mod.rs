//! Ratatui widget rendering functions.
//!
//! Each submodule exposes a single `pub fn render` that draws one
//! section of the TUI (composer, transcript, dialogs, etc.).

/// Slash-command autocomplete palette.
pub mod command_palette;
/// Prompt composer input field.
pub mod composer;
/// Top-level layout orchestrator.
pub mod layout;
/// Modal selection dialog.
pub mod selector;
/// Conversation transcript viewport.
pub mod transcript;
