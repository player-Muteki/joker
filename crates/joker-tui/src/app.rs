//! Application state machine for the TUI.
//!
//! Defines [`App`], the central state struct that holds the composer buffer,
//! transcript, dialog stack, and all runtime configuration.  Key events flow
//! through [`App::handle_key`] which returns [`AppAction`] values; the driver
//! loop in [`crate::terminal::run_tui`] dispatches those actions and feeds
//! [`UiEvent`]s back through [`App::apply_ui_event`].

use std::sync::Arc;
use std::fmt;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use joker::SharedApprovalChannel;
use joker_config::RuntimeConfig;
use tokio_util::sync::CancellationToken;

use crate::event::UiEvent;

/// Central application state for the Joker TUI.
#[derive(Clone)]
pub struct App {
    /// Current text in the prompt composer line.
    pub composer: String,
    /// Byte offset of the cursor within [`composer`](Self::composer).
    pub cursor: usize,
    /// Ordered transcript of user messages, assistant replies, tool calls, and status lines.
    pub transcript: Vec<TranscriptItem>,
    /// Whether an agent run is currently in progress.
    pub running: bool,
    /// Set to `true` by [`quit`](Self::quit) to signal the event loop to exit.
    pub should_quit: bool,
    /// Status-line text shown in the TUI header bar.
    pub status: String,
    /// Scroll offset for the transcript viewport.
    pub scroll: u16,
    /// Token used to cancel the current agent run.
    pub cancellation_token: Option<CancellationToken>,
    /// Provider/model configuration for the current session.
    pub runtime_config: RuntimeConfig,
    /// Active modal dialog, if any.
    pub dialog: Option<Dialog>,
    /// Shared approval channel for the current run
    pub approval_channel: Option<SharedApprovalChannel>,
    /// Persistent session store (e.g. JSON-lines file).
    pub session_store: Option<Arc<dyn joker::SessionStore>>,
    /// A previously-saved conversation loaded for continuation.
    pub loaded_conversation: Option<joker::Conversation>,
    /// Flag to request context compaction on the next agent run.
    pub compact_requested: bool,
    /// Persistent credential store for API keys.
    pub credential_store: joker::CredentialStore,
    /// Active API-key input overlay state `(provider_id, buffer)`.
    pub api_key_input: Option<(String, String)>,
    /// Models discovered for the current provider.
    pub available_models: Vec<String>,
    /// Agent management
    pub active_agent: String,
    /// Names of all registered agent profiles.
    pub agent_names: Vec<String>,
    /// Wizard state for the multi-step agent creation flow.
    pub agent_new_state: Option<AgentNewState>,
}

/// A modal selection dialog rendered over the main TUI.
#[derive(Clone, Debug)]
pub struct Dialog {
    /// Discriminant determining dialog behaviour.
    pub kind: DialogKind,
    /// Title text shown in the dialog border.
    pub title: String,
    /// Pairs of `(display_label, value)` for each selectable option.
    pub options: Vec<(String, String)>,
    /// Index of the currently highlighted option.
    pub selected: usize,
}

impl Dialog {
    /// Return the `value` of the currently selected option.
    pub fn selected_value(&self) -> String {
        self.options
            .get(self.selected)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

/// Discriminant for the kind of modal dialog to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// Provider-selection dialog.
    Provider,
    /// Model-selection dialog.
    Model,
    /// API-key input overlay for a specific provider.
    ApiKeyInput {
        /// Provider ID for which the key is being entered.
        provider_id: String,
    },
    /// Agent-switching selection dialog.
    AgentSwitch,
    /// Multi-step wizard for creating a new agent.
    AgentNew {
        /// Current step index in the wizard (0 = name, 1+ = permissions).
        step: usize,
    },
}

/// State machine for the multi-step agent creation wizard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentNewState {
    /// Name chosen for the new agent.
    pub agent_name: String,
    /// Raw input buffer for the current wizard step.
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
            .field("loaded_conversation", &self.loaded_conversation.as_ref().map(|c| c.messages().len()))
            .field("compact_requested", &self.compact_requested)
            .field("credential_store", &self.credential_store.list())
            .field("api_key_input", &self.api_key_input.as_ref().map(|(p, _)| p.as_str()))
            .field("available_models", &self.available_models)
            .field("active_agent", &self.active_agent)
            .field("agent_names", &self.agent_names)
            .finish()
    }
}

