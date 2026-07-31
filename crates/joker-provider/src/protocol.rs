//! Wire-level protocol types and routing primitives.
//!
//! Defines [`Protocol`], [`Framing`], [`AuthScheme`], [`CredentialSource`],
//! [`Auth`], and [`Route`] — the building blocks for describing a provider
//! endpoint's wire format, authentication, and transport framing.

use std::sync::Arc;

use joker::Model;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Wire-level protocol / API format used by a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    /// OpenAI-compatible `/v1/chat/completions` style payloads.
    ChatCompletions,
    /// Native Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
    /// Google Gemini `streamGenerateContent` API.
    GoogleGemini,
}

/// Transport framing for streaming responses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Framing {
    /// Server-sent events.
    Sse,
    /// Google-style `StreamableHttp` (alternate SSE framing).
    StreamableHttp,
}

/// Authentication scheme for a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialSource {
    /// No credential available.
    None,
    /// Read from the named environment variable at runtime.
    EnvVar(String),
    /// An in-memory value supplied directly.
    Value(String),
}

/// Authentication bundle for a [`Route`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    /// Authentication scheme (bearer, API key header, or none).
    pub scheme: AuthScheme,
    /// Where credential values come from.
    pub credentials: CredentialSource,
}

impl Auth {
    /// Create an unauthenticated [`Auth`] bundle.
    pub const fn none() -> Self {
        Self {
            scheme: AuthScheme::None,
            credentials: CredentialSource::None,
        }
    }

    /// Create a bearer-token [`Auth`] sourced from an environment variable.
    pub fn bearer_from_env(env_var: &str) -> Self {
        Self {
            scheme: AuthScheme::Bearer,
            credentials: CredentialSource::EnvVar(env_var.into()),
        }
    }

    /// Create an API-key-header [`Auth`] sourced from an environment variable.
    pub fn api_key_from_env(header: &str, env_var: &str) -> Self {
        Self {
            scheme: AuthScheme::ApiKey {
                header: header.into(),
            },
            credentials: CredentialSource::EnvVar(env_var.into()),
        }
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
            (AuthScheme::Bearer, CredentialSource::EnvVar(name)) => std::env::var(name)
                .ok()
                .map(|v| ("Authorization".into(), format!("Bearer {v}"))),
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
/// unit that can materialize [`Model`](joker::Model) instances. When built from
/// a [`ProviderSpec`](crate::spec::ProviderSpec), the spec is retained so model
/// construction can consult catalog capabilities, limits, and options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The provider spec this route was built from, if any.
    #[serde(default)]
    pub spec: Option<crate::spec::ProviderSpec>,
    /// Runtime credential store consulted during model construction.
    ///
    /// Not serialized: this is runtime state attached by the host
    /// (e.g. the TUI) so [`Route::build_model`] can resolve credentials
    /// through the unified chain in [`resolve_auth`](crate::auth::resolve_auth).
    #[serde(skip)]
    pub credential_store: Option<joker::CredentialStore>,
}

impl Route {
    /// Build a [`Route`] from a provider spec and optional model override.
    ///
    /// Framing is derived from the protocol (Google uses `StreamableHttp`,
    /// everything else uses SSE). The default model falls back to the first
    /// entry of the spec's catalog when no override is given.
    #[must_use]
    pub fn from_spec(spec: &crate::spec::ProviderSpec, model: Option<&str>) -> Self {
        let default_model = model
            .map(String::from)
            .or_else(|| spec.models.keys().next().cloned())
            .unwrap_or_default();
        Self {
            id: spec.id.clone(),
            protocol: spec.protocol.clone(),
            base_url: spec.base_url.clone(),
            auth: spec.default_auth(),
            framing: match spec.protocol {
                Protocol::GoogleGemini => Framing::StreamableHttp,
                Protocol::ChatCompletions | Protocol::AnthropicMessages => Framing::Sse,
            },
            default_model,
            spec: Some(spec.clone()),
            credential_store: None,
        }
    }

