//! Provider implementations for the joker model runtime.
//!
//! This crate provides concrete [`Model`](joker::Model) implementations backed
//! by OpenAI-compatible, Anthropic Messages, and Google Gemini APIs, along with
//! protocol/route abstractions, a data-driven provider catalog, error
//! classification, model discovery, and stream reconnection.

#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

pub mod anthropic;
pub mod auth;
pub mod catalog;
pub mod error;
pub mod google;
pub mod model_discovery;
pub mod openai;
pub mod protocol;
pub mod reconnect;
pub mod spec;
pub mod sse;
pub mod transform;

pub use auth::resolve_auth;
pub use catalog::*;
pub use error::classify_error;
pub use model_discovery::*;
pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleModel, OpenAiProviderError};
pub use protocol::*;
pub use reconnect::ReconnectingModel;
pub use spec::*;
pub use sse::{SseEvent, SseTokenizer};
