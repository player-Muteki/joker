use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

/// Slash command `/exit` for quitting the application.
pub(super) struct QuitCommand;

impl SlashCommand for QuitCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "exit",
            usage: "/exit",
            description: "Exit Joker.",
        }
    }

    fn execute(
        &self,
        _app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        CommandResult::action(CommandAction::Quit)
    }
}
