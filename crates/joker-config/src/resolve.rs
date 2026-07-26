//! Configuration resolution — merges file config with CLI overrides into a [`RuntimeConfig`].

use joker_provider::Route;

use crate::error::ConfigError;
use crate::provider_selection::ProviderSelection;
use crate::types::{ConfigOverrides, FileConfig};
use crate::RuntimeConfig;

/// Merge a [`FileConfig`] with CLI [`ConfigOverrides`] into a resolved [`RuntimeConfig`].
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
        && let ProviderSelection::Route(route) = &mut config.provider {
            route.base_url = base_url;
        }
    if let Some(api_key_env) = overrides.api_key_env.or(file.api_key_env.clone())
        && let ProviderSelection::Route(route) = &mut config.provider {
            let key = std::env::var(&api_key_env).ok();
            route.auth.credentials = match key {
                Some(v) => joker_provider::CredentialSource::Value(v),
                None => joker_provider::CredentialSource::EnvVar(api_key_env),
            };
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
