use joker_config::{ConfigStore, ProviderSelection};
use joker_provider::{ALIBABA, ANTHROPIC, BAIDU, DEEPSEEK, GOOGLE, MOONSHOT, ZHIPUAI};

use crate::app::{App, Dialog, DialogKind, TranscriptItem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub message: Option<String>,
    pub action: Option<CommandAction>,
    pub is_error: bool,
}

impl CommandResult {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: false,
        }
    }

    fn action(action: CommandAction) -> Self {
        Self {
            message: None,
            action: Some(action),
            is_error: false,
        }
    }

    fn with_message_and_action(message: impl Into<String>, action: CommandAction) -> Self {
        Self {
            message: Some(message.into()),
            action: Some(action),
            is_error: false,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            action: None,
            is_error: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandAction {
    Cancel,
    Clear,
    ConfigChanged,
    Quit,
}

pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        usage: "/help [command]",
        description: "Show slash commands.",
    },
    CommandInfo {
        name: "clear",
        usage: "/clear",
        description: "Clear the transcript.",
    },
    CommandInfo {
        name: "quit",
        usage: "/quit",
        description: "Exit Joker.",
    },
    CommandInfo {
        name: "cancel",
        usage: "/cancel",
        description: "Cancel the current run.",
    },
    CommandInfo {
        name: "status",
        usage: "/status",
        description: "Show current runtime status.",
    },
    CommandInfo {
        name: "provider",
        usage: "/provider [name]",
        description: "Select a provider interactively, or switch directly by name.",
    },
    CommandInfo {
        name: "model",
        usage: "/model [name]",
        description: "Select a model interactively, or switch directly by name.",
    },
    CommandInfo {
        name: "models",
        usage: "/models",
        description: "List models for the current provider.",
    },
    CommandInfo {
        name: "config",
        usage: "/config [show|set <key> <value>|save]",
        description: "View, edit, or save configuration.",
    },
    CommandInfo {
        name: "credentials",
        usage: "/credentials",
        description: "List stored API key credentials.",
    },
    CommandInfo {
        name: "tools",
        usage: "/tools",
        description: "List enabled tools.",
    },
    CommandInfo {
        name: "approve",
        usage: "/approve [request_id]",
        description: "Approve a pending tool call.",
    },
    CommandInfo {
        name: "deny",
        usage: "/deny [request_id] [reason?]",
        description: "Deny a pending tool call.",
    },
    CommandInfo {
        name: "sessions",
        usage: "/sessions",
        description: "List saved sessions.",
    },
    CommandInfo {
        name: "compact",
        usage: "/compact",
        description: "Compact the conversation context.",
    },
    CommandInfo {
        name: "agent",
        usage: "/agent [new|switch|list]",
        description: "Manage agent profiles: list, switch active, or create custom agents.",
    },
];

pub fn execute(input: &str, app: &mut App, config_store: &ConfigStore) -> CommandResult {
    let trimmed = input.trim().trim_start_matches('/');
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().to_ascii_lowercase();
    let args = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let result = match name.as_str() {
        "" | "help" => help(args),
        "clear" => CommandResult::action(CommandAction::Clear),
        "quit" | "exit" => CommandResult::action(CommandAction::Quit),
        "cancel" => CommandResult::action(CommandAction::Cancel),
        "status" => status(app, config_store),
        "provider" => provider(app, args),
        "model" => model(app, args),
        "models" => models(app),
        "config" => config(app, config_store, args),
        "tools" => tools(app),
        "credentials" => credentials(app),
        "approve" => approve(app, args),
        "deny" => deny(app, args),
        "sessions" => sessions(app),
        "compact" => compact(app),
        "agent" => agent(app, args),
        other => unknown(other),
    };

    if let Some(message) = &result.message {
        if result.is_error {
            app.transcript.push(TranscriptItem::Error(message.clone()));
        } else {
            app.transcript.push(TranscriptItem::Status(message.clone()));
        }
    }

    result
}

