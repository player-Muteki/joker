//! Model discovery and vendor detection.
//!
//! Utilities for fetching available models from a provider's API —
//! dispatching on the wire [`Protocol`] — identifying the vendor from a base
//! URL, and guessing the correct [`Protocol`](crate::protocol::Protocol),
//! [`Framing`](crate::protocol::Framing), and [`Auth`](crate::protocol::Auth)
//! for an unknown endpoint.

use serde::Deserialize;

use crate::protocol::{Auth, Framing, Protocol};
use crate::spec::ModelInfo;

/// OpenAI/Anthropic-compatible `/models` response shape.
#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// Google Gemini `models.list` response shape.
#[derive(Deserialize)]
struct GoogleModelsResponse {
    models: Option<Vec<GoogleModelEntry>>,
}

#[derive(Deserialize)]
struct GoogleModelEntry {
    #[serde(rename = "name")]
    name: String,
}

/// Fetch available models from a provider's API, dispatching on protocol.
///
/// - ChatCompletions tries several candidate URL patterns derived from
///   `base_url` (OpenAI-style `/v1/models`).
/// - AnthropicMessages queries `{base}/models` with the auth header.
/// - GoogleGemini queries `{base}/v1beta/models` with the auth header.
pub async fn discover_models(
    base_url: &str,
    auth: &Auth,
    protocol: &Protocol,
) -> Result<Vec<ModelInfo>, String> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    match protocol {
        Protocol::ChatCompletions => {
            let candidates = build_model_fetch_urls(base_url);
            for url in &candidates {
                let mut req = client.get(url);
                if let Some((header, value)) = auth.header_value() {
                    req = req.header(&header, &value);
                }
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        let body = resp
                            .text()
                            .await
                            .map_err(|e| format!("failed to read models response from {url}: {e}"))?;
                        return parse_models_response(&body)
                            .map_err(|e| format!("{e} (from {url})"));
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
        Protocol::AnthropicMessages => {
            let url = format!("{}/models", base_url.trim_end_matches('/'));
            let mut req = client.get(&url);
            if let Some((header, value)) = auth.header_value() {
                req = req.header(&header, &value);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| format!("model discovery request to {url} failed: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("model discovery failed ({url}): {}", resp.status()));
            }
            let body = resp
                .text()
                .await
                .map_err(|e| format!("failed to read models response from {url}: {e}"))?;
            parse_models_response(&body)
        }
        Protocol::GoogleGemini => {
            let url = format!("{}/v1beta/models", base_url.trim_end_matches('/'));
            let mut req = client.get(&url);
            if let Some((header, value)) = auth.header_value() {
                req = req.header(&header, &value);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| format!("model discovery request to {url} failed: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("model discovery failed ({url}): {}", resp.status()));
            }
            let body = resp
                .text()
                .await
                .map_err(|e| format!("failed to read models response from {url}: {e}"))?;
            parse_google_models_response(&body)
        }
    }
}

/// Parse an OpenAI/Anthropic-style models response (`{"data": [{"id": ...}]}`).
fn parse_models_response(json: &str) -> Result<Vec<ModelInfo>, String> {
    let body: ModelsResponse = serde_json::from_str(json)
        .map_err(|e| format!("failed to parse models response: {e}"))?;
    Ok(body
        .data
        .into_iter()
        .map(|entry| ModelInfo {
            id: entry.id,
            ..Default::default()
        })
        .collect())
}

/// Parse a Google models response (`{"models": [{"name": "models/..."}]}`).
fn parse_google_models_response(json: &str) -> Result<Vec<ModelInfo>, String> {
    let body: GoogleModelsResponse = serde_json::from_str(json)
        .map_err(|e| format!("failed to parse models response: {e}"))?;
    Ok(body
        .models
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.name.strip_prefix("models/").map(String::from))
        .map(|id| ModelInfo {
            id,
            ..Default::default()
        })
        .collect())
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
/// Returns `"unknown"` when no vendor is recognised. Known vendors can be
/// mapped to catalog data via [`preset_spec`](crate::catalog::preset_spec).
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
        Protocol::GoogleGemini => {
            Auth::api_key_from_env("x-goog-api-key", "GOOGLE_GENERATIVE_AI_API_KEY")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AuthScheme, CredentialSource};
    use crate::spec::ModelStatus;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve `body` as an HTTP response on a loopback listener; returns the base URL.
    fn fake_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    fn keyed_auth(header: &str) -> Auth {
        Auth {
            scheme: AuthScheme::ApiKey {
                header: header.into(),
            },
            credentials: CredentialSource::Value("sk-test".into()),
        }
    }

    #[tokio::test]
    async fn discovers_openai_compatible_models() {
        let base = fake_server(r#"{"data":[{"id":"deepseek-chat"},{"id":"deepseek-reasoner"}]}"#);
        let models = discover_models(&base, &Auth::none(), &Protocol::ChatCompletions)
            .await
            .expect("discover");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["deepseek-chat", "deepseek-reasoner"]);
    }

    #[tokio::test]
    async fn discovers_anthropic_models() {
        let base = fake_server(r#"{"data":[{"id":"claude-sonnet-4-20250514"}],"has_more":false}"#);
        let models =
            discover_models(&base, &keyed_auth("x-api-key"), &Protocol::AnthropicMessages)
                .await
                .expect("discover");
        assert_eq!(models[0].id, "claude-sonnet-4-20250514");
        assert_eq!(models[0].status, ModelStatus::Available);
    }

    #[tokio::test]
    async fn discovers_google_models() {
        let base = fake_server(
            r#"{"models":[{"name":"models/gemini-2-5-flash"},{"name":"models/gemini-2-5-pro"}]}"#,
        );
        let models = discover_models(&base, &keyed_auth("x-goog-api-key"), &Protocol::GoogleGemini)
            .await
            .expect("discover");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-2-5-flash", "gemini-2-5-pro"]);
    }

    #[test]
    fn parses_google_models_response() {
        let models = parse_google_models_response(
            r#"{"models":[{"name":"models/gemini-2-5-flash"},{"name":"publisherModels/x"}]}"#,
        )
        .expect("parse");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-2-5-flash"], "publisherModels are excluded");
    }

    #[test]
    fn parses_empty_google_models() {
        let models = parse_google_models_response(r#"{"models":[]}"#).expect("parse");
        assert!(models.is_empty());
    }

    #[test]
    fn rejects_malformed_models_response() {
        assert!(parse_models_response(r#"{"nope":true}"#).is_err());
    }

    #[tokio::test]
    async fn discover_models_fallback_no_network() {
        let result =
            discover_models("http://127.0.0.1:1", &Auth::none(), &Protocol::ChatCompletions)
                .await;
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
        assert_eq!(
            detect_vendor("https://generativelanguage.googleapis.com"),
            "google"
        );
        assert_eq!(detect_vendor("https://api.openai.com/v1"), "openai");
        assert_eq!(detect_vendor("https://api.groq.com/openai/v1"), "groq");
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
        assert_eq!(
            guess_protocol("https://api.anthropic.com"),
            Protocol::AnthropicMessages
        );
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
    fn routing_detects_unknown_vendor() {
        assert_eq!(detect_vendor("http://localhost:11434"), "unknown");
    }
}
