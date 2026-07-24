#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

pub mod anthropic;
pub mod google;
pub mod openai;
pub mod transform;

use std::sync::Arc;

use joker::Model;

/// A descriptor for a known provider — used for presets and display.
#[derive(Clone, Debug)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_key_env: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
}

pub const ANTHROPIC: ProviderDescriptor = ProviderDescriptor {
    id: "anthropic",
    name: "Anthropic",
    base_url: "https://api.anthropic.com",
    api_key_env: "ANTHROPIC_API_KEY",
    default_model: "claude-sonnet-4-20250514",
    models: &[
        "claude-sonnet-4-20250514",
        "claude-3-5-sonnet-latest",
        "claude-3-opus-latest",
        "claude-3-haiku-latest",
    ],
};

pub const GOOGLE: ProviderDescriptor = ProviderDescriptor {
    id: "google",
    name: "Google",
    base_url: "https://generativelanguage.googleapis.com",
    api_key_env: "GOOGLE_GENERATIVE_AI_API_KEY",
    default_model: "gemini-2-5-flash",
    models: &[
        "gemini-2-5-flash",
        "gemini-2-5-pro",
        "gemini-2-0-flash",
    ],
};

// Re-export openai module items at crate root
pub use openai::{
    ALIBABA, BAIDU, DEEPSEEK, MOONSHOT, ZHIPUAI, OpenAiCompatibleConfig, OpenAiCompatibleModel,
    OpenAiProviderError,
};

/// Build an `Arc<dyn Model>` from a well-known provider name.
///
/// Returns `None` if the name is not a recognised built-in provider.
pub fn build_model(
    name: &str,
    api_key: Option<String>,
    base_url: Option<String>,
    model: String,
) -> Option<Arc<dyn Model>> {
    match name {
        "anthropic" => {
            let key = api_key.or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())?;
            Some(Arc::new(
                anthropic::AnthropicModel::new(
                    anthropic::AnthropicConfig { base_url: base_url.unwrap_or_else(|| ANTHROPIC.base_url.into()), model, api_key: key },
                )
                .ok()?,
            )
                as Arc<dyn Model>)
        }
        "google" => {
            let key = api_key.or_else(|| std::env::var("GOOGLE_GENERATIVE_AI_API_KEY").ok())?;
            Some(Arc::new(
                google::GoogleModel::new(
                    google::GoogleConfig { base_url: base_url.unwrap_or_else(|| GOOGLE.base_url.into()), model, api_key: key },
                )
                .ok()?,
            )
                as Arc<dyn Model>)
        }
        _ => None,
    }
}
