use joker_config::{ConfigStore, ProviderSelection};

use crate::app::App;

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

pub(super) struct ConfigCommand;

impl SlashCommand for ConfigCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "config",
            usage: "/config [show|set <key> <value>|save]",
            description: "View, edit, or save configuration.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        config_store: &ConfigStore,
    ) -> CommandResult {
        let Some(args) = args else {
            return config_show(app, config_store);
        };
        let mut parts = args.splitn(3, char::is_whitespace);
        match parts.next().unwrap_or_default() {
            "show" => config_show(app, config_store),
            "save" => match config_store.save(&app.runtime_config) {
                Ok(()) => CommandResult::message(format!(
                    "Saved config to {}",
                    config_store.path().display()
                )),
                Err(error) => CommandResult::error(error.to_string()),
            },
            "set" => {
                if app.running {
                    return CommandResult::error(
                        "Cannot edit config while a run is active. Use /cancel first.",
                    );
                }
                let Some(key) = parts.next() else {
                    return CommandResult::error("Usage: /config set <key> <value>");
                };
                let Some(value) = parts.next() else {
                    return CommandResult::error("Usage: /config set <key> <value>");
                };
                config_set(app, key, value)
            }
            other => CommandResult::error(format!("Unknown /config command: {other}")),
        }
    }

    fn complete(&self, args_partial: &str) -> Vec<String> {
        let partial = args_partial.to_ascii_lowercase();
        ["show", "set", "save"]
            .iter()
            .filter(|s| s.starts_with(&partial))
            .map(|s| s.to_string())
            .collect()
    }
}

fn config_show(app: &App, config_store: &ConfigStore) -> CommandResult {
    CommandResult::message(format!(
        "provider = {}\nmodels = [{}]\nscripted_response = {:?}\ndemo_tool = {}\nconfig_path = {}",
        app.runtime_config.provider_label(),
        app.runtime_config.available_models().join(", "),
        app.runtime_config.scripted_response,
        app.runtime_config.demo_tool,
        config_store.path().display()
    ))
}

fn config_set(app: &mut App, key: &str, value: &str) -> CommandResult {
    let result = match key {
        "provider" => app.runtime_config.switch_provider(value),
        "model" => app.runtime_config.switch_model(value),
        "scripted_response" => {
            app.runtime_config.scripted_response = value.into();
            Ok(())
        }
        "demo_tool" => {
            app.runtime_config.demo_tool = matches!(value, "true" | "on" | "yes" | "1");
            Ok(())
        }
        "base_url" => match &mut app.runtime_config.provider {
            ProviderSelection::OpenAiCompatible(provider) => {
                provider.base_url = value.into();
                Ok(())
            }
            ProviderSelection::Scripted { .. }
            | ProviderSelection::Anthropic { .. }
            | ProviderSelection::Google { .. } => Err(joker_config::ConfigError::InvalidValue(
                "base_url only applies to OpenAI-compatible providers".into(),
            )),
        },
        "api_key_env" => match &mut app.runtime_config.provider {
            ProviderSelection::OpenAiCompatible(provider) => {
                provider.api_key = std::env::var(value).ok();
                provider.api_key_env = Some(value.into());
                Ok(())
            }
            ProviderSelection::Scripted { .. }
            | ProviderSelection::Anthropic { .. }
            | ProviderSelection::Google { .. } => Err(joker_config::ConfigError::InvalidValue(
                "api_key_env only applies to OpenAI-compatible providers".into(),
            )),
        },
        _ => Err(joker_config::ConfigError::InvalidValue(format!(
            "unknown config key: {key}"
        ))),
    };
    match result {
        Ok(()) => CommandResult::with_message_and_action(
            format!("Set {key} = {value}"),
            CommandAction::ConfigChanged,
        ),
        Err(error) => CommandResult::error(error.to_string()),
    }
}
