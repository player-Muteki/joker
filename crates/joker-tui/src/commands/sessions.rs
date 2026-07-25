use joker_config::ConfigStore;

use crate::app::App;

use super::{CommandInfo, CommandResult, SlashCommand};

pub(super) struct SessionsCommand;

impl SlashCommand for SessionsCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "sessions",
            usage: "/sessions",
            description: "List saved sessions.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        _args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if app.running {
            return CommandResult::error("Cannot list sessions while a run is active.");
        }
        if let Some(ref store) = app.session_store {
            let store = store.clone();
            let result = tokio::runtime::Handle::current()
                .block_on(async move { store.list().await });
            match result {
                Ok(sessions) if sessions.is_empty() => {
                    CommandResult::message("No saved sessions.")
                }
                Ok(sessions) => {
                    let lines: Vec<String> = sessions
                        .iter()
                        .map(|s| {
                            format!(
                                "  {} | {} | {} msgs | {}",
                                s.id,
                                s.label,
                                s.message_count,
                                format_timestamp(s.updated_at)
                            )
                        })
                        .collect();
                    let mut output = format!("Sessions ({}):\n", sessions.len());
                    output.push_str(&lines.join("\n"));
                    CommandResult::message(output)
                }
                Err(_) => CommandResult::error("Failed to list sessions."),
            }
        } else {
            CommandResult::message("No session store configured. Sessions are not persisted.")
        }
    }
}

fn format_timestamp(unix_secs: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = std::time::Duration::from_secs(unix_secs);
    let sys = UNIX_EPOCH + dur;
    if let Ok(diff) = SystemTime::now().duration_since(sys) {
        let mins = diff.as_secs() / 60;
        let hours = mins / 60;
        let days = hours / 24;
        if days > 0 {
            format!("{days}d ago")
        } else if hours > 0 {
            format!("{hours}h ago")
        } else {
            format!("{mins}m ago")
        }
    } else {
        "just now".to_string()
    }
}
