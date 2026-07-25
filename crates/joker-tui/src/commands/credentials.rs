use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct CredentialsCommand;

impl SlashCommand for CredentialsCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "credentials",
            usage: "/credentials",
            description: "List stored API key credentials.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if app.credential_store.is_empty() {
            return CommandResult::message(
                "No API key credentials stored. Use /provider to add one.",
            );
        }
        let lines: Vec<String> = app
            .credential_store
            .list()
            .into_iter()
            .map(|provider| format!("  {provider}: **** (stored)"))
            .collect();
        CommandResult::message(format!(
            "Credentials ({}):\n{}",
            lines.len(),
            lines.join("\n")
        ))
    }
}
