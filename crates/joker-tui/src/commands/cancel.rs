use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

pub(super) struct CancelCommand;

impl SlashCommand for CancelCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "cancel",
            usage: "/cancel",
            description: "Cancel the current run.",
        }
    }

    fn execute(
        &self,
        _app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        CommandResult::action(CommandAction::Cancel)
    }
}
