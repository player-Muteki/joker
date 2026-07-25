use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::{
    event::{self, Event as CrosstermEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use joker::SharedApprovalChannel;
use joker_provider::CredentialSource;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    TuiError,
    app::{App, AppAction, DialogKind, TranscriptItem},
    commands::{self, CommandAction},
    driver::AgentDriver,
    event::UiEvent,
    widgets,
};

#[derive(Clone, Debug)]
pub struct TuiOptions {
    pub initial_prompt: Option<String>,
    pub use_alt_screen: bool,
    pub config_store: joker_config::ConfigStore,
    pub runtime_config: joker_config::RuntimeConfig,
}

pub async fn run_tui(options: TuiOptions) -> Result<(), TuiError> {
    let (mut terminal, _guard) = setup_terminal(options.use_alt_screen)?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_terminal_events(tx.clone());
    spawn_tick_events(tx.clone());

    let mut app = App::with_config(options.runtime_config.clone());
    let workspace = std::env::current_dir()?;
    let mut driver = AgentDriver::new(options.runtime_config.clone(), workspace);

    // Connect to MCP servers configured in joker.toml
    driver.init_mcp_servers().await;

    // Set up session store in .joker/sessions/
    let session_dir = std::env::current_dir().unwrap_or_default().join(".joker").join("sessions");
    if let Ok(store) = joker::JsonlSessionStore::new(&session_dir) {
        app.session_store = Some(std::sync::Arc::new(store));
    }

    // Set up credential store in ~/.joker/auth.json
    if let Ok(home) = std::env::var("HOME") {
        let cred_path = std::path::PathBuf::from(home).join(".joker").join("auth.json");
        app.set_credential_path(cred_path);
    }

    if let Some(prompt) = options.initial_prompt {
        app.composer = prompt;
        app.cursor = app.composer.len();
        if let Some(action) = app.submit_prompt() {
            handle_action(
                &mut app,
                &mut driver,
                &options.config_store,
                action,
                tx.clone(),
            );
        }
    }

    terminal.draw(|frame| widgets::layout::render(frame, &app))?;

    while !app.should_quit {
        let Some(event) = rx.recv().await else {
            return Err(TuiError::ChannelClosed);
        };

        match event {
            UiEvent::Terminal(CrosstermEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                if let Some(action) = app.handle_key(key) {
                    handle_action(
                        &mut app,
                        &mut driver,
                        &options.config_store,
                        action,
                        tx.clone(),
                    );
                }
            }
            UiEvent::Terminal(CrosstermEvent::Resize(_, _)) | UiEvent::Tick => {}
            UiEvent::Agent(_) | UiEvent::RunCompleted(_) => app.apply_ui_event(event),
            UiEvent::Terminal(_) => {}
        }

        terminal.draw(|frame| widgets::layout::render(frame, &app))?;
    }

    Ok(())
}

fn setup_terminal(
    use_alt_screen: bool,
) -> Result<(Terminal<CrosstermBackend<Stdout>>, TerminalGuard), TuiError> {
    enable_raw_mode().map_err(|error| TuiError::Terminal(error.to_string()))?;
    let mut guard = TerminalGuard {
        use_alt_screen: false,
    };
    if use_alt_screen {
        execute!(io::stdout(), EnterAlternateScreen)?;
        guard.use_alt_screen = true;
    }

    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend).map_err(TuiError::Io)?;
    Ok((terminal, guard))
}

