use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct ToolsCommand;

impl SlashCommand for ToolsCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "tools",
            usage: "/tools",
            description: "List enabled tools.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        let mut tools = vec!["list_files", "read_file", "grep"];
        if app.runtime_config.demo_tool {
            tools.push("echo");
        }
        CommandResult::message(tools.join("\n"))
    }
}
