//! Configuration types for Joker's settings file.
//!
//! These types mirror the `joker.toml` schema and are deserialized directly
//! from TOML.  The [`RuntimeConfig`] in the `runtime` module is the resolved,
//! validated representation used at runtime.

use std::collections::BTreeMap;

use joker_provider::{
    ModelCapabilities, ModelInfo, ModelLimit, ModelOptions, Protocol, ProviderOptions, ProviderSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level configuration parsed from `joker.toml`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct FileConfig {
    /// Name of the default LLM provider (e.g. `"deepseek"`, `"anthropic"`).
    pub provider: Option<String>,
    /// Default model name to use for the selected provider.
    pub model: Option<String>,
    /// Custom base URL override for the provider API.
    pub base_url: Option<String>,
    /// Environment variable name holding the API key.
    pub api_key_env: Option<String>,
    /// Template response used when the provider is `"scripted"`.
    pub scripted_response: Option<String>,
    /// Enable demo/debug tools.
    pub demo_tool: Option<bool>,
    /// Custom provider definitions keyed by name.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Name of the default agent profile.
    pub default_agent: Option<String>,
    /// Agent profile configurations keyed by name.
    #[serde(default)]
    pub agent: BTreeMap<String, AgentProfileConfig>,
    /// MCP server definitions keyed by name.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

impl FileConfig {
    /// Returns a list of all configured agent profile names.
    #[must_use]
    pub fn agent_names(&self) -> Vec<&str> {
        self.agent.keys().map(|s| s.as_str()).collect()
    }
}

/// Configuration for a custom LLM provider defined in `[providers]`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProviderConfig {
    /// Provider protocol kind (`"openai-compatible"`, `"anthropic"`, `"google"`).
    pub kind: Option<String>,
    /// Wire protocol; overrides `kind` when both are set.
    #[serde(default)]
    pub protocol: Option<String>,
    /// Base URL for the provider's API endpoint.
    pub base_url: String,
    /// Default model name for this provider.
    pub model: String,
    /// Environment variable holding the API key for this provider.
    pub api_key_env: Option<String>,
    /// Provider-level request options applied to every model call.
    #[serde(default)]
    pub options: ProviderOptionsConfig,
    /// Optional per-model catalog entries.
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
}

impl ProviderConfig {
    /// Convert this config into a data-driven [`ProviderSpec`] for `id`.
    ///
    /// The wire protocol comes from `protocol` (falling back to `kind`), and
    /// each `models` entry becomes a [`ModelInfo`] with sensible defaults
    /// (temperature and tool calls enabled unless disabled explicitly).
    #[must_use]
    pub fn to_spec(&self, id: &str) -> ProviderSpec {
        ProviderSpec {
            id: id.into(),
            name: id.into(),
            base_url: self.base_url.clone(),
            api_key_env: self.api_key_env.clone(),
            protocol: self.wire_protocol(),
            options: ProviderOptions {
                timeout: self.options.timeout,
                headers: self.options.headers.clone(),
                extra_body: self.options.extra_body.clone(),
            },
            models: self
                .models
                .iter()
                .map(|(key, config)| {
                    let model_id = config.id.clone().unwrap_or_else(|| key.clone());
                    (
                        model_id.clone(),
                        ModelInfo {
                            id: model_id,
                            capabilities: ModelCapabilities {
                                temperature: config.temperature.unwrap_or(true),
                                toolcall: config.toolcall.unwrap_or(true),
                                reasoning: config.reasoning.unwrap_or(false),
                                ..Default::default()
                            },
                            limit: ModelLimit {
                                context: config.context.unwrap_or(0),
                                max_output: config.max_output.unwrap_or(0),
                            },
                            options: ModelOptions {
                                temperature: config.default_temperature,
                                max_tokens: config.max_tokens,
                                extra_body: config.extra_body.clone(),
                            },
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        }
    }

    /// Resolve the wire protocol from `protocol`, falling back to `kind`.
    #[must_use]
    fn wire_protocol(&self) -> Protocol {
        match self
            .protocol
            .as_deref()
            .or(self.kind.as_deref())
            .unwrap_or("openai-compatible")
        {
            "anthropic" => Protocol::AnthropicMessages,
            "google" => Protocol::GoogleGemini,
            _ => Protocol::ChatCompletions,
        }
    }
}

/// Provider-level request options in `[providers.<name>.options]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ProviderOptionsConfig {
    /// Overall request timeout in seconds.
    pub timeout: Option<u64>,
    /// Static HTTP headers merged into every request.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Extra JSON body fields merged into every request body.
    pub extra_body: Option<Value>,
}

/// Per-model entry in a configured provider's catalog.
///
/// The map key is the model identifier unless `id` overrides it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelConfig {
    /// Model identifier sent in the request body; defaults to the map key.
    pub id: Option<String>,
    /// Whether the model accepts a `temperature` parameter. Defaults to `true`.
    pub temperature: Option<bool>,
    /// Default `temperature` sent when the provider supports it.
    pub default_temperature: Option<f64>,
    /// Whether the model supports tool calls. Defaults to `true`.
    pub toolcall: Option<bool>,
    /// Whether the model produces reasoning content.
    pub reasoning: Option<bool>,
    /// Context window size in tokens.
    pub context: Option<u64>,
    /// Maximum output tokens.
    pub max_output: Option<u64>,
    /// Default `max_tokens` sent in the request body.
    pub max_tokens: Option<u64>,
    /// Extra JSON body fields merged for this model.
    pub extra_body: Option<Value>,
}

/// Configuration for a single agent profile in `[agent]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AgentProfileConfig {
    /// Override model for this agent.
    pub model: Option<String>,
    /// System prompt for this agent.
    pub system: Option<String>,
    /// Per-tool permission overrides keyed by tool name.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolPermissionConfig>,
    /// Fallback permission rules for tools not individually configured.
    #[serde(default)]
    pub permissions: PermissionRuleConfig,
}

/// Permission settings for a single tool within an agent profile.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ToolPermissionConfig {
    /// Whether the tool is enabled for this agent.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Permission level (`"ask"`, `"auto-accept"`, `"disabled"`).
    #[serde(default)]
    pub permission: Option<String>,
}

/// Global permission rules for an agent profile.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct PermissionRuleConfig {
    /// Tool name patterns that are always denied.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Tool name patterns that require user confirmation.
    #[serde(default)]
    pub ask: Vec<String>,
    /// Tool name patterns that are auto-approved.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Whether to persist approvals across the session.
    #[serde(default)]
    pub remember_session_approvals: Option<bool>,
}

