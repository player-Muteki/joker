use joker_config::ConfigStore;

use crate::app::{App, AgentNewState, Dialog, DialogKind};

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

pub(super) struct AgentCommand;

impl SlashCommand for AgentCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "agent",
            usage: "/agent [new|switch|list]",
            description: "Manage agent profiles: list, switch active, or create custom agents.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if app.running {
            return CommandResult::error(
                "Cannot manage agents while a run is active. Use /cancel first.",
            );
        }
        let Some(args) = args else {
            return agent_list(app);
        };
        let mut parts = args.splitn(2, char::is_whitespace);
        match parts.next().unwrap_or_default() {
            "list" => agent_list(app),
            "switch" => {
                let options: Vec<(String, String)> = app
                    .agent_names
                    .iter()
                    .map(|name| (name.clone(), name.clone()))
                    .collect();
                if options.is_empty() {
                    return CommandResult::error("No agent profiles found.");
                }
                if let Some(target) = parts.next() {
                    if app.agent_names.contains(&target.to_string()) {
                        app.active_agent = target.to_string();
                        CommandResult::with_message_and_action(
                            format!("Switched to agent: {target}"),
                            CommandAction::ConfigChanged,
                        )
                    } else {
                        CommandResult::error(format!(
                            "Unknown agent: {target}. Available: {}",
                            app.agent_names.join(", ")
                        ))
                    }
                } else {
                    app.dialog = Some(Dialog {
                        kind: DialogKind::AgentSwitch,
                        title: "Select Agent".into(),
                        options,
                        selected: 0,
                    });
                    CommandResult {
                        message: None,
                        action: None,
                        is_error: false,
                    }
                }
            }
            "new" => {
                app.dialog = Some(Dialog {
                    kind: DialogKind::AgentNew { step: 0 },
                    title: "New Agent — Name".into(),
                    options: Vec::new(),
                    selected: 0,
                });
                app.agent_new_state = Some(AgentNewState::default());
                CommandResult {
                    message: None,
                    action: None,
                    is_error: false,
                }
            }
            other => CommandResult::error(format!(
                "Unknown /agent subcommand: {other}. Use: list, switch, new"
            )),
        }
    }

    fn complete(&self, args_partial: &str) -> Vec<String> {
        let partial = args_partial.to_ascii_lowercase();
        ["list", "switch", "new"]
            .iter()
            .filter(|s| s.starts_with(&partial))
            .map(|s| s.to_string())
            .collect()
    }
}

fn agent_list(app: &App) -> CommandResult {
    if app.agent_names.is_empty() {
        CommandResult::message(
            "No agent profiles configured. Use /agent new to create one.",
        )
    } else {
        let lines: Vec<String> = app
            .agent_names
            .iter()
            .map(|name| {
                if name == &app.active_agent {
                    format!("* {name} (active)")
                } else {
                    format!("  {name}")
                }
            })
            .collect();
        CommandResult::message(format!("Agent profiles:\n{}", lines.join("\n")))
    }
}
