//! Built-in provider profiles.
//!
//! Each [`ProviderProfile`] constant carries the default base URL, API key
//! environment variable name, wire [`Protocol`](crate::protocol::Protocol), and
//! [`Framing`](crate::protocol::Framing) for a well-known provider. Use
//! [`ProviderProfile::into_route`] to materialize a [`Route`](crate::protocol::Route).

use crate::protocol::{Auth, Framing, Protocol, Route};

/// A known provider profile for quick configuration.
///
/// Stores the canonical defaults for one provider. Convert to a
/// [`Route`](crate::protocol::Route) via [`into_route`](ProviderProfile::into_route).
#[derive(Clone, Debug)]
pub struct ProviderProfile {
    /// Machine-readable provider identifier (e.g. `"deepseek"`).
    pub id: &'static str,
    /// Human-readable display name (e.g. `"DeepSeek"`).
    pub name: &'static str,
    /// Default base URL for the provider API.
    pub base_url: &'static str,
    /// Name of the environment variable that holds the API key.
    pub api_key_env: &'static str,
    /// Wire protocol used by this provider.
    pub protocol: Protocol,
    /// Streaming transport framing.
    pub framing: Framing,
}

impl ProviderProfile {
    /// Build an [`Auth`](crate::protocol::Auth) using the profile's default scheme and env var.
    pub fn default_auth(&self) -> Auth {
        match self.protocol {
            Protocol::ChatCompletions => Auth::bearer_from_env(self.api_key_env),
            Protocol::AnthropicMessages => Auth::api_key_from_env("x-api-key", self.api_key_env),
            Protocol::GoogleGemini => Auth::api_key_from_env("x-goog-api-key", self.api_key_env),
        }
    }

    /// Convert this profile into a [`Route`](crate::protocol::Route).
    ///
    /// The optional `model` overrides the default model identifier; `None` uses
    /// an empty string (caller should set it later).
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

/// Anthropic Messages API profile.
pub const ANTHROPIC: ProviderProfile = ProviderProfile {
    id: "anthropic",
    name: "Anthropic",
    base_url: "https://api.anthropic.com",
    api_key_env: "ANTHROPIC_API_KEY",
    protocol: Protocol::AnthropicMessages,
    framing: Framing::Sse,
};

/// Google Gemini API profile.
pub const GOOGLE: ProviderProfile = ProviderProfile {
    id: "google",
    name: "Google",
    base_url: "https://generativelanguage.googleapis.com",
    api_key_env: "GOOGLE_GENERATIVE_AI_API_KEY",
    protocol: Protocol::GoogleGemini,
    framing: Framing::StreamableHttp,
};

/// DeepSeek API profile (OpenAI-compatible).
pub const DEEPSEEK: ProviderProfile = ProviderProfile {
    id: "deepseek",
    name: "DeepSeek",
    base_url: "https://api.deepseek.com",
    api_key_env: "DEEPSEEK_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

/// Alibaba DashScope API profile (OpenAI-compatible).
pub const ALIBABA: ProviderProfile = ProviderProfile {
    id: "alibaba",
    name: "Alibaba",
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    api_key_env: "ALIBABA_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

/// ZhipuAI GLM API profile (OpenAI-compatible).
pub const ZHIPUAI: ProviderProfile = ProviderProfile {
    id: "zhipuai",
    name: "ZhipuAI",
    base_url: "https://open.bigmodel.cn/api/paas/v4",
    api_key_env: "ZHIPUAI_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

/// Moonshot / Kimi API profile (OpenAI-compatible).
pub const MOONSHOT: ProviderProfile = ProviderProfile {
    id: "moonshot",
    name: "Moonshot",
    base_url: "https://api.moonshot.cn/v1",
    api_key_env: "MOONSHOT_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};

/// Baidu Qianfan API profile (OpenAI-compatible).
pub const BAIDU: ProviderProfile = ProviderProfile {
    id: "baidu",
    name: "Baidu",
    base_url: "https://qianfan.baidubce.com/v2",
    api_key_env: "BAIDU_API_KEY",
    protocol: Protocol::ChatCompletions,
    framing: Framing::Sse,
};
