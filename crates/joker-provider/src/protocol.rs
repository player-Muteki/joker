use std::sync::Arc;

use joker::Model;

/// Wire-level protocol / API format used by a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protocol {
    ChatCompletions,
    AnthropicMessages,
    GoogleGemini,
}

/// Transport framing for streaming responses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Framing {
    Sse,
    StreamableHttp,
}

/// Authentication scheme for a provider endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthScheme {
    Bearer,
    ApiKey { header: String },
    None,
}

/// Where credential values come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    None,
    EnvVar(String),
    Value(String),
}

/// Authentication bundle for a Route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auth {
    pub scheme: AuthScheme,
    pub credentials: CredentialSource,
}

impl Auth {
    pub const fn none() -> Self {
        Self { scheme: AuthScheme::None, credentials: CredentialSource::None }
    }

    pub fn bearer_from_env(env_var: &str) -> Self {
        Self { scheme: AuthScheme::Bearer, credentials: CredentialSource::EnvVar(env_var.into()) }
    }

    pub fn api_key_from_env(header: &str, env_var: &str) -> Self {
        Self { scheme: AuthScheme::ApiKey { header: header.into() }, credentials: CredentialSource::EnvVar(env_var.into()) }
    }

    pub fn header_value(&self) -> Option<(String, String)> {
        match (&self.scheme, &self.credentials) {
            (AuthScheme::Bearer, CredentialSource::Value(v)) => {
                Some(("Authorization".into(), format!("Bearer {v}")))
            }
            (AuthScheme::Bearer, CredentialSource::EnvVar(name)) => {
                std::env::var(name).ok().map(|v| ("Authorization".into(), format!("Bearer {v}")))
            }
            (AuthScheme::ApiKey { header }, CredentialSource::Value(v)) => {
                Some((header.clone(), v.clone()))
            }
            (AuthScheme::ApiKey { header }, CredentialSource::EnvVar(name)) => {
                std::env::var(name).ok().map(|v| (header.clone(), v))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub id: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub auth: Auth,
    pub framing: Framing,
    pub default_model: String,
}

impl Route {
    pub fn build_model(&self) -> Result<Arc<dyn Model>, String> {
        self.do_build_model(&self.default_model)
    }

    pub fn build_model_for(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        self.do_build_model(model)
    }

    pub fn do_build_model(&self, model: &str) -> Result<Arc<dyn Model>, String> {
        let api_key = match &self.auth.credentials {
            CredentialSource::Value(v) => Some(v.clone()),
            CredentialSource::EnvVar(name) => std::env::var(name).ok(),
            CredentialSource::None => None,
        };

        match self.protocol {
            Protocol::ChatCompletions => {
                let config = crate::openai::OpenAiCompatibleConfig {
                    provider_name: self.id.clone(),
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key,
                    api_key_env: match &self.auth.credentials {
                        CredentialSource::EnvVar(n) => Some(n.clone()),
                        _ => None,
                    },
                    require_api_key: self.auth.credentials != CredentialSource::None,
                    extra_body: None,
                };
                Ok(Arc::new(
                    crate::openai::OpenAiCompatibleModel::new(config)
                        .map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::AnthropicMessages => {
                let key = api_key.ok_or_else(|| "Anthropic API key not configured".to_string())?;
                let config = crate::anthropic::AnthropicConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                };
                Ok(Arc::new(
                    crate::anthropic::AnthropicModel::new(config)
                        .map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
            Protocol::GoogleGemini => {
                let key = api_key.ok_or_else(|| "Google API key not configured".to_string())?;
                let config = crate::google::GoogleConfig {
                    base_url: self.base_url.clone(),
                    model: model.into(),
                    api_key: key,
                };
                Ok(Arc::new(
                    crate::google::GoogleModel::new(config).map_err(|e| e.to_string())?,
                ) as Arc<dyn Model>)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_builds_openai_model() {
        let route = Route {
            id: "test".into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://api.openai.com/v1".into(),
            auth: Auth { scheme: AuthScheme::Bearer, credentials: CredentialSource::EnvVar("OPENAI_API_KEY".into()) },
            framing: Framing::Sse,
            default_model: "gpt-4".into(),
        };
        let _ = route.build_model();
    }

    #[test]
    fn auth_bearer_from_env() {
        let auth = Auth::bearer_from_env("MY_KEY");
        assert_eq!(auth.scheme, AuthScheme::Bearer);
        assert_eq!(auth.credentials, CredentialSource::EnvVar("MY_KEY".into()));
    }

    #[test]
    fn auth_api_key_from_env() {
        let auth = Auth::api_key_from_env("x-api-key", "ANTHROPIC_API_KEY");
        assert_eq!(auth.scheme, AuthScheme::ApiKey { header: "x-api-key".into() });
        assert_eq!(auth.credentials, CredentialSource::EnvVar("ANTHROPIC_API_KEY".into()));
    }

    #[test]
    fn auth_none_has_no_header() {
        assert!(Auth::none().header_value().is_none());
    }
}
