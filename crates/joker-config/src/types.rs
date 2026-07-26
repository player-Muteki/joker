use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct FileConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub scripted_response: Option<String>,
    pub demo_tool: Option<bool>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    pub default_agent: Option<String>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentProfileConfig>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

impl FileConfig {
    pub fn agent_names(&self) -> Vec<&str> {
        self.agent.keys().map(|s| s.as_str()).collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProviderConfig {
    pub kind: Option<String>,
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AgentProfileConfig {
    pub model: Option<String>,
    pub system: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolPermissionConfig>,
    #[serde(default)]
    pub permissions: PermissionRuleConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ToolPermissionConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub permission: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct PermissionRuleConfig {
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub remember_session_approvals: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct McpServerConfig {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub scripted_response: Option<String>,
    pub demo_tool: Option<bool>,
}
