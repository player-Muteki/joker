//! Slash-command system for the TUI.
//!
//! Commands are triggered by typing `/` followed by a command name in the
//! composer.  Each command implements the [`SlashCommand`] trait; a global
//! [`registry::CommandRegistry`] singleton handles dispatch and fuzzy
//! autocomplete.  Public API functions [`execute`] and [`suggestions`]
//! provide backwards-compatible entry points.

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

/// Metadata describing a registered slash command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    /// Canonical command name (without the `/` prefix).
    pub name: &'static str,
    /// Usage string shown in help, e.g. `"/provider [name]"`.
    pub usage: &'static str,
    /// One-line description of what the command does.
    pub description: &'static str,
}

/// Result returned by a slash command handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    /// Optional status/error message to display.
    pub message: Option<String>,
    /// Optional action for the driver loop to process.
    pub action: Option<CommandAction>,
    /// Whether the command indicates an error condition.
    pub is_error: bool,
}

impl CommandResult {
    /// Create a result with a status message and no action.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: false,
        }
    }

    /// Create a result with an action for the driver and no message.
    pub fn action(action: CommandAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }

    /// Create a result with both a message and an action.
    pub fn with_message_and_action(message: impl Into<String>, action: CommandAction) -> Self {
        Self {
            message: Some(message.into()),
            action: Some(action),
            is_error: false,
        }
    }

    /// Create a result representing an error.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: true,
        }
    }
}

/// Side-effect action for the driver loop after a command executes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandAction {
    /// Cancel the current agent run.
    Cancel,
    /// Clear the transcript display.
    Clear,
    /// Provider/model/agent config changed — re-sync the driver.
    ConfigChanged,
    /// Quit the application.
    Quit,
}

// ── SlashCommand trait ─────────────────────────────────────────────────────

use registry::CommandRegistry;

/// Abstraction for slash commands with built-in autocomplete support.
pub trait SlashCommand: Send + Sync {
    /// Return static metadata about this command.
    fn info(&self) -> CommandInfo;

    /// Execute the command against the given [`App`] state.
    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        config_store: &ConfigStore,
    ) -> CommandResult;

    /// Return partial-argument completion candidates (empty by default).
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

/// Parse and execute a slash command by dispatching to the global registry.
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

/// Return autocomplete suggestions for the given partial input.
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
