use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use joker_config::RuntimeConfig;
use tokio_util::sync::CancellationToken;

use crate::event::UiEvent;

#[derive(Clone, Debug)]
pub struct App {
    pub composer: String,
    pub cursor: usize,
    pub transcript: Vec<TranscriptItem>,
    pub running: bool,
    pub should_quit: bool,
    pub status: String,
    pub scroll: u16,
    pub cancellation_token: Option<CancellationToken>,
    pub runtime_config: RuntimeConfig,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(RuntimeConfig::default())
    }

    #[must_use]
    pub fn with_config(runtime_config: RuntimeConfig) -> Self {
        let status = format!("Idle ({})", runtime_config.provider_label());
        Self {
            composer: String::new(),
            cursor: 0,
            transcript: vec![TranscriptItem::Status("Welcome to Joker TUI".into())],
            running: false,
            should_quit: false,
            status,
            scroll: 0,
            cancellation_token: None,
            runtime_config,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.running {
                    Some(AppAction::Cancel)
                } else {
                    Some(AppAction::Quit)
                }
            }
            KeyCode::Esc => {
                if self.running {
                    Some(AppAction::Cancel)
                } else {
                    Some(AppAction::Quit)
                }
            }
            KeyCode::Enter => self.submit_prompt(),
            KeyCode::Backspace => {
                if let Some(previous) = previous_char_boundary(&self.composer, self.cursor) {
                    self.composer.drain(previous..self.cursor);
                    self.cursor = previous;
                }
                Some(AppAction::Redraw)
            }
            KeyCode::Delete => {
                if let Some(next) = next_char_boundary(&self.composer, self.cursor) {
                    self.composer.drain(self.cursor..next);
                }
                Some(AppAction::Redraw)
            }
            KeyCode::Left => {
                self.cursor = previous_char_boundary(&self.composer, self.cursor).unwrap_or(0);
                Some(AppAction::Redraw)
            }
            KeyCode::Right => {
                self.cursor =
                    next_char_boundary(&self.composer, self.cursor).unwrap_or(self.composer.len());
                Some(AppAction::Redraw)
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(3);
                Some(AppAction::Redraw)
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(3);
                Some(AppAction::Redraw)
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.cursor.min(self.composer.len());
                self.composer.insert(self.cursor, ch);
                self.cursor += ch.len_utf8();
                Some(AppAction::Redraw)
            }
            _ => None,
        }
    }

    pub fn submit_prompt(&mut self) -> Option<AppAction> {
        let prompt = self.composer.trim().to_string();
        if prompt.is_empty() {
            return None;
        }
        if prompt.starts_with('/') {
            self.composer.clear();
            self.cursor = 0;
            return Some(AppAction::Command(prompt));
        }
        if self.running {
            return None;
        }
        self.composer.clear();
        self.cursor = 0;
        self.transcript.push(TranscriptItem::User(prompt.clone()));
        self.running = true;
        self.status = "Running".into();
        Some(AppAction::Submit(prompt))
    }

    pub fn cancel_running(&mut self) {
        if let Some(token) = &self.cancellation_token {
            token.cancel();
        }
        self.cancellation_token = None;
        self.running = false;
        self.status = "Cancelled".into();
        self.transcript
            .push(TranscriptItem::Status("Cancelled".into()));
    }

    pub fn quit(&mut self) {
        if self.running {
            self.cancel_running();
        }
        self.should_quit = true;
    }

    pub fn apply_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Agent(event) => self.apply_agent_event(event),
            UiEvent::RunCompleted(result) => {
                self.running = false;
                self.cancellation_token = None;
                match result {
                    Ok(_) => {
                        self.status = format!("Idle ({})", self.runtime_config.provider_label())
                    }
                    Err(error) => {
                        self.status = "Error".into();
                        self.transcript.push(TranscriptItem::Error(error));
                    }
                }
            }
            UiEvent::Tick | UiEvent::Terminal(_) => {}
        }
    }

    fn apply_agent_event(&mut self, event: joker::Event) {
        match event {
            joker::Event::RunStarted => {
                self.status = "Running".into();
            }
            joker::Event::ModelStarted => {
                self.status = "Model streaming".into();
                self.ensure_streaming_assistant();
            }
            joker::Event::ModelDelta { delta } => {
                self.push_assistant_delta(&delta);
            }
            joker::Event::ModelFinished { .. } => {
                self.finish_streaming_assistant();
            }
            joker::Event::ToolStarted { call_id, name } => {
                self.status = format!("Tool {name} running");
                self.transcript.push(TranscriptItem::Tool {
                    call_id,
                    name,
                    state: ToolState::Running,
                });
            }
            joker::Event::ToolFinished { result } => {
                self.status = format!("Tool {} finished", result.name);
                if let Some(item) = self.transcript.iter_mut().rev().find(|item| {
                    matches!(item, TranscriptItem::Tool { call_id, .. } if call_id == &result.call_id)
                }) {
                    *item = TranscriptItem::Tool {
                        call_id: result.call_id,
                        name: result.name,
                        state: if result.is_error {
                            ToolState::Error(result.output.to_string())
                        } else {
                            ToolState::Done(result.output.to_string())
                        },
                    };
                }
            }
            joker::Event::LimitReached { reason } => {
                self.transcript
                    .push(TranscriptItem::Status(format!("Limit reached: {reason}")));
            }
            joker::Event::RunFinished { .. } => {
                self.status = "Finishing".into();
            }
            joker::Event::ToolDelta { .. } => {}
            _ => {}
        }
    }

    fn ensure_streaming_assistant(&mut self) {
        if !matches!(
            self.transcript.last(),
            Some(TranscriptItem::Assistant {
                streaming: true,
                ..
            })
        ) {
            self.transcript.push(TranscriptItem::Assistant {
                text: String::new(),
                streaming: true,
            });
        }
    }

    fn push_assistant_delta(&mut self, delta: &str) {
        self.ensure_streaming_assistant();
        if let Some(TranscriptItem::Assistant { text, .. }) = self.transcript.last_mut() {
            text.push_str(delta);
        }
    }

    fn finish_streaming_assistant(&mut self) {
        if let Some(TranscriptItem::Assistant { streaming, .. }) = self.transcript.last_mut() {
            *streaming = false;
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    Submit(String),
    Command(String),
    Cancel,
    Quit,
    Redraw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptItem {
    User(String),
    Assistant {
        text: String,
        streaming: bool,
    },
    Tool {
        call_id: String,
        name: String,
        state: ToolState,
    },
    Status(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done(String),
    Error(String),
}

fn previous_char_boundary(text: &str, cursor: usize) -> Option<usize> {
    text.char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index < cursor)
        .last()
}

fn next_char_boundary(text: &str, cursor: usize) -> Option<usize> {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .find(|index| *index > cursor)
}
