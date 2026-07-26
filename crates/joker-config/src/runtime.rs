use crate::error::ConfigError;
use crate::provider_selection::ProviderSelection;
use crate::types::FileConfig;

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
            providers: std::collections::BTreeMap::new(),
            default_agent: None,
            agent: std::collections::BTreeMap::new(),
            mcp_servers: std::collections::BTreeMap::new(),
        }
    }
}
