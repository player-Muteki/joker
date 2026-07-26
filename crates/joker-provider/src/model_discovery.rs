//! Model discovery and vendor detection.
//!
//! Utilities for fetching available model IDs from a provider's API,
//! identifying the vendor from a base URL, and guessing the correct
//! [`Protocol`](crate::protocol::Protocol), [`Framing`](crate::protocol::Framing),
//! and [`Auth`](crate::protocol::Auth) for a given endpoint.

use serde::Deserialize;

use crate::protocol::{Auth, Framing, Protocol};

/// Response from an OpenAI-compatible `/v1/models` endpoint.
#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// Fetch available model IDs from a provider's API.
///
/// Tries several candidate URL patterns derived from `base_url` and returns
/// the model IDs from the first successful response.
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

    Err(format!("no candidate model URLs worked for base_url: {base_url}"))
}

fn build_model_fetch_urls(base_url: &str) -> Vec<String> {
    let base = base_url.trim_end_matches('/');

    let mut candidates = Vec::new();
    candidates.push(format!("{base}/models"));

    if !base.ends_with("/v1") {
        candidates.push(format!("{base}/v1/models"));
    }

    for suffix in &["/api/anthropic", "/api/claudecode", "/api/coding"] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            let stripped = stripped.trim_end_matches('/');
            candidates.push(format!("{stripped}/models"));
            candidates.push(format!("{stripped}/v1/models"));
        }
    }

    candidates
}

/// Identify the vendor name from a base URL by matching known host patterns.
///
/// Returns `"unknown"` when no vendor is recognised.
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

/// Guess the wire [`Protocol`] for a base URL based on the vendor.
pub fn guess_protocol(base_url: &str) -> Protocol {
    match detect_vendor(base_url) {
        "anthropic" => Protocol::AnthropicMessages,
        "google" => Protocol::GoogleGemini,
        _ => Protocol::ChatCompletions,
    }
}

/// Guess the [`Framing`] for a given [`Protocol`].
pub fn guess_framing(protocol: &Protocol) -> Framing {
    match protocol {
        Protocol::GoogleGemini => Framing::StreamableHttp,
        _ => Framing::Sse,
    }
}

/// Guess a sensible default [`Auth`] for a given [`Protocol`].
///
/// The guessed auth reads the API key from a well-known environment variable.
pub fn guess_auth(protocol: &Protocol) -> Auth {
    match protocol {
        Protocol::ChatCompletions => Auth::bearer_from_env("OPENAI_COMPATIBLE_API_KEY"),
        Protocol::AnthropicMessages => Auth::api_key_from_env("x-api-key", "ANTHROPIC_API_KEY"),
        Protocol::GoogleGemini => Auth::api_key_from_env("x-goog-api-key", "GOOGLE_GENERATIVE_AI_API_KEY"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discover_models_fallback_no_network() {
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
        assert!(!urls.contains(&"https://api.openai.com/v1/v1/models".into()));
    }

    #[test]
    fn detect_vendor_identifies_known_providers() {
        assert_eq!(detect_vendor("https://api.deepseek.com"), "deepseek");
        assert_eq!(detect_vendor("https://api.anthropic.com/v1"), "anthropic");
        assert_eq!(detect_vendor("https://generativelanguage.googleapis.com"), "google");
        assert_eq!(detect_vendor("https://api.openai.com/v1"), "openai");
        assert_eq!(detect_vendor("https://api.groq.com/openai/v1"), "groq");
    }

    #[test]
    fn detect_vendor_unknown_default() {
        assert_eq!(detect_vendor("https://my-custom-llm.example.com"), "unknown");
    }

    #[test]
    fn guess_protocol_by_vendor() {
        assert_eq!(guess_protocol("https://api.anthropic.com"), Protocol::AnthropicMessages);
        assert_eq!(guess_protocol("https://generativelanguage.googleapis.com"), Protocol::GoogleGemini);
        assert_eq!(guess_protocol("https://api.deepseek.com"), Protocol::ChatCompletions);
    }

    #[test]
    fn routing_detects_unknown_vendor() {
        assert_eq!(detect_vendor("http://localhost:11434"), "unknown");
    }
}
