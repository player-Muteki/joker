//! Provider selection — maps provider names to routes or scripted mode.

use joker_provider::{Auth, Framing, Protocol, Route, preset_spec};
use tracing::info;
use crate::error::ConfigError;

/// Represents the active provider: either a scripted echo provider or a routed LLM.
///
/// The `Route` variant intentionally carries the full provider spec (catalog
/// capabilities, limits, and options) so request construction can be driven by
/// catalog data — hence its large size.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProviderSelection {
    /// Scripted mode returns a fixed response without calling any LLM.
    Scripted {
        /// Fixed model label used in scripted mode.
        model: String,
    },
    /// A routed provider with a full connection specification.
    Route(Route),
}

impl ProviderSelection {
    /// Returns the default scripted provider selection.
    #[must_use]
    pub fn scripted() -> Self {
        Self::Scripted {
            model: "scripted".into(),
        }
    }

    /// Select a provider by its well-known name (e.g. `"deepseek"`, `"anthropic"`, `"openai-compatible"`).
    ///
    /// Built-in providers resolve through the [`preset_spec`] catalog, which
    /// supplies the base URL, auth convention, and default model.
    pub fn preset(provider: &str) -> Result<Self, ConfigError> {
        info!(target: "config", provider = %provider, "selecting provider");
        match provider.trim().to_ascii_lowercase().as_str() {
            "" | "scripted" => Ok(Self::scripted()),
            "openai-compatible" | "custom" => Ok(Self::Route(Route {
                id: "openai-compatible".into(),
                protocol: Protocol::ChatCompletions,
                base_url: "http://localhost:8000/v1".into(),
                auth: Auth::bearer_from_env("OPENAI_COMPATIBLE_API_KEY"),
                framing: Framing::Sse,
                default_model: "model".into(),
                spec: None,
                credential_store: None,
            })),
            name => preset_spec(name)
                .map(|spec| Self::Route(Route::from_spec(spec, None)))
                .ok_or_else(|| ConfigError::UnknownProvider(provider.into())),
        }
    }
}
