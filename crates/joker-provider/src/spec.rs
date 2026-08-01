//! Data-driven provider and model specifications.
//!
//! [`ProviderSpec`] describes a provider endpoint entirely as data — base
//! URL, auth conventions, static headers, and a model catalog — so that
//! adding an OpenAI-compatible provider requires no code. This mirrors the
//! catalog-driven design of opencode's `provider.ts` and codex's
//! `ModelProviderInfo`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{Auth, Protocol};

/// A data-driven description of one provider endpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProviderSpec {
    /// Machine-readable provider identifier (e.g. `"deepseek"`).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Base URL of the provider API.
    pub base_url: String,
    /// Environment variable holding the API key, if any.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Wire protocol used by this provider.
    pub protocol: Protocol,
    /// Provider-level request options.
    #[serde(default)]
    pub options: ProviderOptions,
    /// Model catalog keyed by model identifier.
    #[serde(default)]
    pub models: BTreeMap<String, ModelInfo>,
}

impl ProviderSpec {
    /// Build the default [`Auth`] for this spec's protocol and env var.
    ///
    /// Chat completions endpoints use a bearer token; Anthropic and Google use
    /// vendor-specific key headers.
    #[must_use]
    pub fn default_auth(&self) -> Auth {
        match (&self.protocol, self.api_key_env.as_deref()) {
            (Protocol::ChatCompletions, Some(env)) => Auth::bearer_from_env(env),
            (Protocol::AnthropicMessages, Some(env)) => Auth::api_key_from_env("x-api-key", env),
            (Protocol::GoogleGemini, Some(env)) => Auth::api_key_from_env("x-goog-api-key", env),
            _ => Auth::none(),
        }
    }
}

/// Provider-level request options applied to every model call.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ProviderOptions {
    /// Overall request timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Static HTTP headers merged into every request.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Extra JSON body fields merged into every request body.
    #[serde(default)]
    pub extra_body: Option<Value>,
}

/// Data-driven metadata for one model.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    /// Model identifier sent in the request body.
    #[serde(default)]
    pub id: String,
    /// Capabilities driving request construction.
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    /// Token pricing, currently informational.
    #[serde(default)]
    pub cost: ModelCost,
    /// Context and output limits.
    #[serde(default)]
    pub limit: ModelLimit,
    /// Per-model request defaults.
    #[serde(default)]
    pub options: ModelOptions,
    /// Reasoning-effort variants for this model. Reserved for future use.
    #[serde(default)]
    pub variants: Vec<String>,
    /// Lifecycle status.
    #[serde(default)]
    pub status: ModelStatus,
}

/// Capabilities of a model, driving request construction and tool filtering.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    /// Whether the provider accepts a `temperature` parameter.
    #[serde(default)]
    pub temperature: bool,
    /// Whether the model produces reasoning content.
    #[serde(default)]
    pub reasoning: bool,
    /// Whether the model supports tool calls.
    #[serde(default)]
    pub toolcall: bool,
    /// Whether the model accepts attachments (multimodal input). Reserved.
    #[serde(default)]
    pub attachment: bool,
    /// Whether the provider hosts web search. Reserved for agent-level planning.
    #[serde(default)]
    pub web_search: bool,
    /// Whether reasoning parts are interleaved into message content. Reserved.
    #[serde(default)]
    pub interleaved: bool,
}

/// Token pricing for a model (per million tokens, USD).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelCost {
    /// Price per million input tokens.
    #[serde(default)]
    pub input: f64,
    /// Price per million output tokens.
    #[serde(default)]
    pub output: f64,
    /// Price per million cache-read tokens.
    #[serde(default)]
    pub cache_read: f64,
    /// Price per million cache-write tokens.
    #[serde(default)]
    pub cache_write: f64,
}

/// Token limits for a model.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelLimit {
    /// Context window size in tokens.
    #[serde(default)]
    pub context: u64,
    /// Maximum output tokens.
    #[serde(default)]
    pub max_output: u64,
}

/// Per-model request defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelOptions {
    /// Default `temperature` sent when the provider supports it.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Default `max_tokens` sent in the request body.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Extra JSON body fields merged for this model.
    #[serde(default)]
    pub extra_body: Option<Value>,
}

/// Lifecycle status of a model in the catalog.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelStatus {
    /// Usable by default.
    #[default]
    Available,
    /// Deprecated — hidden from pickers unless explicitly requested.
    Deprecated,
    /// Disabled — filtered from the catalog.
    Disabled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips_through_json() {
        let spec = ProviderSpec {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            base_url: "https://api.deepseek.com".into(),
            api_key_env: Some("DEEPSEEK_API_KEY".into()),
            protocol: Protocol::ChatCompletions,
            options: ProviderOptions {
                timeout: Some(30),
                headers: BTreeMap::from([("x-custom".into(), "1".into())]),
                extra_body: None,
            },
            models: BTreeMap::from([(
                "deepseek-chat".into(),
                ModelInfo {
                    id: "deepseek-chat".into(),
                    capabilities: ModelCapabilities {
                        temperature: true,
                        reasoning: true,
                        toolcall: true,
                        ..Default::default()
                    },
                    limit: ModelLimit {
                        context: 128_000,
                        max_output: 8_192,
                    },
                    ..Default::default()
                },
            )]),
        };

        let json = serde_json::to_string(&spec).expect("serialize");
        let decoded: ProviderSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn default_auth_matches_protocol() {
        let chat = ProviderSpec {
            id: "x".into(),
            name: "X".into(),
            base_url: "https://x.example".into(),
            api_key_env: Some("X_API_KEY".into()),
            protocol: Protocol::ChatCompletions,
            options: ProviderOptions::default(),
            models: BTreeMap::new(),
        };
        assert_eq!(
            chat.default_auth().credentials,
            crate::protocol::CredentialSource::EnvVar("X_API_KEY".into())
        );
        assert_eq!(
            chat.default_auth().scheme,
            crate::protocol::AuthScheme::Bearer
        );

        let anthropic = ProviderSpec {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
            protocol: Protocol::AnthropicMessages,
            options: ProviderOptions::default(),
            models: BTreeMap::new(),
        };
        assert_eq!(
            anthropic.default_auth().scheme,
            crate::protocol::AuthScheme::ApiKey {
                header: "x-api-key".into()
            }
        );
    }
}
