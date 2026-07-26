//! Wire-level protocol types and routing primitives.
//!
//! Defines [`Protocol`], [`Framing`], [`AuthScheme`], [`CredentialSource`],
//! [`Auth`], and [`Route`] — the building blocks for describing a provider
//! endpoint's wire format, authentication, and transport framing.

use std::sync::Arc;

use joker::Model;
use tracing::debug;

/// Wire-level protocol / API format used by a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// OpenAI-compatible `/v1/chat/completions` style payloads.
    ChatCompletions,
    /// Native Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
    /// Google Gemini `streamGenerateContent` API.
    GoogleGemini,
}

/// Transport framing for streaming responses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Framing {
    /// Server-sent events.
    Sse,
    /// Google-style `StreamableHttp` (alternate SSE framing).
    StreamableHttp,
}

/// Authentication scheme for a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthScheme {
    /// HTTP `Authorization: Bearer <token>`.
    Bearer,
    /// Custom header-based API key (e.g. `x-api-key`).
    ApiKey {
        /// HTTP header name (e.g. `"x-api-key"`).
        header: String,
    },
    /// No authentication.
    None,
}

/// Where credential values come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    /// No credential available.
    None,
    /// Read from the named environment variable at runtime.
    EnvVar(String),
    /// An in-memory value supplied directly.
    Value(String),
}

/// Authentication bundle for a [`Route`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auth {
    /// Authentication scheme (bearer, API key header, or none).
    pub scheme: AuthScheme,
    /// Where credential values come from.
    pub credentials: CredentialSource,
}

impl Auth {
    /// Create an unauthenticated [`Auth`] bundle.
    pub const fn none() -> Self {
        Self { scheme: AuthScheme::None, credentials: CredentialSource::None }
    }

    /// Create a bearer-token [`Auth`] sourced from an environment variable.
    pub fn bearer_from_env(env_var: &str) -> Self {
        Self { scheme: AuthScheme::Bearer, credentials: CredentialSource::EnvVar(env_var.into()) }
    }

    /// Create an API-key-header [`Auth`] sourced from an environment variable.
    pub fn api_key_from_env(header: &str, env_var: &str) -> Self {
        Self { scheme: AuthScheme::ApiKey { header: header.into() }, credentials: CredentialSource::EnvVar(env_var.into()) }
    }

    /// Resolve the concrete HTTP header name and value, if available.
    ///
    /// Returns `None` when credentials are unavailable or the scheme is
    /// [`AuthScheme::None`].
    pub fn header_value(&self) -> Option<(String, String)> {
        match (&self.scheme, &self.credentials) {
            (AuthScheme::Bearer, CredentialSource::Value(v)) => {
                Some(("Authorization".into(), format!("Bearer {v}")))
            }
            (AuthScheme::Bearer, CredentialSource::EnvVar(name)) => {
                std::env::var(name).ok().map(|v| ("Authorization".into(), format!("Bearer {v}")))
            }
            (AuthScheme::ApiKey { header }, CredentialSource::Value(v)) => {
                Some((header.clone(), v.clone()))
            }
            (AuthScheme::ApiKey { header }, CredentialSource::EnvVar(name)) => {
                std::env::var(name).ok().map(|v| (header.clone(), v))
            }
            _ => None,
        }
    }
}

/// A fully-described provider endpoint.
///
/// Combines a [`Protocol`], base URL, [`Auth`], and [`Framing`] into a routable
/// unit that can materialize [`Model`](joker::Model) instances.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// Unique identifier for this route.
    pub id: String,
    /// Wire protocol used by this endpoint.
    pub protocol: Protocol,
    /// Base URL of the provider API.
    pub base_url: String,
    /// Authentication configuration.
    pub auth: Auth,
    /// Streaming transport framing.
    pub framing: Framing,
    /// Default model identifier.
    pub default_model: String,
}

impl Route {
    /// Build a [`Model`](joker::Model) using the default model identifier.
    pub fn build_model(&self) -> Result<Arc<dyn Model>, String> {
        self.do_build_model(&self.default_model)
    }

    /// Build a [`Model`](joker::Model) for a specific model identifier.
    pub fn build_model_for(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        self.do_build_model(model)
    }

    /// Internal helper that materializes a [`Model`](joker::Model) from a model identifier.
    ///
    /// Dispatches to the appropriate provider implementation based on
    /// [`Route::protocol`].
    pub fn do_build_model(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        debug!(target: "protocol", model = %model, protocol = ?self.protocol, id = %self.id, "building model");
        let api_key = match &self.auth.credentials {
            CredentialSource::Value(v) => Some(v.clone()),
            CredentialSource::EnvVar(name) => std::env::var(name).ok(),
            CredentialSource::None => None,
        };

        match self.protocol {
            Protocol::ChatCompletions => {
                let config = crate::openai::OpenAiCompatibleConfig {
                    provider_name: self.id.clone(),
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key,
                    api_key_env: match &self.auth.credentials {
                        CredentialSource::EnvVar(n) => Some(n.clone()),
                        _ => None,
                    },
                    require_api_key: self.auth.credentials != CredentialSource::None,
                    extra_body: None,
                };
                Ok(Arc::new(
                    crate::openai::OpenAiCompatibleModel::new(config)
                        .map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::AnthropicMessages => {
                let key = api_key.ok_or_else(|| "Anthropic API key not configured".to_string())?;
                let config = crate::anthropic::AnthropicConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                };
                Ok(Arc::new(
                    crate::anthropic::AnthropicModel::new(config)
                        .map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::GoogleGemini => {
                let key = api_key.ok_or_else(|| "Google API key not configured".to_string())?;
                let config = crate::google::GoogleConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                };
                Ok(Arc::new(
                    crate::google::GoogleModel::new(config).map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_builds_openai_model() {
        let route = Route {
            id: "test".into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://api.openai.com/v1".into(),
            auth: Auth { scheme: AuthScheme::Bearer, credentials: CredentialSource::EnvVar("OPENAI_API_KEY".into()) },
            framing: Framing::Sse,
            default_model: "gpt-4".into(),
        };
        let _ = route.build_model();
    }

    #[test]
    fn auth_bearer_from_env() {
        let auth = Auth::bearer_from_env("MY_KEY");
        assert_eq!(auth.scheme, AuthScheme::Bearer);
        assert_eq!(auth.credentials, CredentialSource::EnvVar("MY_KEY".into()));
    }

    #[test]
    fn auth_api_key_from_env() {
        let auth = Auth::api_key_from_env("x-api-key", "ANTHROPIC_API_KEY");
        assert_eq!(auth.scheme, AuthScheme::ApiKey { header: "x-api-key".into() });
        assert_eq!(auth.credentials, CredentialSource::EnvVar("ANTHROPIC_API_KEY".into()));
    }

    #[test]
    fn auth_none_has_no_header() {
        assert!(Auth::none().header_value().is_none());
    }
}
