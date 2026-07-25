#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use joker_provider::{Route, ALIBABA, ANTHROPIC, BAIDU, DEEPSEEK, GOOGLE, MOONSHOT, ZHIPUAI};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub provider: ProviderSelection,
    pub scripted_response: String,
    pub demo_tool: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            provider: ProviderSelection::scripted(),
            scripted_response: "Hello from Joker TUI.".into(),
            demo_tool: false,
        }
    }
}

impl RuntimeConfig {
    #[must_use]
    pub fn current_model(&self) -> String {
        match &self.provider {
            ProviderSelection::Scripted { model } => model.clone(),
            ProviderSelection::Route(route) => {
                if route.default_model.is_empty() {
                    String::new()
                } else {
                    route.default_model.clone()
                }
            }
        }
    }

    #[must_use]
    pub fn provider_label(&self) -> String {
        match &self.provider {
            ProviderSelection::Scripted { .. } => "scripted".into(),
            ProviderSelection::Route(route) => {
                let vendor = joker_provider::detect_vendor(&route.base_url);
                if route.default_model.is_empty() {
                    vendor.into()
                } else {
                    format!("{vendor}/{}", route.default_model)
                }
            }
        }
    }

    pub fn switch_provider(&mut self, provider: &str) -> Result<(), ConfigError> {
        self.provider = ProviderSelection::preset(provider)?;
        Ok(())
    }

