//! Provider selection — maps provider names to routes or scripted mode.

use joker_provider::{Route, ALIBABA, ANTHROPIC, BAIDU, DEEPSEEK, GOOGLE, MOONSHOT, ZHIPUAI};
use tracing::info;

use crate::error::ConfigError;

/// Represents the active provider: either a scripted echo provider or a routed LLM.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub fn preset(provider: &str) -> Result<Self, ConfigError> {
        info!(target: "config", provider = %provider, "selecting provider");
        match provider.trim().to_ascii_lowercase().as_str() {
            "" | "scripted" => Ok(Self::scripted()),
            "deepseek" => Ok(Self::Route(DEEPSEEK.into_route(Some("deepseek-chat")))),
            "anthropic" => Ok(Self::Route(ANTHROPIC.into_route(Some("claude-sonnet-4-20250514")))),
            "google" => Ok(Self::Route(GOOGLE.into_route(Some("gemini-2-5-flash")))),
            "alibaba" | "dashscope" => Ok(Self::Route(ALIBABA.into_route(Some("qwen-plus")))),
            "zhipuai" | "glm" => Ok(Self::Route(ZHIPUAI.into_route(Some("glm-4-plus")))),
            "moonshot" | "kimi" => Ok(Self::Route(MOONSHOT.into_route(Some("kimi-k2.5")))),
            "baidu" | "ernie" => Ok(Self::Route(BAIDU.into_route(Some("ernie-4.0")))),
            "openai-compatible" | "custom" => Ok(Self::Route(Route {
                id: "openai-compatible".into(),
                protocol: joker_provider::Protocol::ChatCompletions,
                base_url: "http://localhost:8000/v1".into(),
                auth: joker_provider::Auth::bearer_from_env("OPENAI_COMPATIBLE_API_KEY"),
                framing: joker_provider::Framing::Sse,
                default_model: "model".into(),
            })),
            other => Err(ConfigError::UnknownProvider(other.into())),
        }
    }
}