impl App {
    /// Create an `App` with default [`RuntimeConfig`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(RuntimeConfig::default())
    }

    /// Create an `App` with a specific [`RuntimeConfig`].
    #[must_use]
    pub fn with_config(runtime_config: RuntimeConfig) -> Self {
        let status = format!("Idle ({})", runtime_config.provider_label());
        let agent_names = vec!["plan".into(), "build".into(), "yolo".into()];
        let available_models = runtime_config.available_models();
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
            loaded_conversation: None,
            compact_requested: false,
            credential_store: joker::CredentialStore::new(),
            api_key_input: None,
            available_models,
            active_agent: "build".into(),
            agent_names,
            agent_new_state: None,
        }
    }

    /// Process a `KeyEvent` and return an optional [`AppAction`].
    ///
    /// Delegates to the active dialog, approval-request, or the default
    /// keybindings for navigation, submission, cancellation, and quitting.
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
        if self.running
            && let Some(request_id) = self.pending_approval_request_id()
        {
            return match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.approve_pending(&request_id, false);
                    Some(AppAction::Redraw)
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.approve_pending(&request_id, true);
                    Some(AppAction::Redraw)
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.deny_pending(&request_id, None);
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

    /// Submit the current composer text.
    ///
    /// If the text starts with `/` it is treated as a slash command;
    /// otherwise it starts a new agent run.
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

    /// Cancel the currently running agent via its [`CancellationToken`].
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

    /// Cancel any running agent and set [`should_quit`](Self::should_quit).
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

    /// Approve a pending tool-approval request by `request_id`.
    pub fn approve_pending(&mut self, request_id: &str, remember_for_session: bool) {
        if let Some(channel) = &self.approval_channel {
            channel.respond(joker::ApprovalResponse::Approved {
                remember_for_session,
            });
        }
        self.transcript
            .push(TranscriptItem::Status(format!("Approved: {request_id}")));
    }

    /// Deny a pending tool-approval request by `request_id`, optionally providing a `reason`.
    pub fn deny_pending(&mut self, request_id: &str, reason: Option<&str>) {
        if let Some(channel) = &self.approval_channel {
            channel.respond(joker::ApprovalResponse::Denied {
                reason: reason.unwrap_or("denied by user").to_string(),
            });
        }
        self.transcript
            .push(TranscriptItem::Status(format!("Denied: {request_id}")));
    }

    fn pending_approval_request_id(&self) -> Option<String> {
        self.transcript.iter().rev().find_map(|item| {
            if let TranscriptItem::ApprovalRequest { request_id, .. } = item {
                Some(request_id.clone())
            } else {
                None
            }
        })
    }

    /// Apply an incoming [`UiEvent`] to the application state.
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
            UiEvent::ModelDiscoveryCompleted(result) => match result {
                Ok(models) => {
                    if !models.is_empty() {
                        self.available_models = models;
                    }
                    self.transcript.push(TranscriptItem::Status(format!(
                        "Discovered {} model(s).",
                        self.available_models.len()
                    )));
                }
                Err(error) => {
                    self.transcript.push(TranscriptItem::Status(format!(
                        "Model discovery failed: {error}"
                    )));
                }
            },
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
            let agent_name = self.active_agent.clone();
            let data = joker::SessionData {
                id: id.clone(),
                label,
                created_at: now,
                updated_at: now,
                model,
                agent_name,
                parent_id: None,
                root_id: id,
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

/// Action emitted by [`App`] for the driver loop to dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    /// Submit a user prompt for an agent run.
    Submit(String),
    /// Execute a slash command.
    Command(String),
    /// Cancel the current agent run.
    Cancel,
    /// Quit the application entirely.
    Quit,
    /// Request a re-draw of the TUI (no side effects).
    Redraw,
    /// Confirmed a dialog selection.
    DialogConfirm {
        /// Which dialog kind was confirmed.
        kind: DialogKind,
        /// The selected value.
        selection: String,
    },
    /// API key entered via the input overlay.
    ApiKeyConfirm {
        /// Target provider ID.
        provider_id: String,
        /// The raw API key value.
        api_key: String,
    },
    /// Create a new agent from the wizard.
    AgentCreate {
        /// Name for the new agent.
        name: String,
        /// Pairs of `(display_label, permission_setting)` for each tool.
        tool_permissions: Vec<(String, String)>,
    },
}

/// An entry in the conversation transcript rendered in the TUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptItem {
    /// A user-submitted message.
    User(String),
    /// An assistant (model) response, possibly still streaming.
    Assistant {
        /// Accumulated text so far.
        text: String,
        /// Whether the model is still producing output.
        streaming: bool,
    },
    /// A tool invocation and its outcome.
    Tool {
        /// Unique tool-call identifier.
        call_id: String,
        /// Human-readable tool name.
        name: String,
        /// Current execution state of the tool.
        state: ToolState,
    },
    /// A pending approval request awaiting user response.
    ApprovalRequest {
        /// Identifier for this request.
        request_id: String,
        /// Name of the tool requesting approval.
        tool_name: String,
        /// Subject or context for the approval.
        subject: String,
        /// Explanation of why approval is needed.
        reason: String,
    },
    /// A status or informational message.
    Status(String),
    /// An error message.
    Error(String),
}

/// Execution state of a tool call in the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// Tool is currently executing.
    Running,
    /// Tool has produced partial output.
    Progress(String),
    /// Tool completed successfully with the given output.
    Done(String),
    /// Tool failed with the given error message.
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
