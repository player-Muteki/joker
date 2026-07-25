use joker_config::ConfigStore;

use crate::app::{App, ToolState, TranscriptItem};

use super::{CommandAction, CommandInfo, CommandResult, SlashCommand};

pub(super) struct SessionsCommand;

impl SlashCommand for SessionsCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "sessions",
            usage: "/sessions [list|load <id>|delete <id>]",
            description: "List, load, or delete saved sessions.",
        }
    }

    fn execute(
        &self,
        app: &mut App,
        args: Option<&str>,
        _config_store: &ConfigStore,
    ) -> CommandResult {
        if app.running {
            return CommandResult::error("Cannot manage sessions while a run is active.");
        }
        let Some(args) = args else {
            return sessions_list(app);
        };
        let mut parts = args.splitn(2, char::is_whitespace);
        match parts.next().unwrap_or_default() {
            "list" | "" => sessions_list(app),
            "load" => {
                let Some(id) = parts.next() else {
                    return CommandResult::error("Usage: /sessions load <id>");
                };
                sessions_load(app, id)
            }
            "delete" => {
                let Some(id) = parts.next() else {
                    return CommandResult::error("Usage: /sessions delete <id>");
                };
                sessions_delete(app, id)
            }
            other => CommandResult::error(format!(
                "Unknown /sessions subcommand: {other}. Use: list, load <id>, delete <id>"
            )),
        }
    }

    fn complete(&self, args_partial: &str) -> Vec<String> {
        let partial = args_partial.to_ascii_lowercase();
        ["list", "load", "delete"]
            .iter()
            .filter(|s| s.starts_with(&partial))
            .map(|s| s.to_string())
            .collect()
    }
}

fn sessions_list(app: &App) -> CommandResult {
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

fn sessions_load(app: &mut App, id: &str) -> CommandResult {
    if let Some(ref store) = app.session_store {
        let store = store.clone();
        let id_owned = id.to_string();
        let id_clone = id_owned.clone();
        let result = tokio::runtime::Handle::current()
            .block_on(async move { store.load(&id_owned).await });
        match result {
            Ok(Some(data)) => {
                app.transcript.clear();
                app.loaded_conversation = Some(data.conversation.clone());
                app.active_agent = data.agent_name.clone();
                for msg in data.conversation.into_messages() {
                    match msg.role {
                        joker::Role::User => {
                            if let Some(text) = msg.content.iter().find_map(|c| match c {
                                joker::Content::Text(t) => Some(t.text.clone()),
                                _ => None,
                            }) {
                                app.transcript.push(TranscriptItem::User(text));
                            }
                        }
                        joker::Role::Assistant => {
                            let text = msg.content.iter().filter_map(|c| match c {
                                joker::Content::Text(t) => Some(t.text.clone()),
                                _ => None,
                            }).collect::<Vec<_>>().join("\n");
                            if !text.is_empty() {
                                app.transcript.push(TranscriptItem::Assistant {
                                    text,
                                    streaming: false,
                                });
                            }
                            for call in msg.content.iter().filter_map(|c| match c {
                                joker::Content::ToolCall(tc) => Some(tc),
                                _ => None,
                            }) {
                                app.transcript.push(TranscriptItem::Tool {
                                    call_id: call.id.clone(),
                                    name: call.name.clone(),
                                    state: ToolState::Done(String::new()),
                                });
                            }
                        }
                        joker::Role::Tool => {
                            let text = msg.content.iter().filter_map(|c| match c {
                                joker::Content::ToolResult(tr) => Some(tr.output.to_string()),
                                _ => None,
                            }).collect::<Vec<_>>().join("\n");
                            if !text.is_empty() {
                                app.transcript.push(TranscriptItem::Status(format!("Tool result: {text}")));
                            }
                        }
                        joker::Role::System => {},
                        _ => {}
                    }
                }
                CommandResult::with_message_and_action(
                    format!("Loaded session: {} ({})", data.id, data.label),
                    CommandAction::ConfigChanged,
                )
            }
            Ok(None) => CommandResult::error(format!("Session not found: {id_clone}")),
            Err(e) => CommandResult::error(format!("Failed to load session: {e}")),
        }
    } else {
        CommandResult::message("No session store configured.")
    }
}

fn sessions_delete(app: &mut App, id: &str) -> CommandResult {
    if let Some(ref store) = app.session_store {
        let store = store.clone();
        let id_owned = id.to_string();
        let id_clone = id_owned.clone();
        let result = tokio::runtime::Handle::current()
            .block_on(async move { store.delete(&id_owned).await });
        match result {
            Ok(()) => CommandResult::message(format!("Deleted session: {id_clone}")),
            Err(e) => CommandResult::error(format!("Failed to delete session: {e}")),
        }
    } else {
        CommandResult::message("No session store configured.")
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
