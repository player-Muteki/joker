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

pub use error::TuiError;
pub use terminal::{TuiOptions, run_tui};
