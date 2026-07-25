use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct ModelsCommand;

impl SlashCommand for ModelsCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "models",
            usage: "/models",
            description: "List models for the current provider.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        let current = app.runtime_config.current_model();
        let lines: Vec<String> = app
            .runtime_config
            .available_models()
            .iter()
            .map(|m| {
                if m == &current {
                    format!("* {m} (current)")
                } else {
                    format!("  {m}")
                }
            })
            .collect();
        CommandResult::message(lines.join("\n"))
    }
}