fn spawn_terminal_events(tx: mpsc::UnboundedSender<UiEvent>) {
    std::thread::spawn(move || {
        while !tx.is_closed() {
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        let _ = tx.send(UiEvent::Terminal(event));
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
}

fn spawn_tick_events(tx: mpsc::UnboundedSender<UiEvent>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        while !tx.is_closed() {
            interval.tick().await;
            let _ = tx.send(UiEvent::Tick);
        }
    });
}

fn handle_action(
    app: &mut App,
    driver: &mut AgentDriver,
    config_store: &joker_config::ConfigStore,
    action: AppAction,
    tx: mpsc::UnboundedSender<UiEvent>,
) {
    match action {
        AppAction::Submit(prompt) => spawn_agent_run(app, driver, prompt, tx),
        AppAction::Command(command) => {
            let result = commands::execute(&command, app, config_store);
            if let Some(action) = result.action {
                handle_command_action(app, driver, action);
            }
        }
        AppAction::DialogConfirm { kind, selection } => {
            handle_dialog_confirm(app, driver, kind, &selection);
        }
        AppAction::ApiKeyConfirm { provider_id, api_key } => {
            handle_api_key_confirm(app, driver, &provider_id, &api_key);
        }
        AppAction::Cancel => app.cancel_running(),
        AppAction::Quit => app.quit(),
        AppAction::Redraw => {}
        AppAction::AgentCreate { name, tool_permissions } => {
            handle_agent_create(app, driver, config_store, &name, &tool_permissions);
        }
    }
}

fn handle_api_key_confirm(
    app: &mut App,
    driver: &mut AgentDriver,
    provider_id: &str,
    api_key: &str,
) {
    // Store credential in persistent store and save to disk
    app.credential_store.set(provider_id, api_key.to_string());
    let _ = app.credential_store.save();

    // Set the API key in the runtime config so model building picks it up
    if let joker_config::ProviderSelection::Route(route) = &mut app.runtime_config.provider {
        route.auth.credentials = CredentialSource::Value(api_key.to_string());
    }

    driver.set_runtime_config(app.runtime_config.clone());
    app.transcript.push(crate::app::TranscriptItem::Status(
        format!("API key stored. Switched to provider: {}", app.runtime_config.provider_label()),
    ));
}

fn handle_agent_create(
    app: &mut App,
    driver: &mut AgentDriver,
    config_store: &joker_config::ConfigStore,
    name: &str,
    tool_permissions: &[(String, String)],
) {
    use std::collections::{BTreeMap, HashMap};
    use joker::{AgentPermission, PermissionSetting, ToolName};

    let mut perms = HashMap::new();
    let mut file_tools = BTreeMap::new();
    for (label, tool_name) in tool_permissions {
        let setting = if label.starts_with("[Auto]") {
            PermissionSetting::AutoAccept
        } else if label.starts_with("[Off]") {
            PermissionSetting::Disabled
        } else {
            PermissionSetting::Ask
        };
        let permission_str = if label.starts_with("[Auto]") {
            "auto-accept"
        } else if label.starts_with("[Off]") {
            "disabled"
        } else {
            "ask"
        };
        perms.insert(ToolName::new(tool_name), setting);
        file_tools.insert(
            tool_name.clone(),
            joker_config::ToolPermissionConfig {
                enabled: None,
                permission: Some(permission_str.into()),
            },
        );
    }

    let agents_dir = driver.workspace().join(".joker").join("agents");
    let agent_perm = AgentPermission {
        agent_name: name.to_string(),
        tool_permissions: perms,
        constraint_file: agents_dir.join(format!("{name}_agent.md")),
        hard_permission: None,
        model: None,
    };

    driver.permission_engine_mut().register(agent_perm);
    app.agent_names.push(name.to_string());

    // Persist to joker.toml
    if let Ok(raw) = std::fs::read_to_string(config_store.path())
        && let Ok(mut file) = toml::from_str::<joker_config::FileConfig>(&raw)
    {
        file.agent.insert(
            name.to_string(),
            joker_config::AgentProfileConfig {
                model: None,
                system: None,
                tools: file_tools,
                permissions: joker_config::PermissionRuleConfig::default(),
            },
        );
        if let Ok(raw) = toml::to_string_pretty(&file) {
            let _ = std::fs::write(config_store.path(), raw);
        }
    }

    // Generate constraint file
    let constraint_path = agents_dir.join(format!("{name}_agent.md"));
    let constraint_content = format!(
        r##"# {name} Agent

Custom agent created via /agent new.

## Behavior
- Describe what this agent should do.
- Set expectations for tool usage and permission levels.
"##
    );
    let _ = std::fs::write(&constraint_path, constraint_content);

    app.transcript.push(TranscriptItem::Status(format!(
        "Agent '{name}' created, registered, and saved to config."
    )));
}

fn handle_dialog_confirm(
    app: &mut App,
    driver: &mut AgentDriver,
    kind: DialogKind,
    selection: &str,
) {
    match kind {
        DialogKind::Provider => match app.runtime_config.switch_provider(selection) {
            Ok(()) => {
                app.status = format!("Idle ({})", app.runtime_config.provider_label());
                driver.set_runtime_config(app.runtime_config.clone());
                app.transcript.push(TranscriptItem::Status(format!(
                    "Switched provider to {}",
                    app.runtime_config.provider_label()
                )));
            }
            Err(error) => {
                app.transcript
                    .push(TranscriptItem::Error(error.to_string()));
            }
        },
        DialogKind::Model | DialogKind::ApiKeyInput { .. } => match app.runtime_config.switch_model(selection) {
            Ok(()) => {
                app.status = format!("Idle ({})", app.runtime_config.provider_label());
                driver.set_runtime_config(app.runtime_config.clone());
                app.transcript.push(TranscriptItem::Status(format!(
                    "Switched model to {}",
                    app.runtime_config.provider_label()
                )));
            }
            Err(error) => {
                app.transcript
                    .push(TranscriptItem::Error(error.to_string()));
            }
        },
        DialogKind::AgentSwitch => {
            app.active_agent = selection.to_string();
            driver.set_active_agent(selection.to_string());
            app.transcript.push(TranscriptItem::Status(format!(
                "Switched agent to: {selection}"
            )));
        }
        DialogKind::AgentNew { .. } => {
            // Handled internally by the wizard; shouldn't reach here
        }
    }
}

fn handle_command_action(app: &mut App, driver: &mut AgentDriver, action: CommandAction) {
    match action {
        CommandAction::Cancel => app.cancel_running(),
        CommandAction::Clear => {
            app.transcript.clear();
            app.transcript
                .push(TranscriptItem::Status("Transcript cleared.".into()));
        }
        CommandAction::ConfigChanged => {
            app.status = format!("Idle ({})", app.runtime_config.provider_label());
            driver.set_runtime_config(app.runtime_config.clone());
        }
        CommandAction::Quit => app.quit(),
    }
}

fn spawn_agent_run(
    app: &mut App,
    driver: &mut AgentDriver,
    prompt: String,
    tx: mpsc::UnboundedSender<UiEvent>,
) {
    let cancellation_token = CancellationToken::new();
    app.cancellation_token = Some(cancellation_token.clone());

    // Pass compact flag from app to driver, then reset
    if app.compact_requested {
        driver.set_compact_pending(true);
        app.compact_requested = false;
    }

    // Create a shared approval channel for UI <-> agent communication
    let approval_channel = SharedApprovalChannel::new();
    app.approval_channel = Some(approval_channel.clone());

    if let Err(error) = driver.spawn_run(prompt, cancellation_token, tx, approval_channel) {
        app.running = false;
        app.cancellation_token = None;
        app.approval_channel = None;
        app.status = "Error".into();
        app.transcript
            .push(crate::app::TranscriptItem::Error(error.to_string()));
    }
}

struct TerminalGuard {
    use_alt_screen: bool,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.use_alt_screen {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}
