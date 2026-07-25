use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct HelpCommand;

impl SlashCommand for HelpCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "help",
            usage: "/help [command]",
            description: "Show slash commands.",
        }
    }

    fn execute(
        &self,
        _app: &mut App,
        args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        // The registry handles the actual help listing via execute().
        // This standalone handler provides a basic message when called without registry.
        if let Some(_name) = args {
            CommandResult::message("Use /help for the full command list.")
        } else {
            CommandResult::message("Type /help for available commands.")
        }
    }

    fn complete(&self, _args_partial: &str) -> Vec<String> {
        vec![]
    }
}
