use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

/// Slash command `/compact` for requesting context compaction.
pub(super) struct CompactCommand;

impl SlashCommand for CompactCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "compact",
            usage: "/compact",
            description: "Compact the conversation context.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if app.running
            && let Some(handle) = &app.runtime_handle
        {
            app.compact_requested = false;
            match handle.compact() {
                Ok(()) => CommandResult::message("Compact request sent to the active runtime."),
                Err(error) => CommandResult::error(error.to_string()),
            }
        } else {
            app.compact_requested = true;
            CommandResult::message(
                "Compact request sent. The next agent run will use summary-based context.",
            )
        }
    }
}
