use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::{
    event::{self, Event as CrosstermEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
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
    let mut driver = AgentDriver::new(options.runtime_config, workspace);

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
        AppAction::Cancel => app.cancel_running(),
        AppAction::Quit => app.quit(),
        AppAction::Redraw => {}
    }
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
        DialogKind::Model => match app.runtime_config.switch_model(selection) {
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
    driver: &AgentDriver,
    prompt: String,
    tx: mpsc::UnboundedSender<UiEvent>,
) {
    let cancellation_token = CancellationToken::new();
    app.cancellation_token = Some(cancellation_token.clone());
    if let Err(error) = driver.spawn_run(prompt, cancellation_token, tx) {
        app.running = false;
        app.cancellation_token = None;
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