pub fn suggestions(input: &str) -> Vec<&'static CommandInfo> {
    let query = input
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| query.is_empty() || command.name.starts_with(&query))
        .take(6)
        .collect()
}

fn help(args: Option<&str>) -> CommandResult {
    if let Some(name) = args {
        if let Some(command) = COMMANDS.iter().find(|command| command.name == name) {
            return CommandResult::message(format!("{}\n{}", command.usage, command.description));
        }
        return CommandResult::error(format!("Unknown command: /{name}"));
    }

    CommandResult::message(
        COMMANDS
            .iter()
            .map(|command| format!("{:<36} {}", command.usage, command.description))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn status(app: &App, config_store: &ConfigStore) -> CommandResult {
    CommandResult::message(format!(
        "Provider: {}\nModel: {}\nRunning: {}\nTools: {}\nConfig: {}",
        app.runtime_config.provider_label(),
        app.runtime_config.current_model(),
        if app.running { "yes" } else { "no" },
        app.runtime_config.available_models().len(),
        config_store.path().display()
    ))
}

fn provider(app: &mut App, args: Option<&str>) -> CommandResult {
    if app.running {
        return CommandResult::error(
            "Cannot switch provider while a run is active. Use /cancel first.",
        );
    }
    let Some(provider) = args else {
        let options = available_providers_for_dialog();
        app.dialog = Some(Dialog {
            kind: DialogKind::Provider,
            title: "Select Provider".into(),
            options,
            selected: 0,
        });
        return CommandResult {
            message: None,
            action: None,
            is_error: false,
        };
    };
    match app.runtime_config.switch_provider(provider) {
        Ok(()) => CommandResult::with_message_and_action(
            format!(
                "Switched provider to {}",
                app.runtime_config.provider_label()
            ),
            CommandAction::ConfigChanged,
        ),
        Err(error) => CommandResult::error(error.to_string()),
    }
}

fn model(app: &mut App, args: Option<&str>) -> CommandResult {
    if app.running {
        return CommandResult::error(
            "Cannot switch model while a run is active. Use /cancel first.",
        );
    }
    let Some(model) = args else {
        let options = available_models_for_dialog(app);
        if options.is_empty() {
            return CommandResult {
                message: Some("No models available for the current provider.".into()),
                action: None,
                is_error: false,
            };
        }
        app.dialog = Some(Dialog {
            kind: DialogKind::Model,
            title: format!("Select Model ({})", app.runtime_config.provider_label()),
            options,
            selected: 0,
        });
        return CommandResult {
            message: None,
            action: None,
            is_error: false,
        };
    };
    match app.runtime_config.switch_model(model) {
        Ok(()) => CommandResult::with_message_and_action(
            format!("Switched model to {}", app.runtime_config.provider_label()),
            CommandAction::ConfigChanged,
        ),
        Err(error) => CommandResult::error(error.to_string()),
    }
}

fn available_providers_for_dialog() -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();

    let providers: [(&str, &str); 8] = [
        ("DeepSeek", DEEPSEEK.id),
        ("Alibaba Cloud (DashScope)", ALIBABA.id),
        ("ZhipuAI (GLM)", ZHIPUAI.id),
        ("Moonshot (Kimi)", MOONSHOT.id),
        ("Baidu (ERNIE)", BAIDU.id),
        ("Anthropic", ANTHROPIC.id),
        ("Google", GOOGLE.id),
        ("Scripted (no AI)", "scripted"),
    ];

    for (label, id) in &providers {
        result.push((label.to_string(), id.to_string()));
    }

    result
}

fn available_models_for_dialog(app: &App) -> Vec<(String, String)> {
    app.runtime_config
        .available_models()
        .into_iter()
        .map(|m| (m.clone(), m))
        .collect()
}

fn models(app: &App) -> CommandResult {
    let current = app.runtime_config.current_model();
    let lines: Vec<String> = app
        .runtime_config
        .available_models()
        .iter()
        .map(|m| {
            if m == &current {
                format!("* {m} (current)")
            } else {
                format!("  {m}")
            }
        })
        .collect();
    CommandResult::message(lines.join("\n"))
}

fn config(app: &mut App, config_store: &ConfigStore, args: Option<&str>) -> CommandResult {
    let Some(args) = args else {
        return config_show(app, config_store);
    };
    let mut parts = args.splitn(3, char::is_whitespace);
    match parts.next().unwrap_or_default() {
        "show" => config_show(app, config_store),
        "save" => match config_store.save(&app.runtime_config) {
            Ok(()) => {
                CommandResult::message(format!("Saved config to {}", config_store.path().display()))
            }
            Err(error) => CommandResult::error(error.to_string()),
        },
        "set" => {
            if app.running {
                return CommandResult::error(
                    "Cannot edit config while a run is active. Use /cancel first.",
                );
            }
            let Some(key) = parts.next() else {
                return CommandResult::error("Usage: /config set <key> <value>");
            };
            let Some(value) = parts.next() else {
                return CommandResult::error("Usage: /config set <key> <value>");
            };
            config_set(app, key, value)
        }
        other => CommandResult::error(format!("Unknown /config command: {other}")),
    }
}

fn config_show(app: &App, config_store: &ConfigStore) -> CommandResult {
    CommandResult::message(format!(
        "provider = {}\nmodels = [{}]\nscripted_response = {:?}\ndemo_tool = {}\nconfig_path = {}",
        app.runtime_config.provider_label(),
        app.runtime_config.available_models().join(", "),
        app.runtime_config.scripted_response,
        app.runtime_config.demo_tool,
        config_store.path().display()
    ))
}

fn config_set(app: &mut App, key: &str, value: &str) -> CommandResult {
    let result = match key {
        "provider" => app.runtime_config.switch_provider(value),
        "model" => app.runtime_config.switch_model(value),
        "scripted_response" => {
            app.runtime_config.scripted_response = value.into();
            Ok(())
        }
        "demo_tool" => {
            app.runtime_config.demo_tool = matches!(value, "true" | "on" | "yes" | "1");
            Ok(())
        }
        "base_url" => match &mut app.runtime_config.provider {
            ProviderSelection::OpenAiCompatible(provider) => {
                provider.base_url = value.into();
                Ok(())
            }
            ProviderSelection::Scripted { .. }
            | ProviderSelection::Anthropic { .. }
            | ProviderSelection::Google { .. } => Err(joker_config::ConfigError::InvalidValue(
                "base_url only applies to OpenAI-compatible providers".into(),
            )),
        },
        "api_key_env" => match &mut app.runtime_config.provider {
            ProviderSelection::OpenAiCompatible(provider) => {
                provider.api_key = std::env::var(value).ok();
                provider.api_key_env = Some(value.into());
                Ok(())
            }
            ProviderSelection::Scripted { .. }
            | ProviderSelection::Anthropic { .. }
            | ProviderSelection::Google { .. } => Err(joker_config::ConfigError::InvalidValue(
                "api_key_env only applies to OpenAI-compatible providers".into(),
            )),
        },
        _ => Err(joker_config::ConfigError::InvalidValue(format!(
            "unknown config key: {key}"
        ))),
    };

    match result {
        Ok(()) => CommandResult::with_message_and_action(
            format!("Set {key} = {value}"),
            CommandAction::ConfigChanged,
        ),
        Err(error) => CommandResult::error(error.to_string()),
    }
}

fn tools(app: &App) -> CommandResult {
    let mut tools = vec!["list_files", "read_file", "grep"];
    if app.runtime_config.demo_tool {
        tools.push("echo");
    }
    CommandResult::message(tools.join("\n"))
}

fn approve(app: &mut App, args: Option<&str>) -> CommandResult {
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

fn deny(app: &mut App, args: Option<&str>) -> CommandResult {
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

fn sessions(app: &App) -> CommandResult {
    if app.running {
        return CommandResult::error("Cannot list sessions while a run is active.");
    }
    if let Some(ref store) = app.session_store {
        let store = store.clone();
        let result = tokio::runtime::Handle::current()
            .block_on(async move { store.list().await });
        match result {
            Ok(sessions) if sessions.is_empty() => {
                return CommandResult::message("No saved sessions.");
            }
            Ok(sessions) => {
                let lines: Vec<String> = sessions.iter().map(|s| {
                    format!("  {} | {} | {} msgs | {}", s.id, s.label, s.message_count,
                        format_timestamp(s.updated_at))
                }).collect();
                let mut output = format!("Sessions ({}):
", sessions.len());
                output.push_str(&lines.join("
"));
                CommandResult::message(output)
            }
            Err(_) => CommandResult::error("Failed to list sessions."),
        }
    } else {
        CommandResult::message("No session store configured. Sessions are not persisted.")
    }
}

fn compact(app: &mut App) -> CommandResult {
    app.compact_requested = true;
    app.transcript.push(TranscriptItem::Status(
        "Context compaction requested. SummaryContextBuilder will activate on next run.".into(),
    ));
    CommandResult::message("Compact request sent. The next agent run will use summary-based context.")
}


fn agent(app: &mut App, args: Option<&str>) -> CommandResult {
    if app.running {
        return CommandResult::error("Cannot manage agents while a run is active. Use /cancel first.");
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
                CommandResult { message: None, action: None, is_error: false }
            }
        }
        "new" => {
            // Launch the multi-step agent creation wizard
            app.dialog = Some(Dialog {
                kind: DialogKind::AgentNew { step: 0 },
                title: "New Agent — Name".into(),
                options: Vec::new(),
                selected: 0,
            });
            app.agent_new_state = Some(crate::app::AgentNewState::default());
            CommandResult { message: None, action: None, is_error: false }
        }
        other => CommandResult::error(format!(
            "Unknown /agent subcommand: {other}. Use: list, switch, new"
        )),
    }
}

fn agent_list(app: &App) -> CommandResult {
    if app.agent_names.is_empty() {
        CommandResult::message("No agent profiles configured. Use /agent new to create one.")
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

fn credentials(app: &App) -> CommandResult {
    if app.credential_store.is_empty() {
        return CommandResult::message("No API key credentials stored. Use /provider to add one.");
    }
    let lines: Vec<String> = app
        .credential_store
        .list()
        .into_iter()
        .map(|provider| {
            format!("  {provider}: **** (stored)")
        })
        .collect();
    CommandResult::message(format!("Credentials ({}):\n{}", lines.len(), lines.join("\n")))
}

fn unknown(name: &str) -> CommandResult {
    let suggestions = suggestions(name)
        .into_iter()
        .map(|command| format!("/{}", command.name))
        .collect::<Vec<_>>();
    if suggestions.is_empty() {
        CommandResult::error(format!("Unknown command: /{name}. Type /help."))
    } else {
        CommandResult::error(format!(
            "Unknown command: /{name}. Did you mean {}?",
            suggestions.join(", ")
        ))
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
        if days > 0 { format!("{days}d ago") }
        else if hours > 0 { format!("{hours}h ago") }
        else { format!("{mins}m ago") }
    } else {
        "just now".to_string()
    }
}

#[cfg(test)]
mod tests {
    use joker_config::{ConfigStore, RuntimeConfig};

    use super::*;
    use crate::app::App;

    #[test]
    fn provider_command_switches_session_config() {
        let mut app = App::with_config(RuntimeConfig::default());
        let store = ConfigStore::new("joker-test.toml");

        let result = execute("/provider deepseek", &mut app, &store);

        assert!(!result.is_error);
        assert_eq!(result.action, Some(CommandAction::ConfigChanged));
        assert!(app.runtime_config.provider_label().starts_with("deepseek/"));
    }

    #[test]
    fn model_command_blocks_while_running() {
        let mut app = App::with_config(RuntimeConfig::default());
        app.running = true;
        let store = ConfigStore::new("joker-test.toml");

        let result = execute("/model anything", &mut app, &store);

        assert!(result.is_error);
    }

    #[test]
    fn suggestions_match_prefix() {
        let names = suggestions("/pro")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["provider"]);
    }
}
