use joker_config::{ConfigStore, ProviderSelection};

use crate::app::{App, TranscriptItem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub message: Option<String>,
    pub action: Option<CommandAction>,
    pub is_error: bool,
}

impl CommandResult {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: false,
        }
    }

    fn action(action: CommandAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }

    fn with_message_and_action(message: impl Into<String>, action: CommandAction) -> Self {
        Self {
            message: Some(message.into()),
            action: Some(action),
            is_error: false,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandAction {
    Cancel,
    Clear,
    ConfigChanged,
    Quit,
}

pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        usage: "/help [command]",
        description: "Show slash commands.",
    },
    CommandInfo {
        name: "clear",
        usage: "/clear",
        description: "Clear the transcript.",
    },
    CommandInfo {
        name: "quit",
        usage: "/quit",
        description: "Exit Joker.",
    },
    CommandInfo {
        name: "cancel",
        usage: "/cancel",
        description: "Cancel the current run.",
    },
    CommandInfo {
        name: "status",
        usage: "/status",
        description: "Show current runtime status.",
    },
    CommandInfo {
        name: "provider",
        usage: "/provider [scripted|deepseek|openai-compatible]",
        description: "View or switch the provider.",
    },
    CommandInfo {
        name: "model",
        usage: "/model [name]",
        description: "View or switch the model.",
    },
    CommandInfo {
        name: "models",
        usage: "/models",
        description: "List models for the current provider.",
    },
    CommandInfo {
        name: "config",
        usage: "/config [show|set <key> <value>|save]",
        description: "View, edit, or save configuration.",
    },
    CommandInfo {
        name: "tools",
        usage: "/tools",
        description: "List enabled tools.",
    },
];

pub fn execute(input: &str, app: &mut App, config_store: &ConfigStore) -> CommandResult {
    let trimmed = input.trim().trim_start_matches('/');
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().to_ascii_lowercase();
    let args = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let result = match name.as_str() {
        "" | "help" => help(args),
        "clear" => CommandResult::action(CommandAction::Clear),
        "quit" | "exit" => CommandResult::action(CommandAction::Quit),
        "cancel" => CommandResult::action(CommandAction::Cancel),
        "status" => status(app, config_store),
        "provider" => provider(app, args),
        "model" => model(app, args),
        "models" => models(app),
        "config" => config(app, config_store, args),
        "tools" => tools(app),
        other => unknown(other),
    };

    if let Some(message) = &result.message {
        if result.is_error {
            app.transcript.push(TranscriptItem::Error(message.clone()));
        } else {
            app.transcript.push(TranscriptItem::Status(message.clone()));
        }
    }

    result
}

pub fn suggestions(input: &str) -> Vec<&'static CommandInfo> {
    let query = input
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| query.is_empty() || command.name.starts_with(&query))
        .take(6)
        .collect()
}

