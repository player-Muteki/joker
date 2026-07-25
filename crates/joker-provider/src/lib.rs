#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

pub mod anthropic;
pub mod google;
pub mod openai;
pub mod transform;

// Re-export items used by joker-config and other consumers
pub use openai::{
    OpenAiCompatibleConfig, OpenAiCompatibleModel, OpenAiProviderError,
};

use std::sync::Arc;

use joker::Model;
use serde::Deserialize;

// ── 4-axis Route decomposition ──────────────────────────────────────────────

/// Wire-level protocol / API format used by a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// OpenAI `/v1/chat/completions` format (also used by DeepSeek, Together, Groq, …).
    ChatCompletions,
    /// Anthropic `/v1/messages` API.
    AnthropicMessages,
    /// Google Gemini `generateContent` API.
    GoogleGemini,
}

/// Transport framing for streaming responses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Framing {
    /// Server-sent events (`text/event-stream`).
    Sse,
    /// Google-style streaming HTTP (chunked transfer).
    StreamableHttp,
}

/// Authentication scheme for a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <token>`
    Bearer,
    /// Custom header-based auth (e.g. `x-api-key: <value>`).
    ApiKey { header: String },
    /// No authentication (local endpoints, etc.).
    None,
}

/// Where credential values come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    /// No credential available.
    None,
    /// Read from an environment variable at runtime.
    EnvVar(String),
    /// An explicit value provided inline.
    Value(String),
}

/// A fully-specified Route composing the four orthogonal axes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub id: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub auth: Auth,
    pub framing: Framing,
    pub default_model: String,
}

impl Route {
    /// Build a Model from this route.
    pub fn build_model(&self) -> Result<Arc<dyn Model>, String> {
        self.do_build_model(&self.default_model)
    }

    /// Build a Model from this route for a specific model name.
    pub fn build_model_for(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        self.do_build_model(model)
    }