    pub fn needs_api_key(&self) -> Option<String> {
        match &self.provider {
            ProviderSelection::Scripted { .. } => None,
            ProviderSelection::Route(route) => {
                match &route.auth.credentials {
                    joker_provider::CredentialSource::EnvVar(name) => {
                        if std::env::var(name).is_err() {
                            Some(name.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    pub fn switch_model(&mut self, model: impl Into<String>) -> Result<(), ConfigError> {
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ConfigError::InvalidValue("model cannot be empty".into()));
        }
        match &mut self.provider {
            ProviderSelection::Scripted { .. } => Ok(()),
            ProviderSelection::Route(route) => {
                route.default_model = model;
                Ok(())
            }
        }
    }

    /// Available models — this list is only populated if model discovery
    /// has been run. For dynamic discovery, use `joker_provider::discover_models()`.
    #[must_use]
    pub fn available_models(&self) -> Vec<String> {
        match &self.provider {
            ProviderSelection::Scripted { model } => vec![model.clone()],
            ProviderSelection::Route(route) => {
                if route.default_model.is_empty() {
                    vec![]
                } else {
                    vec![route.default_model.clone()]
                }
            }
        }
    }

    #[must_use]
    pub fn to_file_config(&self) -> FileConfig {
        FileConfig {
            provider: Some(match &self.provider {
                ProviderSelection::Scripted { .. } => "scripted".into(),
                ProviderSelection::Route(route) => {
                    let vendor = joker_provider::detect_vendor(&route.base_url);
                    if vendor == "unknown" {
                        route.id.clone()
                    } else {
                        vendor.into()
                    }
                }
            }),
            model: Some(self.current_model()),
            base_url: match &self.provider {
                ProviderSelection::Route(route)
                    if joker_provider::detect_vendor(&route.base_url) == "unknown" =>
                {
                    Some(route.base_url.clone())
                }
                _ => None,
            },
            api_key_env: match &self.provider {
                ProviderSelection::Route(route) => match &route.auth.credentials {
                    joker_provider::CredentialSource::EnvVar(name) => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            },
            scripted_response: Some(self.scripted_response.clone()),
            demo_tool: Some(self.demo_tool),
            providers: BTreeMap::new(),
            default_agent: None,
            agent: BTreeMap::new(),
            mcp_servers: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderSelection {
    Scripted { model: String },
    Route(Route),
}

impl ProviderSelection {
    #[must_use]
    pub fn scripted() -> Self {
        Self::Scripted {
            model: "scripted".into(),
        }
    }

    pub fn preset(provider: &str) -> Result<Self, ConfigError> {
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

#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn project_default() -> Self {
        Self::new("joker.toml")
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self, overrides: ConfigOverrides) -> Result<RuntimeConfig, ConfigError> {
        let file = if self.path.exists() {
            let raw = fs::read_to_string(&self.path)?;
            toml::from_str::<FileConfig>(&raw)?
        } else {
            FileConfig::default()
        };
        resolve_config(file, overrides)
    }

    pub fn save(&self, config: &RuntimeConfig) -> Result<(), ConfigError> {
        let file = config.to_file_config();
        let raw = toml::to_string_pretty(&file)?;
        fs::write(&self.path, raw)?;
        Ok(())
    }
}

pub fn resolve_config(
    file: FileConfig,
    overrides: ConfigOverrides,
) -> Result<RuntimeConfig, ConfigError> {
    let mut config = RuntimeConfig::default();
    config.scripted_response = overrides
        .scripted_response
        .or(file.scripted_response.clone())
        .unwrap_or(config.scripted_response);
    config.demo_tool = overrides.demo_tool.or(file.demo_tool).unwrap_or(false);

    let provider = overrides
        .provider
        .or(file.provider.clone())
        .unwrap_or_else(|| "scripted".into());
    config.provider = provider_from_file(&provider, &file)?;

    if let Some(model) = overrides.model.or(file.model.clone()) {
        config.switch_model(model)?;
    }
    if let Some(base_url) = overrides.base_url.or(file.base_url.clone()) {
        if let ProviderSelection::Route(route) = &mut config.provider {
            route.base_url = base_url;
        }
    }
    if let Some(api_key_env) = overrides.api_key_env.or(file.api_key_env.clone()) {
        if let ProviderSelection::Route(route) = &mut config.provider {
            let key = std::env::var(&api_key_env).ok();
            route.auth.credentials = match key {
                Some(v) => joker_provider::CredentialSource::Value(v),
                None => joker_provider::CredentialSource::EnvVar(api_key_env),
            };
        }
    }

    Ok(config)
}

fn provider_from_file(provider: &str, file: &FileConfig) -> Result<ProviderSelection, ConfigError> {
    if let Some(custom) = file.providers.get(provider) {
        let api_key_env = custom
            .api_key_env
            .clone()
            .unwrap_or_else(|| format!("{}_API_KEY", env_prefix(provider)));

        let kind = custom.kind.as_deref().unwrap_or("openai-compatible");
        let protocol = match kind {
            "anthropic" => joker_provider::Protocol::AnthropicMessages,
            "google" => joker_provider::Protocol::GoogleGemini,
            _ => joker_provider::Protocol::ChatCompletions,
        };
        let framing = joker_provider::guess_framing(&protocol);
        let auth = match protocol {
            joker_provider::Protocol::AnthropicMessages => {
                joker_provider::Auth::api_key_from_env("x-api-key", &api_key_env)
            }
            joker_provider::Protocol::GoogleGemini => {
                joker_provider::Auth::api_key_from_env("x-goog-api-key", &api_key_env)
            }
            joker_provider::Protocol::ChatCompletions => {
                joker_provider::Auth::bearer_from_env(&api_key_env)
            }
        };

        return Ok(ProviderSelection::Route(Route {
            id: provider.into(),
            protocol,
            base_url: custom.base_url.clone(),
            auth,
            framing,
            default_model: custom.model.clone(),
        }));
    }
    ProviderSelection::preset(provider)
}

fn env_prefix(provider: &str) -> String {
    let prefix = provider
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    prefix.trim_matches('_').to_string()
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    TomlDeserialize(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_deepseek_preset() {
        let config = resolve_config(
            FileConfig {
                provider: Some("deepseek".into()),
                ..FileConfig::default()
            },
            ConfigOverrides::default(),
        )
        .unwrap();

        let ProviderSelection::Route(route) = config.provider else {
            panic!("expected Route provider");
        };
        assert_eq!(route.base_url, "https://api.deepseek.com");
        assert_eq!(route.default_model, "deepseek-chat");
    }

    #[test]
    fn cli_overrides_file_model() {
        let config = resolve_config(
            FileConfig {
                provider: Some("deepseek".into()),
                model: Some("deepseek-v4-pro".into()),
                ..FileConfig::default()
            },
            ConfigOverrides {
                model: Some("deepseek-v4-flash".into()),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();

        let ProviderSelection::Route(route) = config.provider else {
            panic!("expected Route provider");
        };
        assert_eq!(route.default_model, "deepseek-v4-flash");
    }

    #[test]
    fn saves_project_config() {
        let path =
            std::env::temp_dir().join(format!("joker-config-test-{}.toml", std::process::id()));
        let store = ConfigStore::new(&path);
        let config = RuntimeConfig {
            provider: ProviderSelection::preset("deepseek").unwrap(),
            scripted_response: "demo".into(),
            demo_tool: true,
        };

        store.save(&config).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(raw.contains("provider = \"deepseek\""));
        assert!(raw.contains("model = \"deepseek-chat\""));
    }
}
