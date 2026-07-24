#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use joker_provider::{
    ALIBABA, ANTHROPIC, BAIDU, DEEPSEEK, GOOGLE, MOONSHOT, ZHIPUAI, OpenAiCompatibleConfig,
};
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
            ProviderSelection::OpenAiCompatible(config) => config.model.clone(),
            ProviderSelection::Anthropic { model, .. } => model.clone(),
            ProviderSelection::Google { model, .. } => model.clone(),
        }
    }

    #[must_use]
    pub fn provider_label(&self) -> String {
        match &self.provider {
            ProviderSelection::Scripted { .. } => "scripted".into(),
            ProviderSelection::OpenAiCompatible(config) => {
                format!("{}/{}", config.provider_name, config.model)
            }
            ProviderSelection::Anthropic { model, .. } => format!("anthropic/{model}"),
            ProviderSelection::Google { model, .. } => format!("google/{model}"),
        }
    }

    pub fn switch_provider(&mut self, provider: &str) -> Result<(), ConfigError> {
        self.provider = ProviderSelection::preset(provider)?;
        Ok(())
    }

    pub fn switch_model(&mut self, model: impl Into<String>) -> Result<(), ConfigError> {
        match &mut self.provider {
            ProviderSelection::Scripted { model } => {
                *model = "scripted".into();
                Ok(())
            }
            ProviderSelection::OpenAiCompatible(config) => {
                let model = model.into();
                if model.trim().is_empty() {
                    return Err(ConfigError::InvalidValue("model cannot be empty".into()));
                }
                config.model = model;
                Ok(())
            }
            ProviderSelection::Anthropic { model: m, .. } => {
                let model = model.into();
                if model.trim().is_empty() {
                    return Err(ConfigError::InvalidValue("model cannot be empty".into()));
                }
                *m = model;
                Ok(())
            }
            ProviderSelection::Google { model: m, .. } => {
                let model = model.into();
                if model.trim().is_empty() {
                    return Err(ConfigError::InvalidValue("model cannot be empty".into()));
                }
                *m = model;
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn available_models(&self) -> Vec<String> {
        match &self.provider {
            ProviderSelection::Scripted { model } => vec![model.clone()],
            ProviderSelection::OpenAiCompatible(config) => known_model_list(&config.provider_name)
                .unwrap_or(&[&config.model])
                .iter()
                .map(|m| (*m).into())
                .collect(),
            ProviderSelection::Anthropic { .. } => {
                ANTHROPIC.models.iter().map(|m| (*m).into()).collect()
            }
            ProviderSelection::Google { .. } => {
                GOOGLE.models.iter().map(|m| (*m).into()).collect()
            }
        }
    }

    #[must_use]
    pub fn to_file_config(&self) -> FileConfig {
        FileConfig {
            provider: Some(match &self.provider {
                ProviderSelection::Scripted { .. } => "scripted".into(),
                ProviderSelection::OpenAiCompatible(config) => config.provider_name.clone(),
                ProviderSelection::Anthropic { .. } => "anthropic".into(),
                ProviderSelection::Google { .. } => "google".into(),
            }),
            model: Some(match &self.provider {
                ProviderSelection::Scripted { model } => model.clone(),
                ProviderSelection::OpenAiCompatible(config) => config.model.clone(),
                ProviderSelection::Anthropic { model, .. } => model.clone(),
                ProviderSelection::Google { model, .. } => model.clone(),
            }),
            base_url: match &self.provider {
                ProviderSelection::OpenAiCompatible(config)
                    if !is_known_preset(&config.provider_name) =>
                {
                    Some(config.base_url.clone())
                }
                _ => None,
            },
            api_key_env: match &self.provider {
                ProviderSelection::OpenAiCompatible(config)
                    if !is_known_preset(&config.provider_name) =>
                {
                    config.api_key_env.clone()
                }
                _ => None,
            },
            scripted_response: Some(self.scripted_response.clone()),
            demo_tool: Some(self.demo_tool),
            providers: BTreeMap::new(),
            default_agent: None,
            agent: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderSelection {
    Scripted { model: String },
    OpenAiCompatible(OpenAiCompatibleConfig),
    Anthropic {
        model: String,
        api_key: Option<String>,
    },
    Google {
        model: String,
        api_key: Option<String>,
    },
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
            "deepseek" => Ok(Self::OpenAiCompatible(OpenAiCompatibleConfig {
                provider_name: DEEPSEEK.id.into(),
                base_url: DEEPSEEK.base_url.into(),
                model: DEEPSEEK.default_model.into(),
                api_key: std::env::var(DEEPSEEK.api_key_env).ok(),
                api_key_env: Some(DEEPSEEK.api_key_env.into()),
                require_api_key: true,
                extra_body: None,
            })),
            "openai-compatible" | "custom" => Ok(Self::OpenAiCompatible(OpenAiCompatibleConfig {
                provider_name: "openai-compatible".into(),
                base_url: "http://localhost:8000/v1".into(),
                model: "model".into(),
                api_key: std::env::var("OPENAI_COMPATIBLE_API_KEY").ok(),
                api_key_env: Some("OPENAI_COMPATIBLE_API_KEY".into()),
                require_api_key: false,
                extra_body: None,
            })),
            "anthropic" => Ok(Self::Anthropic {
                model: ANTHROPIC.default_model.into(),
                api_key: std::env::var(ANTHROPIC.api_key_env).ok(),
            }),
            "google" => Ok(Self::Google {
                model: GOOGLE.default_model.into(),
                api_key: std::env::var(GOOGLE.api_key_env).ok(),
            }),
            "alibaba" | "dashscope" => Self::openai_preset(ALIBABA),
            "zhipuai" | "glm" => Self::openai_preset(ZHIPUAI),
            "moonshot" | "kimi" => Self::openai_preset(MOONSHOT),
            "baidu" | "ernie" => Self::openai_preset(BAIDU),
            other => Err(ConfigError::UnknownProvider(other.into())),
        }
    }

    fn openai_preset(desc: joker_provider::ProviderDescriptor) -> Result<Self, ConfigError> {
        Ok(Self::OpenAiCompatible(OpenAiCompatibleConfig {
            provider_name: desc.id.into(),
            base_url: desc.base_url.into(),
            model: desc.default_model.into(),
            api_key: std::env::var(desc.api_key_env).ok(),
            api_key_env: Some(desc.api_key_env.into()),
            require_api_key: true,
            extra_body: None,
        }))
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
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProviderConfig {
    pub kind: Option<String>,
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
}

/// Agent profile configuration, matching TOML `[agent.<name>]` sections.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct AgentProfileConfig {
    pub model: Option<String>,
    pub system: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolPermissionConfig>,
    #[serde(default)]
    pub permissions: PermissionRuleConfig,
}

/// Per-tool permission configuration in TOML.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ToolPermissionConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub permission: Option<String>,
}

/// Permission rule configuration for TOML.
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
    if let Some(base_url) = overrides.base_url.or(file.base_url.clone())
        && let ProviderSelection::OpenAiCompatible(provider) = &mut config.provider
    {
        provider.base_url = base_url;
    }
    if let Some(api_key_env) = overrides.api_key_env.or(file.api_key_env.clone())
        && let ProviderSelection::OpenAiCompatible(provider) = &mut config.provider
    {
        provider.api_key = std::env::var(&api_key_env).ok();
        provider.api_key_env = Some(api_key_env);
    }

    Ok(config)
}

fn provider_from_file(provider: &str, file: &FileConfig) -> Result<ProviderSelection, ConfigError> {
    if let Some(custom) = file.providers.get(provider) {
        let api_key_env = custom
            .api_key_env
            .clone()
            .unwrap_or_else(|| format!("{}_API_KEY", env_prefix(provider)));
        return Ok(ProviderSelection::OpenAiCompatible(
            OpenAiCompatibleConfig {
                provider_name: provider.into(),
                base_url: custom.base_url.clone(),
                model: custom.model.clone(),
                api_key: std::env::var(&api_key_env).ok(),
                api_key_env: Some(api_key_env),
                require_api_key: false,
                extra_body: None,
            },
        ));
    }
    ProviderSelection::preset(provider)
}

/// Check if a provider name is a built-in preset (not a custom endpoint).
fn is_known_preset(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "deepseek" | "alibaba" | "zhipuai" | "moonshot" | "baidu"
    )
}

/// Return the known model list for a well-known provider, or `None`.
fn known_model_list(provider_name: &str) -> Option<&'static [&'static str]> {
    match provider_name.to_lowercase().as_str() {
        "deepseek" => Some(DEEPSEEK.models),
        "alibaba" => Some(ALIBABA.models),
        "zhipuai" => Some(ZHIPUAI.models),
        "moonshot" => Some(MOONSHOT.models),
        "baidu" => Some(BAIDU.models),
        _ => None,
    }
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

        let ProviderSelection::OpenAiCompatible(provider) = config.provider else {
            panic!("expected openai-compatible provider");
        };
        assert_eq!(provider.base_url, "https://api.deepseek.com");
        assert_eq!(provider.model, "deepseek-v4-flash");
        assert_eq!(provider.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
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

        let ProviderSelection::OpenAiCompatible(provider) = config.provider else {
            panic!("expected openai-compatible provider");
        };
        assert_eq!(provider.model, "deepseek-v4-flash");
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
        assert!(raw.contains("model = \"deepseek-v4-flash\""));
    }
}
