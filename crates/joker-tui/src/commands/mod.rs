mod agent;
mod compact;
mod model;
mod provider;
mod quit;
pub mod registry;
mod sessions;

use std::sync::LazyLock;

use joker_config::ConfigStore;

use crate::app::App;

// ── Public types ───────────────────────────────────────────────────────────

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
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: false,
        }
    }

    pub fn action(action: CommandAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }

    pub fn with_message_and_action(message: impl Into<String>, action: CommandAction) -> Self {
        Self {
            message: Some(message.into()),
            action: Some(action),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
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

// ── SlashCommand trait ─────────────────────────────────────────────────────

use registry::CommandRegistry;

/// Abstraction for slash commands with built-in autocomplete support.
pub trait SlashCommand: Send + Sync {
    fn info(&self) -> CommandInfo;

    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        config_store: &ConfigStore,
    ) -> CommandResult;

    fn complete(&self, _args_partial: &str) -> Vec<String> {
        vec![]
    }
}

// ── Registry singleton ─────────────────────────────────────────────────────

fn global_registry() -> &'static CommandRegistry {
    static REGISTRY: LazyLock<CommandRegistry> = LazyLock::new(|| {
        let mut registry = CommandRegistry::new();
        use std::sync::Arc;

        registry.register(Arc::new(quit::QuitCommand));
        registry.register(Arc::new(provider::ProviderCommand));
        registry.register(Arc::new(model::ModelCommand));
        registry.register(Arc::new(sessions::SessionsCommand));
        registry.register(Arc::new(compact::CompactCommand));
        registry.register(Arc::new(agent::AgentCommand));

        registry
    });
    &REGISTRY
}

// ── Public API (backwards compatible) ──────────────────────────────────────

pub fn execute(input: &str, app: &mut App, config_store: &ConfigStore) -> CommandResult {
    let result = global_registry().execute(input, app, config_store);

    if let Some(message) = &result.message {
        if result.is_error {
            app.transcript
                .push(crate::app::TranscriptItem::Error(message.clone()));
        } else {
            app.transcript
                .push(crate::app::TranscriptItem::Status(message.clone()));
        }
    }

    result
}

pub fn suggestions(input: &str) -> Vec<CommandInfo> {
    global_registry().complete(input)
}

// ── Tests ──────────────────────────────────────────────────────────────────

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
        let names: Vec<String> = suggestions("/pro")
            .into_iter()
            .map(|info| info.name.to_string())
            .collect();

        assert!(names.contains(&"provider".to_string()));
    }
}