fn help(args: Option<&str>) -> CommandResult {
    if let Some(name) = args {
        if let Some(command) = COMMANDS.iter().find(|command| command.name == name) {
            return CommandResult::message(format!("{}\n{}", command.usage, command.description));
        }
        return CommandResult::error(format!("Unknown command: /{name}"));
    }

    CommandResult::message(
        COMMANDS
            .iter()
            .map(|command| format!("{:<36} {}", command.usage, command.description))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn status(app: &App, config_store: &ConfigStore) -> CommandResult {
    CommandResult::message(format!(
        "Provider: {}\nRunning: {}\nConfig: {}",
        app.runtime_config.provider_label(),
        app.running,
        config_store.path().display()
    ))
}

fn provider(app: &mut App, args: Option<&str>) -> CommandResult {
    if app.running {
        return CommandResult::error(
            "Cannot switch provider while a run is active. Use /cancel first.",
        );
    }
    let Some(provider) = args else {
        return CommandResult::message(format!(
            "Current provider: {}\nAvailable: scripted, deepseek, openai-compatible",
            app.runtime_config.provider_label()
        ));
    };
    match app.runtime_config.switch_provider(provider) {
        Ok(()) => CommandResult::with_message_and_action(
            format!(
                "Switched provider to {}",
                app.runtime_config.provider_label()
            ),
            CommandAction::ConfigChanged,
        ),
        Err(error) => CommandResult::error(error.to_string()),
    }
}

fn model(app: &mut App, args: Option<&str>) -> CommandResult {
    if app.running {
        return CommandResult::error(
            "Cannot switch model while a run is active. Use /cancel first.",
        );
    }
    let Some(model) = args else {
        return CommandResult::message(format!(
            "Current model: {}\nAvailable: {}",
            app.runtime_config.provider_label(),
            app.runtime_config.available_models().join(", ")
        ));
    };
    match app.runtime_config.switch_model(model) {
        Ok(()) => CommandResult::with_message_and_action(
            format!("Switched model to {}", app.runtime_config.provider_label()),
            CommandAction::ConfigChanged,
        ),
        Err(error) => CommandResult::error(error.to_string()),
    }
}

fn models(app: &App) -> CommandResult {
    CommandResult::message(app.runtime_config.available_models().join("\n"))
}

fn config(app: &mut App, config_store: &ConfigStore, args: Option<&str>) -> CommandResult {
    let Some(args) = args else {
        return config_show(app, config_store);
    };
    let mut parts = args.splitn(3, char::is_whitespace);
    match parts.next().unwrap_or_default() {
        "show" => config_show(app, config_store),
        "save" => match config_store.save(&app.runtime_config) {
            Ok(()) => {
                CommandResult::message(format!("Saved config to {}", config_store.path().display()))
            }
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
            ProviderSelection::Scripted { .. } => Err(joker_config::ConfigError::InvalidValue(
                "base_url only applies to OpenAI-compatible providers".into(),
            )),
        },
        "api_key_env" => match &mut app.runtime_config.provider {
            ProviderSelection::OpenAiCompatible(provider) => {
                provider.api_key = std::env::var(value).ok();
                provider.api_key_env = Some(value.into());
                Ok(())
            }
            ProviderSelection::Scripted { .. } => Err(joker_config::ConfigError::InvalidValue(
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

fn tools(app: &App) -> CommandResult {
    let mut tools = vec!["list_files", "read_file", "grep"];
    if app.runtime_config.demo_tool {
        tools.push("echo");
    }
    CommandResult::message(tools.join("\n"))
}

fn unknown(name: &str) -> CommandResult {
    let suggestions = suggestions(name)
        .into_iter()
        .map(|command| format!("/{}", command.name))
        .collect::<Vec<_>>();
    if suggestions.is_empty() {
        CommandResult::error(format!("Unknown command: /{name}. Type /help."))
    } else {
        CommandResult::error(format!(
            "Unknown command: /{name}. Did you mean {}?",
            suggestions.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use joker_config::{ConfigStore, RuntimeConfig};

    use super::*;
    use crate::app::App;

    #[test]
    fn provider_command_switches_session_config() {
        let mut app = App::with_config(RuntimeConfig::default());
        let store = ConfigStore::new("joker-test.toml");

        let result = execute("/provider deepseek", &mut app, &store);

        assert!(!result.is_error);
        assert_eq!(result.action, Some(CommandAction::ConfigChanged));
        assert!(app.runtime_config.provider_label().starts_with("deepseek/"));
    }

    #[test]
    fn model_command_blocks_while_running() {
        let mut app = App::with_config(RuntimeConfig::default());
        app.running = true;
        let store = ConfigStore::new("joker-test.toml");

        let result = execute("/model anything", &mut app, &store);

        assert!(result.is_error);
    }

    #[test]
    fn suggestions_match_prefix() {
        let names = suggestions("/pro")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["provider"]);
    }
}
