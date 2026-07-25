use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

pub(super) struct ClearCommand;

impl SlashCommand for ClearCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "clear",
            usage: "/clear",
            description: "Clear the transcript.",
        }
    }

    fn execute(
        &self,
        _app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        CommandResult::action(CommandAction::Clear)
    }
}