    /// Attach a credential store consulted during model construction.
    #[must_use]
    pub fn with_credential_store(mut self, store: joker::CredentialStore) -> Self {
        self.credential_store = Some(store);
        self
    }

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
    /// [`Route::protocol`]. Catalog data (limits, capabilities, options) from
    /// the route's spec is passed into the concrete model configuration.
    pub fn do_build_model(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        debug!(target: "protocol", model = %model, protocol = ?self.protocol, id = %self.id, "building model");
        let auth = crate::auth::resolve_auth(&self.id, &self.auth, self.credential_store.as_ref());
        let api_key = match &auth.credentials {
            CredentialSource::Value(v) => Some(v.clone()),
            CredentialSource::EnvVar(name) => std::env::var(name).ok(),
            CredentialSource::None => None,
        };
        let mut headers: Vec<(String, String)> = self
            .spec
            .as_ref()
            .map(|spec| {
                spec.options
                    .headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if let Some((name, value)) = auth.header_value() {
            headers.push((name, value));
        }
        let model_info = self.model_info(model);
        let provider_options = self.spec.as_ref().map(|spec| &spec.options);

        match self.protocol {
            Protocol::ChatCompletions => {
                let extra_body = merge_extra_body(
                    provider_options.and_then(|opts| opts.extra_body.clone()),
                    model_info
                        .as_ref()
                        .and_then(|info| info.options.extra_body.clone()),
                );
                let config = crate::openai::OpenAiCompatibleConfig {
                    provider_name: self.id.clone(),
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key,
                    api_key_env: match &auth.credentials {
                        CredentialSource::EnvVar(n) => Some(n.clone()),
                        _ => None,
                    },
                    require_api_key: auth.credentials != CredentialSource::None,
                    extra_body,
                    reasoning: model_info.as_ref().map(|info| info.capabilities.reasoning),
                    headers,
                };
                Ok(Arc::new(
                    crate::openai::OpenAiCompatibleModel::new(config).map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::AnthropicMessages => {
                let key = api_key.ok_or_else(|| missing_key_message("Anthropic", &auth))?;
                // Version header default for routes built without a spec that
                // supplies its own (the ANTHROPIC catalog spec carries it).
                if !headers.iter().any(|(name, _)| name == "anthropic-version") {
                    headers.push((
                        "anthropic-version".into(),
                        crate::anthropic::ANTHROPIC_VERSION.into(),
                    ));
                }
                let config = crate::anthropic::AnthropicConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                    max_tokens: model_info
                        .as_ref()
                        .and_then(|info| non_zero_limit(info.limit.max_output)),
                    headers,
                };
                Ok(Arc::new(
                    crate::anthropic::AnthropicModel::new(config).map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::GoogleGemini => {
                let key = api_key.ok_or_else(|| missing_key_message("Google", &auth))?;
                let config = crate::google::GoogleConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                    headers,
                };
                Ok(
                    Arc::new(crate::google::GoogleModel::new(config).map_err(|e| e.to_string())?)
                        as Arc<dyn Model>,
                )
            }
        }
    }

    /// Look up [`ModelInfo`](crate::spec::ModelInfo) for `model`, preferring the
    /// route's own spec and falling back to the built-in catalog by route id.
    fn model_info(&self, model: &str) -> Option<crate::spec::ModelInfo> {
        let spec = self
            .spec
            .as_ref()
            .or_else(|| crate::catalog::preset_spec(&self.id));
        spec.and_then(|spec| spec.models.get(model)).cloned()
    }
}

/// Human-readable message for a missing API key, naming the expected env var.
fn missing_key_message(provider: &str, auth: &Auth) -> String {
    match &auth.credentials {
        CredentialSource::EnvVar(name) => format!(
            "{provider} API key not configured (set {name} or enter it via /provider)"
        ),
        _ => format!("{provider} API key not configured"),
    }
}

/// Merge provider-level and model-level extra body fields (model wins).
fn merge_extra_body(a: Option<serde_json::Value>, b: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match (a, b) {
        (Some(serde_json::Value::Object(mut base)), Some(serde_json::Value::Object(overlay))) => {
            base.extend(overlay);
            Some(serde_json::Value::Object(base))
        }
        (_, Some(overlay)) => Some(overlay),
        (base, None) => base,
    }
}

fn non_zero_limit(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
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
            auth: Auth {
                scheme: AuthScheme::Bearer,
                credentials: CredentialSource::EnvVar("OPENAI_API_KEY".into()),
            },
            framing: Framing::Sse,
            default_model: "gpt-4".into(),
            spec: None,
            credential_store: None,
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
        assert_eq!(
            auth.scheme,
            AuthScheme::ApiKey {
                header: "x-api-key".into()
            }
        );
        assert_eq!(
            auth.credentials,
            CredentialSource::EnvVar("ANTHROPIC_API_KEY".into())
        );
    }

    #[test]
    fn auth_none_has_no_header() {
        assert!(Auth::none().header_value().is_none());
    }
}
