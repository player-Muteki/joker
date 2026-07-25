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
    pub credential_store: joker::CredentialStore,
    pub api_key_input: Option<(String, String)>,
    /// Agent management
    pub active_agent: String,
    pub agent_names: Vec<String>,
    pub agent_new_state: Option<AgentNewState>,
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
    ApiKeyInput { provider_id: String },
    AgentSwitch,
    AgentNew { step: usize },
}

/// State machine for the multi-step agent creation wizard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentNewState {
    pub agent_name: String,
    pub step_input: String,
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
            .field("credential_store", &self.credential_store.list())
            .field("api_key_input", &self.api_key_input.as_ref().map(|(p, _)| p.as_str()))
            .field("active_agent", &self.active_agent)
            .field("agent_names", &self.agent_names)
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
        let agent_names = vec!["plan".into(), "build".into(), "yolo".into()];
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
            credential_store: joker::CredentialStore::new(),
            api_key_input: None,
            active_agent: "build".into(),
            agent_names,
            agent_new_state: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        // API key input mode: keys go to the input buffer
        if let Some((ref _provider_id, ref mut buffer)) = self.api_key_input {
            return match key.code {
                KeyCode::Enter => {
                    let key_value = buffer.clone();
                    let provider = self.api_key_input.take().unwrap().0;
                    Some(AppAction::ApiKeyConfirm { provider_id: provider, api_key: key_value })
                }
                KeyCode::Esc => {
                    self.api_key_input = None;
                    Some(AppAction::Redraw)
                }
                KeyCode::Char(ch) => {
                    buffer.push(ch);
                    Some(AppAction::Redraw)
                }
                KeyCode::Backspace => {
                    buffer.pop();
                    Some(AppAction::Redraw)
                }
                _ => Some(AppAction::Redraw),
            };
        }
        if self.dialog.is_some() {
            // AgentNew wizard: custom input handling (needs mutable self)
            if matches!(
                self.dialog.as_ref().unwrap().kind,
                DialogKind::AgentNew { .. }
            ) {
                return self.handle_agent_new_dialog_key(key);
            }
            // AgentSwitch and other selection dialogs
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
            KeyCode::Tab => self.tab_complete(),
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

    fn handle_agent_new_dialog_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        let dialog = self.dialog.as_mut()?;
        let DialogKind::AgentNew { step } = dialog.kind else {
            return None;
        };
        let state = self.agent_new_state.get_or_insert_with(AgentNewState::default);

        // All tools available for permission assignment
        let tool_names: Vec<&str> = vec![
            "list_files", "read_file", "grep", "glob",
            "write_file", "edit_file", "apply_patch", "shell",
            "todo_write",
            "web_search", "fetch_url", "memory_read", "memory_write",
        ];

        match step {
            0 => {
                // Step 0: enter agent name (free text)
                match key.code {
                    KeyCode::Enter => {
                        let name = state.step_input.trim().to_string();
                        if name.is_empty() {
                            return Some(AppAction::Redraw);
                        }
                        state.agent_name = name.clone();
                        state.step_input.clear();
                        dialog.kind = DialogKind::AgentNew { step: 1 };
                        dialog.title = format!("Agent '{name}' — Tool Permissions");
                        // Build options: tool names with default Ask
                        dialog.options = tool_names
                            .iter()
                            .map(|t| (format!("[?] {t}"), t.to_string()))
                            .collect();
                        dialog.selected = 0;
                        Some(AppAction::Redraw)
                    }
                    KeyCode::Esc => {
                        self.dialog = None;
                        self.agent_new_state = None;
                        Some(AppAction::Redraw)
                    }
                    KeyCode::Char(ch) => {
                        state.step_input.push(ch);
                        dialog.title = format!("New Agent — Name: {}", state.step_input);
                        Some(AppAction::Redraw)
                    }
                    KeyCode::Backspace => {
                        state.step_input.pop();
                        dialog.title = format!("New Agent — Name: {}", state.step_input);
                        Some(AppAction::Redraw)
                    }
                    _ => Some(AppAction::Redraw),
                }
            }
            _ => {
                // Steps 1+: set permission for the selected tool
                let idx = dialog.selected;
                match key.code {
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        if let Some((label, _)) = dialog.options.get_mut(idx) {
                            *label = format!("[Ask] {}", tool_names[idx]);
                        }
                        Some(AppAction::Redraw)
                    }
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if let Some((label, _)) = dialog.options.get_mut(idx) {
                            *label = format!("[Auto] {}", tool_names[idx]);
                        }
                        Some(AppAction::Redraw)
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        if let Some((label, _)) = dialog.options.get_mut(idx) {
                            *label = format!("[Off] {}", tool_names[idx]);
                        }
                        Some(AppAction::Redraw)
                    }
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
                        // Confirm: build agent from wizard selections and close dialog
                        let name = state.agent_name.clone();
                        let permissions = dialog.options.clone();
                        self.dialog = None;
                        self.agent_new_state = None;
                        Some(AppAction::AgentCreate {
                            name,
                            tool_permissions: permissions,
                        })
                    }
                    KeyCode::Esc => {
                        self.dialog = None;
                        self.agent_new_state = None;
                        Some(AppAction::Redraw)
                    }
                    _ => Some(AppAction::Redraw),
                }
            }
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

    fn tab_complete(&mut self) -> Option<AppAction> {
        if !self.composer.starts_with('/') {
            return None;
        }
        // Extract the command name (first word after '/')
        let after_slash = &self.composer[1..];
        let cmd_end = after_slash
            .find(char::is_whitespace)
            .unwrap_or(after_slash.len());
        let _current_cmd = &after_slash[..cmd_end];

        let suggestions = crate::commands::suggestions(self.composer.as_str());
        if suggestions.is_empty() {
            return None;
        }

        // Cycle through completions: if current text already matches the first
        // suggestion's full command name, advance to the next one.
        let full_cmd = format!("/{}", suggestions[0].name);
        if self.composer.len() <= full_cmd.len()
            && full_cmd.starts_with(self.composer.as_str())
            && self.composer == full_cmd
        {
            // Already completed to the first match — cycle to next
            if suggestions.len() > 1 {
                let next = format!("/{}", suggestions[1].name);
                self.composer = next;
                self.cursor = self.composer.len();
            }
        } else {
            self.composer = full_cmd;
            self.cursor = self.composer.len();
        }

        Some(AppAction::Redraw)
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

    /// Set the file path for persistent credential storage and load existing creds.
    pub fn set_credential_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.credential_store = joker::CredentialStore::with_file(path);
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
                    Ok(outcome) => {
                        self.status = format!("Idle ({})", self.runtime_config.provider_label());
                        self.save_current_session(outcome);
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

    fn save_current_session(&self, outcome: joker::RunOutcome) {
        if let Some(ref store) = self.session_store {
            let store = store.clone();
            let model = self.runtime_config.current_model();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let label = outcome
                .conversation
                .messages()
                .iter()
                .find(|m| m.role == joker::Role::User)
                .and_then(|m| m.content.first())
                .map(|c| match c {
                    joker::Content::Text(t) => {
                        let truncated: String = t.text.chars().take(80).collect();
                        if t.text.len() > 80 {
                            format!("{truncated}...")
                        } else {
                            truncated
                        }
                    }
                    _ => "conversation".to_string(),
                })
                .unwrap_or_else(|| "conversation".to_string());

            let id = format!("{}-{:04x}", now, rand_u16());
            let data = joker::SessionData {
                id,
                label,
                created_at: now,
                updated_at: now,
                model,
                conversation: outcome.conversation,
            };

            let store_clone = store.clone();
            tokio::spawn(async move {
                let _ = store_clone.save(data).await;
            });
        }
    }

    fn apply_agent_event(&mut self, event: joker::Event) {
        match event {
            // ── Lifecycle ─────────────────────────────────────────
            joker::Event::RunStarted => {
                self.status = "Running".into();
            }
            joker::Event::RunFinished { .. } => {
                self.status = "Finishing".into();
            }

            // ── Turn boundaries ────────────────────────────────────
            joker::Event::TurnStarted { .. } => {
                self.status = "Turn starting".into();
            }
            joker::Event::TurnDone { .. } => {
                self.status = "Turn complete".into();
            }

            // ── Model output ───────────────────────────────────────
            joker::Event::ModelStarted => {
                self.status = "Model streaming".into();
                self.ensure_streaming_assistant();
            }
            #[allow(deprecated)]
            joker::Event::ModelDelta { delta } => {
                self.push_assistant_delta(&delta);
            }
            joker::Event::TextDelta { delta } => {
                self.push_assistant_delta(&delta);
            }
            joker::Event::ReasoningDelta { delta } => {
                // Currently append to assistant text; can separate later
                self.push_assistant_delta(&delta);
            }
            joker::Event::ModelFinished { .. } => {
                self.finish_streaming_assistant();
            }

            // ── Tool lifecycle ─────────────────────────────────────
            joker::Event::ToolDispatch {
                call_id,
                tool_name,
                ..
            } => {
                self.status = format!("Tool {tool_name} dispatched");
                self.transcript.push(TranscriptItem::Tool {
                    call_id,
                    name: tool_name,
                    state: ToolState::Running,
                });
            }
            joker::Event::ToolStarted { call_id, name } => {
                self.status = format!("Tool {name} running");
                // Only add a transcript item if ToolDispatch wasn't emitted
                if !self.transcript.iter().any(|item| {
                    matches!(item, TranscriptItem::Tool { call_id: id, .. } if id == &call_id)
                }) {
                    self.transcript.push(TranscriptItem::Tool {
                        call_id,
                        name,
                        state: ToolState::Running,
                    });
                }
            }
            joker::Event::ToolDelta { .. } => {}
            joker::Event::ToolProgress {
                call_id,
                partial_output,
            } => {
                if let Some(item) = self.transcript.iter_mut().rev().find(|item| {
                    matches!(item, TranscriptItem::Tool { call_id: id, .. } if id == &call_id)
                }) {
                    *item = TranscriptItem::Tool {
                        call_id,
                        name: String::new(),
                        state: ToolState::Progress(partial_output),
                    };
                }
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

            // ── Token usage ────────────────────────────────────────
            joker::Event::Usage {
                input_tokens,
                output_tokens,
                cache_hit_tokens,
            } => {
                self.status = format!("Tokens: {input_tokens}↑ {output_tokens}↓ cache:{cache_hit_tokens}");
                self.transcript.push(TranscriptItem::Status(format!(
                    "Usage: {input_tokens} in / {output_tokens} out (cache hit: {cache_hit_tokens})"
                )));
            }

            // ── Compaction ─────────────────────────────────────────
            joker::Event::CompactionStarted {
                trigger,
                current_tokens,
                threshold,
            } => {
                self.transcript.push(TranscriptItem::Status(format!(
                    "Compaction triggered: {trigger} ({current_tokens}/{threshold} tokens)"
                )));
            }
            joker::Event::CompactionDone {
                tokens_before,
                tokens_after,
            } => {
                let saved = tokens_before.saturating_sub(tokens_after);
                self.transcript.push(TranscriptItem::Status(format!(
                    "Compaction done: {tokens_before} → {tokens_after} (saved {saved} tokens)"
                )));
            }

            // ── Agent / model switching ────────────────────────────
            joker::Event::AgentSwitched { from, to } => {
                self.active_agent = to.clone();
                self.transcript.push(TranscriptItem::Status(format!(
                    "Switched agent: {from} → {to}"
                )));
            }
            joker::Event::ModelSwitched { from, to } => {
                self.transcript.push(TranscriptItem::Status(format!(
                    "Switched model: {from} → {to}"
                )));
            }

            // ── Limits ─────────────────────────────────────────────
            joker::Event::LimitReached { reason } => {
                self.transcript
                    .push(TranscriptItem::Status(format!("Limit reached: {reason}")));
            }

            // ── Permission ─────────────────────────────────────────
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

            // ── Error / retry ──────────────────────────────────────
            joker::Event::Error {
                kind,
                message,
                recoverable,
            } => {
                let level = if recoverable { "Warning" } else { "Error" };
                self.transcript
                    .push(TranscriptItem::Status(format!("{level} [{kind}]: {message}")));
            }
            joker::Event::Retrying {
                attempt,
                max_attempts,
                reason,
            } => {
                self.transcript.push(TranscriptItem::Status(format!(
                    "Retry {attempt}/{max_attempts}: {reason}"
                )));
            }
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
    ApiKeyConfirm {
        provider_id: String,
        api_key: String,
    },
    AgentCreate {
        name: String,
        tool_permissions: Vec<(String, String)>,
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
    Progress(String),
    Done(String),
    Error(String),
}

fn rand_u16() -> u16 {
    // Simple xorshift for non-crypto randomness (session IDs)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seed = (now.as_nanos() ^ (std::process::id() as u128)) as u64;
    let mut x = seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x & 0xFFFF) as u16
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
