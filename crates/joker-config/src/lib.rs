//! Configuration loading, resolution, and persistence for Joker.
//!
//! Supports a `joker.toml` file format with provider definitions, agent
//! profiles, tool permissions, and MCP server registrations. The resolved
//! [`RuntimeConfig`] merges file settings with CLI overrides.

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

pub mod error;
mod provider_selection;
pub mod resolve;
pub mod runtime;
mod store;
pub mod types;

#[doc(inline)]
pub use error::ConfigError;
#[doc(inline)]
pub use provider_selection::ProviderSelection;
#[doc(inline)]
pub use resolve::resolve_config;
#[doc(inline)]
pub use runtime::RuntimeConfig;
#[doc(inline)]
pub use store::ConfigStore;
#[doc(inline)]
pub use types::*;