    fn do_build_model(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        let api_key = match &self.auth.credentials {
            CredentialSource::Value(v) => Some(v.clone()),
            CredentialSource::EnvVar(name) => std::env::var(name).ok(),
            CredentialSource::None => None,
        };

        match self.protocol {
            Protocol::ChatCompletions => {
                let config = openai::OpenAiCompatibleConfig {
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
                    openai::OpenAiCompatibleModel::new(config)
                        .map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::AnthropicMessages => {
                let key =
                    api_key.ok_or_else(|| "Anthropic API key not configured".to_string())?;
                let config = anthropic::AnthropicConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                };
                Ok(Arc::new(
                    anthropic::AnthropicModel::new(config)
                        .map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::GoogleGemini => {
                let key =
                    api_key.ok_or_else(|| "Google API key not configured".to_string())?;
                let config = google::GoogleConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                };
                Ok(Arc::new(
                    google::GoogleModel::new(config).map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
        }
    }
}

/// Authentication bundle for a Route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auth {
    pub scheme: AuthScheme,
    pub credentials: CredentialSource,
}

impl Auth {
    pub const fn none() -> Self {
        Self {
            scheme: AuthScheme::None,
            credentials: CredentialSource::None,
        }
    }

    pub fn bearer_from_env(env_var: &str) -> Self {
        Self {
            scheme: AuthScheme::Bearer,
            credentials: CredentialSource::EnvVar(env_var.into()),
        }
    }

    pub fn api_key_from_env(header: &str, env_var: &str) -> Self {
        Self {
            scheme: AuthScheme::ApiKey {
                header: header.into(),
            },
            credentials: CredentialSource::EnvVar(env_var.into()),
        }
    }

    /// Resolve the effective `Authorization` header value, if any.
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

// ── Model auto-discovery via /v1/models ─────────────────────────────────────

/// Response from an OpenAI-compatible `/v1/models` endpoint.
#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// Discover model IDs from a provider endpoint by calling `GET /v1/models`.
///
/// Tries multiple candidate URLs in order:
/// 1. `{base_url}/models`
/// 2. `{base_url}/v1/models`
/// 3. Strips known suffix paths and retries
pub async fn discover_models(base_url: &str, auth: &Auth) -> Result<Vec<String>, String> {
    let candidates = build_model_fetch_urls(base_url);

    for url in &candidates {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let mut req = client.get(url);

        if let Some((header, value)) = auth.header_value() {
            req = req.header(&header, &value);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: ModelsResponse = resp.json().await.map_err(|e| {
                    format!("failed to parse models response from {url}: {e}")
                })?;
                return Ok(body.data.into_iter().map(|e| e.id).collect());
            }
            Ok(resp) if url == candidates.last().unwrap() => {
                return Err(format!(
                    "model discovery failed for all candidates; last attempt ({url}): {}",
                    resp.status()
                ));
            }
            _ => continue,
        }
    }

    Err(format!(
        "no candidate model URLs worked for base_url: {base_url}"
    ))
}

/// Build candidate model-fetch URLs, following the DeepSeek-Reasonix pattern.
fn build_model_fetch_urls(base_url: &str) -> Vec<String> {
    let base = base_url.trim_end_matches('/');

    let mut candidates = Vec::new();
    candidates.push(format!("{base}/models"));

    if !base.ends_with("/v1") {
        candidates.push(format!("{base}/v1/models"));
    }

    // Try stripping known suffix paths
    for suffix in &["/api/anthropic", "/api/claudecode", "/api/coding"] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            let stripped = stripped.trim_end_matches('/');
            candidates.push(format!("{stripped}/models"));
            candidates.push(format!("{stripped}/v1/models"));
        }
    }

    candidates
}

// ── Vendor auto-detection from URL ──────────────────────────────────────────

/// Heuristically detect the provider vendor from a base URL.
/// This mirrors DeepSeek-Reasonix's `host.go` approach.
pub fn detect_vendor(base_url: &str) -> &'static str {
    let host = base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(base_url);

    if host.contains("deepseek") {
        "deepseek"
    } else if host.contains("anthropic") {
        "anthropic"
    } else if host.contains("googleapis") || host.contains("generativelanguage") {
        "google"
    } else if host.contains("openai") {
        "openai"
    } else if host.contains("alibaba") || host.contains("dashscope") {
        "alibaba"
    } else if host.contains("zhipu") || host.contains("bigmodel") {
        "zhipuai"
    } else if host.contains("moonshot") || host.contains("kimi") {
        "moonshot"
    } else if host.contains("baidu") || host.contains("ernie") || host.contains("qianfan") {
        "baidu"
    } else if host.contains("xai") || host.contains("x-ai") {
        "xai"
    } else if host.contains("groq") {
        "groq"
    } else if host.contains("togetherai") || host.contains("together") {
        "togetherai"
    } else if host.contains("fireworks") {
        "fireworks"
    } else if host.contains("deepinfra") {
        "deepinfra"
    } else if host.contains("cerebras") {
        "cerebras"
    } else {
        "unknown"
    }
}

// ── Route helpers ───────────────────────────────────────────────────────────

/// Guess the Protocol from a base URL and optional vendor hint.
pub fn guess_protocol(base_url: &str) -> Protocol {
    let vendor = detect_vendor(base_url);
    match vendor {
        "anthropic" => Protocol::AnthropicMessages,
        "google" => Protocol::GoogleGemini,
        _ => Protocol::ChatCompletions,
    }
}

/// Guess framing from a Protocol.
pub fn guess_framing(protocol: &Protocol) -> Framing {
    match protocol {
        Protocol::GoogleGemini => Framing::StreamableHttp,
        _ => Framing::Sse,
    }
}

/// Guess auth scheme from a Protocol.
pub fn guess_auth(protocol: &Protocol) -> Auth {
    match protocol {
        Protocol::ChatCompletions => Auth::bearer_from_env("OPENAI_COMPATIBLE_API_KEY"),
        Protocol::AnthropicMessages => {
            Auth::api_key_from_env("x-api-key", "ANTHROPIC_API_KEY")
        }
        Protocol::GoogleGemini => {
            Auth::api_key_from_env("x-goog-api-key", "GOOGLE_GENERATIVE_AI_API_KEY")
        }
    }
}

// ── Known provider profiles (for quick setup, NOT mandatory) ────────────────
// These are optional presets for "deepseek", "anthropic", etc.
// Model auto-discovery via /v1/models is the primary mechanism.

/// A known provider profile for quick configuration.
///
/// These are optional presets — model auto-discovery via `/v1/models`
/// is the primary mechanism for discovering available models.
#[derive(Clone, Debug)]
pub struct ProviderProfile {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_key_env: &'static str,
    pub protocol: Protocol,
    pub framing: Framing,
}

impl ProviderProfile {
    /// Build the appropriate Auth for this profile's protocol.
    pub fn default_auth(&self) -> Auth {
        match self.protocol {
            Protocol::ChatCompletions => Auth::bearer_from_env(self.api_key_env),
            Protocol::AnthropicMessages => {
                Auth::api_key_from_env("x-api-key", self.api_key_env)
            }
            Protocol::GoogleGemini => {
                Auth::api_key_from_env("x-goog-api-key", self.api_key_env)
            }
        }
    }

    /// Convert this profile into a concrete `Route`.
    pub fn into_route(&self, model: Option<&str>) -> Route {
        Route {
            id: self.id.into(),
            protocol: self.protocol.clone(),
            base_url: self.base_url.into(),
            auth: self.default_auth(),
            framing: self.framing.clone(),
            default_model: model.unwrap_or("").into(),
        }
    }
}

pub const ANTHROPIC: ProviderProfile = ProviderProfile {
    id: "anthropic",
    name: "Anthropic",
    base_url: "https://api.anthropic.com",
    api_key_env: "ANTHROPIC_API_KEY",
    protocol: Protocol::AnthropicMessages,
    framing: Framing::Sse,
};

pub const GOOGLE: ProviderProfile = ProviderProfile {
    id: "google",
    name: "Google",
    base_url: "https://generativelanguage.googleapis.com",
    api_key_env: "GOOGLE_GENERATIVE_AI_API_KEY",
    protocol: Protocol::GoogleGemini,
    framing: Framing::StreamableHttp,
};

pub const DEEPSEEK: ProviderProfile = ProviderProfile {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com",
    api_key_env: "DEEPSEEK_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const ALIBABA: ProviderProfile = ProviderProfile {
    id: "alibaba",
    name: "Alibaba",
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    api_key_env: "ALIBABA_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const ZHIPUAI: ProviderProfile = ProviderProfile {
    id: "zhipuai",
    name: "ZhipuAI",
    base_url: "https://open.bigmodel.cn/api/paas/v4",
    api_key_env: "ZHIPUAI_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const MOONSHOT: ProviderProfile = ProviderProfile {
    id: "moonshot",
    name: "Moonshot",
    base_url: "https://api.moonshot.cn/v1",
    api_key_env: "MOONSHOT_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

pub const BAIDU: ProviderProfile = ProviderProfile {
    id: "baidu",
    name: "Baidu",
    base_url: "https://qianfan.baidubce.com/v2",
    api_key_env: "BAIDU_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

// ── Deprecated backward-compat API ──────────────────────────────────────────

#[deprecated(note = "use Route + Protocol/Endpoint/Auth/Framing instead")]
pub type ProviderDescriptor = ProviderProfile;

/// Build a Model from a well-known provider name (legacy).
///
/// Prefer constructing a `Route` directly and calling `route.build_model()`.
#[deprecated(note = "use Route::build_model() or build_model_from_route() instead")]
pub fn build_model(
    name: &str,
    _api_key: Option<String>,
    _base_url: Option<String>,
    model: String,
) -> Option<Arc<dyn Model>> {
    match name {
        "anthropic" => Some(
            ANTHROPIC.into_route(Some(&model)).do_build_model(&model).ok()?,
        ),
        "google" => Some(
            GOOGLE.into_route(Some(&model)).do_build_model(&model).ok()?,
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discover_models_fallback_no_network() {
        // When the endpoint is unreachable, discover_models returns an error
        let result = discover_models("http://127.0.0.1:1", &Auth::none()).await;
        assert!(result.is_err(), "should fail on unreachable endpoint");
    }

    #[test]
    fn build_model_fetch_urls_standard() {
        let urls = build_model_fetch_urls("https://api.deepseek.com");
        assert!(urls.contains(&"https://api.deepseek.com/models".into()));
        assert!(urls.contains(&"https://api.deepseek.com/v1/models".into()));
    }

    #[test]
    fn build_model_fetch_urls_with_suffix() {
        let urls = build_model_fetch_urls("https://api.openai.com/v1");
        assert!(urls.contains(&"https://api.openai.com/v1/models".into()));
        // /v1/models should NOT be added (base already ends with /v1)
        assert!(!urls.contains(&"https://api.openai.com/v1/v1/models".into()));
    }

    #[test]
    fn detect_vendor_identifies_known_providers() {
        assert_eq!(detect_vendor("https://api.deepseek.com"), "deepseek");
        assert_eq!(
            detect_vendor("https://api.anthropic.com/v1"),
            "anthropic"
        );
        assert_eq!(
            detect_vendor("https://generativelanguage.googleapis.com"),
            "google"
        );
        assert_eq!(detect_vendor("https://api.openai.com/v1"), "openai");
        assert_eq!(
            detect_vendor("https://api.groq.com/openai/v1"),
            "groq"
        );
    }

    #[test]
    fn detect_vendor_unknown_default() {
        assert_eq!(
            detect_vendor("https://my-custom-llm.example.com"),
            "unknown"
        );
    }

    #[test]
    fn guess_protocol_by_vendor() {
        assert_eq!(guess_protocol("https://api.anthropic.com"), Protocol::AnthropicMessages);
        assert_eq!(
            guess_protocol("https://generativelanguage.googleapis.com"),
            Protocol::GoogleGemini
        );
        assert_eq!(
            guess_protocol("https://api.deepseek.com"),
            Protocol::ChatCompletions
        );
    }

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
        };
        // Just verify it doesn't panic — will fail at runtime without API key
        let _ = route.build_model();
    }

    #[test]
    fn profile_into_route() {
        let route = ANTHROPIC.into_route(Some("claude-sonnet-4-20250514"));
        assert_eq!(route.protocol, Protocol::AnthropicMessages);
        assert_eq!(route.base_url, "https://api.anthropic.com");
        assert_eq!(route.default_model, "claude-sonnet-4-20250514");
        assert_eq!(route.framing, Framing::Sse);
    }

    #[test]
    fn auth_bearer_from_env() {
        let auth = Auth::bearer_from_env("MY_KEY");
        assert_eq!(auth.scheme, AuthScheme::Bearer);
        assert_eq!(
            auth.credentials,
            CredentialSource::EnvVar("MY_KEY".into())
        );
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

    #[test]
    fn routing_detects_unknown_vendor() {
        assert_eq!(
            detect_vendor("http://localhost:11434"),  // Ollama
            "unknown"
        );
    }
}
