//! Configuration types for Joker's settings file.
//!
//! These types mirror the `joker.toml` schema and are deserialized directly
//! from TOML.  The [`RuntimeConfig`] in the `runtime` module is the resolved,
//! validated representation used at runtime.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Top-level configuration parsed from `joker.toml`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProviderConfig {
    /// Provider protocol kind (`"openai-compatible"`, `"anthropic"`, `"google"`).
    pub kind: Option<String>,
    /// Base URL for the provider's API endpoint.
    pub base_url: String,
    /// Default model name for this provider.
    pub model: String,
    /// Environment variable holding the API key for this provider.
    pub api_key_env: Option<String>,
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
