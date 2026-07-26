use joker_config::ConfigStore;

use crate::app::{App, TranscriptItem};

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
        app.compact_requested = true;
        app.transcript.push(TranscriptItem::Status(
            "Context compaction requested. SummaryContextBuilder will activate on next run."
                .into(),
        ));
        CommandResult::message(
            "Compact request sent. The next agent run will use summary-based context.",
        )
    }
}