/// Configuration for an MCP server process in `[mcp_servers]`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct McpServerConfig {
    /// Path or name of the server executable.
    pub command: Option<String>,
    /// Command-line arguments passed to the server process.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Command-line overrides that take precedence over the file-based config.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigOverrides {
    /// Override the default provider name.
    pub provider: Option<String>,
    /// Override the default model name.
    pub model: Option<String>,
    /// Override the provider's base URL.
    pub base_url: Option<String>,
    /// Override the API key environment variable name.
    pub api_key_env: Option<String>,
    /// Override the scripted response template.
    pub scripted_response: Option<String>,
    /// Enable or disable demo tools.
    pub demo_tool: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_provider_config_round_trips_through_toml() {
        let file = FileConfig {
            provider: Some("myllm".into()),
            providers: BTreeMap::from([(
                "myllm".into(),
                ProviderConfig {
                    kind: Some("openai-compatible".into()),
                    protocol: None,
                    base_url: "https://llm.example.com/v1".into(),
                    model: "my-chat".into(),
                    api_key_env: Some("MYLLM_API_KEY".into()),
                    options: ProviderOptionsConfig {
                        timeout: Some(30),
                        headers: BTreeMap::from([("x-custom".into(), "1".into())]),
                        extra_body: Some(serde_json::json!({"top_p": 0.9})),
                    },
                    models: BTreeMap::from([(
                        "my-chat".into(),
                        ModelConfig {
                            id: None,
                            temperature: Some(true),
                            default_temperature: Some(0.7),
                            toolcall: Some(true),
                            reasoning: Some(false),
                            context: Some(128_000),
                            max_output: Some(8_192),
                            max_tokens: Some(4_096),
                            extra_body: None,
                        },
                    )]),
                },
            )]),
            ..Default::default()
        };

        let raw = toml::to_string_pretty(&file).expect("serialize");
        let decoded: FileConfig = toml::from_str(&raw).expect("deserialize");
        assert_eq!(decoded, file);
    }

    #[test]
    fn to_spec_maps_config_fields() {
        let config = ProviderConfig {
            kind: Some("anthropic".into()),
            protocol: None,
            base_url: "https://api.anthropic.com".into(),
            model: "claude-x".into(),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
            options: ProviderOptionsConfig::default(),
            models: BTreeMap::from([(
                "claude-x".into(),
                ModelConfig {
                    reasoning: Some(true),
                    context: Some(200_000),
                    ..Default::default()
                },
            )]),
        };

        let spec = config.to_spec("custom");
        assert_eq!(spec.id, "custom");
        assert_eq!(spec.protocol, Protocol::AnthropicMessages);
        assert_eq!(spec.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));

        let model = spec.models.get("claude-x").expect("model entry");
        assert!(model.capabilities.toolcall, "toolcall defaults to true");
        assert!(
            model.capabilities.temperature,
            "temperature defaults to true"
        );
        assert!(model.capabilities.reasoning);
        assert_eq!(model.limit.context, 200_000);
    }

    #[test]
    fn protocol_field_overrides_kind() {
        let config = ProviderConfig {
            kind: Some("openai-compatible".into()),
            protocol: Some("google".into()),
            base_url: "https://generativelanguage.googleapis.com".into(),
            model: "gemini-x".into(),
            api_key_env: None,
            options: ProviderOptionsConfig::default(),
            models: BTreeMap::new(),
        };
        assert_eq!(config.to_spec("g").protocol, Protocol::GoogleGemini);
    }
}
