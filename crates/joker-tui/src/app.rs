use std::sync::Arc;
use std::fmt;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use joker::SharedApprovalChannel;
use joker_config::RuntimeConfig;
use tokio_util::sync::CancellationToken;

use crate::event::UiEvent;

#[derive(Clone)]
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
    pub dialog: Option<Dialog>,
    /// Shared approval channel for the current run
    pub approval_channel: Option<SharedApprovalChannel>,
    pub session_store: Option<Arc<dyn joker::SessionStore>>,
    pub compact_requested: bool,
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    pub title: String,
    pub options: Vec<(String, String)>,
    pub selected: usize,
}

impl Dialog {
    pub fn selected_value(&self) -> String {
        self.options
            .get(self.selected)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Provider,
    Model,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("composer", &self.composer)
            .field("cursor", &self.cursor)
            .field("transcript", &self.transcript)
            .field("running", &self.running)
            .field("should_quit", &self.should_quit)
            .field("status", &self.status)
            .field("scroll", &self.scroll)
            .field("runtime_config", &self.runtime_config)
            .field("dialog", &self.dialog)
            .field("approval_channel", &self.approval_channel)
            .field("session_store", &self.session_store.as_ref().map(|_| "SessionStore"))
            .field("compact_requested", &self.compact_requested)
            .finish()
    }
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
            dialog: None,
            approval_channel: None,
            session_store: None,
            compact_requested: false,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        if self.dialog.is_some() {
            let dialog = self.dialog.as_mut().unwrap();
            return match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    dialog.selected = dialog.selected.saturating_sub(1);
                    Some(AppAction::Redraw)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if dialog.selected + 1 < dialog.options.len() {
                        dialog.selected += 1;
                    }
                    Some(AppAction::Redraw)
                }
                KeyCode::Enter => {
                    let kind = dialog.kind.clone();
                    let selection = dialog.selected_value();
                    self.dialog = None;
                    Some(AppAction::DialogConfirm { kind, selection })
                }
                KeyCode::Esc => {
                    self.dialog = None;
                    Some(AppAction::Redraw)
                }
                _ => None,
            };
        }
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

    /// Approve the pending tool with the given request_id.
    pub fn approve_pending(&mut self, request_id: &str, remember_for_session: bool) {
        if let Some(channel) = &self.approval_channel {
            channel.respond(joker::ApprovalResponse::Approved {
                remember_for_session,
            });
        }
        self.transcript
            .push(TranscriptItem::Status(format!("Approved: {request_id}")));
    }

    /// Deny the pending tool with the given request_id.
    pub fn deny_pending(&mut self, request_id: &str, reason: Option<&str>) {
        if let Some(channel) = &self.approval_channel {
            channel.respond(joker::ApprovalResponse::Denied {
                reason: reason.unwrap_or("denied by user").to_string(),
            });
        }
        self.transcript
            .push(TranscriptItem::Status(format!("Denied: {request_id}")));
    }

    pub fn apply_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Agent(event) => self.apply_agent_event(event),
            UiEvent::RunCompleted(result) => {
                self.running = false;
                self.cancellation_token = None;
                self.approval_channel = None;
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
            joker::Event::PermissionRequested {
                request_id,
                tool_name,
                subject,
                reason,
            } => {
                self.status = format!("Approval needed: {tool_name}");
                self.transcript.push(TranscriptItem::ApprovalRequest {
                    request_id,
                    tool_name,
                    subject,
                    reason,
                });
            }
            joker::Event::PermissionResolved {
                request_id,
                approved,
                reason: _,
            } => {
                if approved {
                    self.status = "Approved".into();
                } else {
                    self.status = "Denied".into();
                }
                // Update the corresponding approval request item
                if let Some(item) = self.transcript.iter_mut().rev().find(|item| {
                    matches!(item, TranscriptItem::ApprovalRequest { request_id: id, .. } if id == &request_id)
                }) {
                    *item = TranscriptItem::Status(if approved {
                        format!("✓ Approved: {request_id}")
                    } else {
                        format!("✗ Denied: {request_id}")
                    });
                }
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
    DialogConfirm {
        kind: DialogKind,
        selection: String,
    },
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
    ApprovalRequest {
        request_id: String,
        tool_name: String,
        subject: String,
        reason: String,
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
