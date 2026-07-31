//! Built-in provider catalog.
//!
//! Each [`ProviderSpec`] constant carries the default base URL, API key
//! environment variable, wire protocol, and model catalog for a well-known
//! provider. This replaces the older `ProviderProfile` constants — specs are
//! data-driven, so adding a provider (or a model to an existing one) is a
//! pure data change. Mirrors opencode's catalog-driven provider design.

use std::sync::LazyLock;

use crate::protocol::Protocol;
use crate::spec::{
    ModelCapabilities, ModelInfo, ModelLimit, ModelOptions, ProviderOptions, ProviderSpec,
};

/// DeepSeek (OpenAI-compatible).
pub static DEEPSEEK: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "deepseek".into(),
    name: "DeepSeek".into(),
    base_url: "https://api.deepseek.com".into(),
    api_key_env: Some("DEEPSEEK_API_KEY".into()),
    protocol: Protocol::ChatCompletions,
    options: ProviderOptions::default(),
    models: spec_models(&[(
        "deepseek-chat",
        ModelCapabilities {
            temperature: true,
            reasoning: false,
            toolcall: true,
            ..Default::default()
        },
        128_000,
        8_192,
    )]),
});

/// Anthropic Messages API.
pub static ANTHROPIC: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "anthropic".into(),
    name: "Anthropic".into(),
    base_url: "https://api.anthropic.com".into(),
    api_key_env: Some("ANTHROPIC_API_KEY".into()),
    protocol: Protocol::AnthropicMessages,
    options: ProviderOptions {
        headers: std::collections::BTreeMap::from([(
            "anthropic-version".into(),
            crate::anthropic::ANTHROPIC_VERSION.into(),
        )]),
        ..Default::default()
    },
    models: spec_models(&[(
        "claude-sonnet-4-20250514",
        ModelCapabilities {
            temperature: true,
            reasoning: true,
            toolcall: true,
            ..Default::default()
        },
        200_000,
        8_192,
    )]),
});

/// Google Gemini API.
pub static GOOGLE: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "google".into(),
    name: "Google".into(),
    base_url: "https://generativelanguage.googleapis.com".into(),
    api_key_env: Some("GOOGLE_GENERATIVE_AI_API_KEY".into()),
    protocol: Protocol::GoogleGemini,
    options: ProviderOptions::default(),
    models: spec_models(&[(
        "gemini-2-5-flash",
        ModelCapabilities {
            temperature: true,
            reasoning: true,
            toolcall: true,
            ..Default::default()
        },
        1_048_576,
        8_192,
    )]),
});

/// Alibaba DashScope (OpenAI-compatible).
pub static ALIBABA: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "alibaba".into(),
    name: "Alibaba".into(),
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
    api_key_env: Some("ALIBABA_API_KEY".into()),
    protocol: Protocol::ChatCompletions,
    options: ProviderOptions::default(),
    models: spec_models(&[(
        "qwen-plus",
        ModelCapabilities {
            temperature: true,
            reasoning: false,
            toolcall: true,
            ..Default::default()
        },
        131_072,
        8_192,
    )]),
});

/// ZhipuAI GLM (OpenAI-compatible).
pub static ZHIPUAI: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "zhipuai".into(),
    name: "ZhipuAI".into(),
    base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
    api_key_env: Some("ZHIPUAI_API_KEY".into()),
    protocol: Protocol::ChatCompletions,
    options: ProviderOptions::default(),
    models: spec_models(&[(
        "glm-4-plus",
        ModelCapabilities {
            temperature: true,
            reasoning: false,
            toolcall: true,
            ..Default::default()
        },
        128_000,
        8_192,
    )]),
});

/// Moonshot / Kimi (OpenAI-compatible).
pub static MOONSHOT: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "moonshot".into(),
    name: "Moonshot".into(),
    base_url: "https://api.moonshot.cn/v1".into(),
    api_key_env: Some("MOONSHOT_API_KEY".into()),
    protocol: Protocol::ChatCompletions,
    options: ProviderOptions::default(),
    models: spec_models(&[(
        "kimi-k2.5",
        ModelCapabilities {
            temperature: true,
            reasoning: true,
            toolcall: true,
            ..Default::default()
        },
        262_144,
        16_384,
    )]),
});

/// Baidu Qianfan (OpenAI-compatible).
pub static BAIDU: LazyLock<ProviderSpec> = LazyLock::new(|| ProviderSpec {
    id: "baidu".into(),
    name: "Baidu".into(),
    base_url: "https://qianfan.baidubce.com/v2".into(),
    api_key_env: Some("BAIDU_API_KEY".into()),
    protocol: Protocol::ChatCompletions,
    options: ProviderOptions::default(),
    models: spec_models(&[(
        "ernie-4.0",
        ModelCapabilities {
            temperature: true,
            reasoning: false,
            toolcall: true,
            ..Default::default()
        },
        128_000,
        8_192,
    )]),
});

/// Look up a built-in spec by id; `None` for unknown ids.
#[must_use]
pub fn preset_spec(id: &str) -> Option<&'static ProviderSpec> {
    match id.to_ascii_lowercase().as_str() {
        "deepseek" => Some(&DEEPSEEK),
        "anthropic" => Some(&ANTHROPIC),
        "google" => Some(&GOOGLE),
        "alibaba" | "dashscope" => Some(&ALIBABA),
        "zhipuai" | "glm" => Some(&ZHIPUAI),
        "moonshot" | "kimi" => Some(&MOONSHOT),
        "baidu" | "ernie" => Some(&BAIDU),
        _ => None,
    }
}

/// Build a single-model catalog entry from a capability tuple.
fn spec_models(entries: &[(&str, ModelCapabilities, u64, u64)]) -> std::collections::BTreeMap<String, ModelInfo> {
    entries
        .iter()
        .map(|(id, caps, context, max_output)| {
            (
                (*id).to_string(),
                ModelInfo {
                    id: (*id).to_string(),
                    capabilities: caps.clone(),
                    limit: ModelLimit {
                        context: *context,
                        max_output: *max_output,
                    },
                    options: ModelOptions::default(),
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_resolve_to_routes() {
        for id in ["deepseek", "anthropic", "google", "alibaba", "zhipuai", "moonshot", "baidu"] {
            let spec = preset_spec(id).expect("preset exists");
            assert!(!spec.base_url.is_empty());
            assert!(!spec.models.is_empty(), "{id} has a model catalog");
            let route = crate::protocol::Route::from_spec(spec, None);
            assert_eq!(route.id, spec.id);
            assert!(!route.default_model.is_empty(), "{id} default model");
        }
    }

    #[test]
    fn from_spec_default_model_prefers_explicit() {
        let spec = &*DEEPSEEK;
        let route = crate::protocol::Route::from_spec(spec, Some("other-model"));
        assert_eq!(route.default_model, "other-model");
        assert_eq!(route.framing, crate::protocol::Framing::Sse);
    }

    #[test]
    fn unknown_preset_is_none() {
        assert!(preset_spec("nope").is_none());
    }
}
