//! Configuration resolution — merges file config with CLI overrides into a [`RuntimeConfig`].

use joker_provider::Route;
use tracing::info;

use crate::RuntimeConfig;
use crate::error::ConfigError;
use crate::provider_selection::ProviderSelection;
use crate::types::{ConfigOverrides, FileConfig};

/// Merge a [`FileConfig`] with CLI [`ConfigOverrides`] into a resolved [`RuntimeConfig`].
pub fn resolve_config(
    file: FileConfig,
    overrides: ConfigOverrides,
) -> Result<RuntimeConfig, ConfigError> {
    let provider_count = file.providers.len();
    info!(target: "config", provider_count, "resolving configuration");
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
        && let ProviderSelection::Route(route) = &mut config.provider
    {
        route.base_url = base_url;
    }
    if let Some(api_key_env) = overrides.api_key_env.or(file.api_key_env.clone())
        && let ProviderSelection::Route(route) = &mut config.provider
    {
        let key = std::env::var(&api_key_env).ok();
        route.auth.credentials = match key {
            Some(v) => joker_provider::CredentialSource::Value(v),
            None => joker_provider::CredentialSource::EnvVar(api_key_env),
        };
    }

    // Preserve agent profile configs and MCP server configs for restart
    // (OUTLINE.md 10.3: modified RuntimeConfig to retain resolved configs).
    config.agent_configs = file.agent.clone();
    config.mcp_server_configs = file.mcp_servers.clone();

    Ok(config)
}

fn provider_from_file(provider: &str, file: &FileConfig) -> Result<ProviderSelection, ConfigError> {
    if let Some(custom) = file.providers.get(provider) {
        let mut spec = custom.to_spec(provider);
        if spec.api_key_env.is_none() {
            spec.api_key_env = Some(format!("{}_API_KEY", env_prefix(provider)));
        }
        return Ok(ProviderSelection::Route(Route::from_spec(
            &spec,
            Some(&custom.model),
        )));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelConfig, ProviderConfig, ProviderOptionsConfig};
    use joker_provider::{CredentialSource, Framing, Protocol};

    #[test]
    fn custom_provider_resolves_to_route() {
        let file = FileConfig {
            provider: Some("myllm".into()),
            providers: std::collections::BTreeMap::from([(
                "myllm".into(),
                ProviderConfig {
                    kind: Some("openai-compatible".into()),
                    protocol: None,
                    base_url: "https://llm.example.com/v1".into(),
                    model: "my-chat".into(),
                    api_key_env: None,
                    options: ProviderOptionsConfig {
                        timeout: Some(15),
                        headers: std::collections::BTreeMap::new(),
                        extra_body: None,
                    },
                    models: std::collections::BTreeMap::from([(
                        "my-chat".into(),
                        ModelConfig {
                            context: Some(64_000),
                            ..Default::default()
                        },
                    )]),
                },
            )]),
            ..Default::default()
        };

        let config = resolve_config(file, ConfigOverrides::default()).expect("resolve");
        let ProviderSelection::Route(route) = &config.provider else {
            panic!("expected route");
        };
        assert_eq!(route.id, "myllm");
        assert_eq!(route.protocol, Protocol::ChatCompletions);
        assert_eq!(route.base_url, "https://llm.example.com/v1");
        assert_eq!(route.default_model, "my-chat");
        assert_eq!(route.framing, Framing::Sse);
        assert_eq!(
            route.auth.credentials,
            CredentialSource::EnvVar("MYLLM_API_KEY".into()),
            "missing api_key_env falls back to {{PROVIDER}}_API_KEY"
        );
    }

    #[test]
    fn anthropic_custom_provider_uses_key_header() {
        let file = FileConfig {
            provider: Some("claude".into()),
            providers: std::collections::BTreeMap::from([(
                "claude".into(),
                ProviderConfig {
                    kind: Some("anthropic".into()),
                    protocol: None,
                    base_url: "https://api.anthropic.com".into(),
                    model: "claude-sonnet-x".into(),
                    api_key_env: Some("ANTHROPIC_API_KEY".into()),
                    options: ProviderOptionsConfig::default(),
                    models: std::collections::BTreeMap::new(),
                },
            )]),
            ..Default::default()
        };

        let config = resolve_config(file, ConfigOverrides::default()).expect("resolve");
        let ProviderSelection::Route(route) = &config.provider else {
            panic!("expected route");
        };
        assert_eq!(
            route.auth.scheme,
            joker_provider::AuthScheme::ApiKey {
                header: "x-api-key".into()
            }
        );
        assert_eq!(route.framing, Framing::Sse);
    }

    #[test]
    fn presets_default_to_catalog_models() {
        let file = FileConfig {
            provider: Some("deepseek".into()),
            ..Default::default()
        };
        let config = resolve_config(file, ConfigOverrides::default()).expect("resolve");
        assert_eq!(config.current_model(), "deepseek-chat");

        let file = FileConfig {
            provider: Some("kimi".into()),
            ..Default::default()
        };
        let config = resolve_config(file, ConfigOverrides::default()).expect("resolve");
        assert_eq!(config.current_model(), "kimi-k2.5");
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let file = FileConfig {
            provider: Some("nope".into()),
            ..Default::default()
        };
        let error = resolve_config(file, ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("nope"));
    }
}
