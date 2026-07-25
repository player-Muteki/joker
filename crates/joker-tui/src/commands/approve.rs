use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct ApproveCommand;

impl SlashCommand for ApproveCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "approve",
            usage: "/approve [request_id]",
            description: "Approve a pending tool call.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if !app.running {
            return CommandResult::error("No run is active.");
        }
        let Some(args) = args else {
            return CommandResult::error("Usage: /approve <request_id> [--session]");
        };
        let mut parts = args.split_whitespace();
        let request_id = parts.next().unwrap_or("").to_string();
        if request_id.is_empty() {
            return CommandResult::error("Usage: /approve <request_id> [--session]");
        }
        let remember_for_session = parts.any(|p| p == "--session" || p == "-s");
        app.approve_pending(&request_id, remember_for_session);
        if remember_for_session {
            CommandResult::message(format!("Approved: {request_id} (remember for session)"))
        } else {
            CommandResult::message(format!("Approved: {request_id}"))
        }
    }
}
