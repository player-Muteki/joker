use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct StatusCommand;

impl SlashCommand for StatusCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "status",
            usage: "/status",
            description: "Show current runtime status.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        _args: Option<&str>,
        config_store: &ConfigStore,
    ) -> CommandResult {
        CommandResult::message(format!(
            "Provider: {}\nModel: {}\nRunning: {}\nTools: {}\nConfig: {}",
            app.runtime_config.provider_label(),
            app.runtime_config.current_model(),
            if app.running { "yes" } else { "no" },
            app.runtime_config.available_models().len(),
            config_store.path().display()
        ))
    }
}
