use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct DenyCommand;

impl SlashCommand for DenyCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "deny",
            usage: "/deny [request_id] [reason?]",
            description: "Deny a pending tool call.",
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
            return CommandResult::error("Usage: /deny <request_id> [reason]");
        };
        let mut parts = args.splitn(2, char::is_whitespace);
        let request_id = parts.next().unwrap_or_default();
        let reason = parts.next();
        if request_id.is_empty() {
            return CommandResult::error("Usage: /deny <request_id> [reason]");
        }
        app.deny_pending(request_id, reason);
        match reason {
            Some(r) => CommandResult::message(format!("Denied: {request_id} ({r})")),
            None => CommandResult::message(format!("Denied: {request_id}")),
        }
    }
}
