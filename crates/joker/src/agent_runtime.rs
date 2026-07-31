use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::{
    Agent, ApprovalResponse, Event, Observer, RunError, RunOutcome, RunRequest, StopReason,
    agent::RunGuard,
    message_queue::{DrainMode, PendingMessageQueue},
};

/// Commands that can be sent to an active [`AgentRuntime`] run via the Op channel.
///
/// Mirrors the OUTLINE.md design (reference: codex SQ/EQ pattern):
/// - [`Op::SendMessage`] → inject a user message mid-run
/// - [`Op::Cancel`] → abort the current run
/// - [`Op::Interrupt`] → inject an interrupt/steer message
/// - [`Op::Approve`] → resolve a pending approval request
/// - [`Op::Compact`] → trigger manual compaction
/// - [`Op::SwitchAgent`] → switch agent profile
/// - [`Op::Shutdown`] → graceful shutdown
#[derive(Clone, Debug)]
pub enum Op {
    /// Inject a user message that will be processed on the next turn.
    SendMessage {
        /// The message content to inject.
        text: String,
    },
    /// Cancel the current run via the cancellation token.
    Cancel,
    /// Inject an interrupt message — processed before the model's next response.
    Interrupt {
        /// The interrupt message content.
        text: String,
    },
    /// Resolve a pending approval request (reference: codex SQ/EQ approval channel).
    Approve {
        /// `true` to approve, `false` to deny.
        approved: bool,
        /// If `true`, remember the decision for the session (no more prompts for this tool).
        remember_for_session: bool,
        /// Optional reason for denial.
        reason: Option<String>,
    },
    /// Trigger manual context compaction.
    Compact,
    /// Switch to a different agent profile mid-run.
    SwitchAgent {
        /// Name of the target agent profile.
        name: String,
    },
    /// Signal the runtime to shut down gracefully.
    Shutdown,
}

/// An agent execution runtime with OpLoop + steer/followUp support.
pub struct AgentRuntime {
    agent: Agent,
    steering_queue: PendingMessageQueue,
    follow_up_queue: PendingMessageQueue,
}

impl AgentRuntime {
    /// Wrap an [`Agent`] in a runtime with empty steer and follow-up queues.
    #[must_use]
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            steering_queue: PendingMessageQueue::new(),
            follow_up_queue: PendingMessageQueue::new(),
        }
    }

    /// Queue a steering message — injected before the next turn.
    pub fn steer(&self, message: impl Into<String>) {
        self.steering_queue.enqueue(message);
    }

    /// Queue a follow-up message — injected after the tool-call loop ends.
    pub fn follow_up(&self, message: impl Into<String>) {
        self.follow_up_queue.enqueue(message);
    }

    /// Drive a full agent run with Op-loop support (steer, cancel, compact, etc.).
    ///
    /// Reads [`Op`]s from `rx_op` before each turn, injects steering messages,
    /// and after the inner tool-call loop injects follow-up messages for the
    /// outer follow-up loop.
    pub async fn run(
        &self,
        mut request: RunRequest,
        rx_op: &mut mpsc::UnboundedReceiver<Op>,
    ) -> Result<RunOutcome, RunError> {
        if self.agent.run_state.swap(true, Ordering::Acquire) {
            return Err(RunError::Busy);
        }
        let _guard = RunGuard(&self.agent.run_state);

        let cancellation_token = request.cancellation_token.clone().unwrap_or_default();
        observe_event(&self.agent.observer, Event::RunStarted).await;

        let _start = Instant::now();
        tracing::info!(
            target: "agent",
            input_len = request.input.as_ref().map_or(0, |s| s.len()),
            "run started"
        );

        let turn_id = format!("turn-{}", now_millis());
        let model_id = "unknown".to_string();
        let mut stop_reason = StopReason::Stop;

        let result = async {
            if request.conversation.messages().is_empty()
                && let Some(input) = request.input.take()
            {
                request.conversation.push(crate::Message::user(input));
            }

            let mut steps = 0usize;
            let mut tool_calls = 0usize;

            let final_stop_reason = loop {
                tracing::debug!(target: "agent", steps, tool_calls, "turn iteration");
                let turn_stop_reason = loop {
                    while let Ok(op) = rx_op.try_recv() {
                        match op {
                            Op::SendMessage { text } => {
                                tracing::info!(target: "agent", msg_preview = %text.chars().take(80).collect::<String>(), "op: send_message");
                                request.conversation.push(crate::Message::user(text));
                            }
                            Op::Cancel => {
                                tracing::info!(target: "agent", "op: cancel");
                                cancellation_token.cancel();
                            }
                            Op::Interrupt { text } => {
                                tracing::info!(target: "agent", msg_preview = %text.chars().take(80).collect::<String>(), "op: interrupt");
                                self.steering_queue.enqueue(text);
                            }
                            Op::Approve { approved, remember_for_session, reason } => {
                                tracing::info!(target: "agent", approved, remember_for_session, "op: approve");
                                if let Some(channel) = &self.agent.approval_channel {
                                    if approved {
                                        channel.respond(ApprovalResponse::Approved { remember_for_session });
                                    } else {
                                        channel.respond(ApprovalResponse::Denied {
                                            reason: reason.unwrap_or_else(|| "denied by user".into()),
                                        });
                                    }
                                }
                            }
                            Op::Compact => {
                                tracing::info!(target: "agent", "op: compact");
                                observe_event(&self.agent.observer, Event::CompactionStarted {
                                    trigger: "manual".into(), current_tokens: 0, threshold: 0,
                                }).await;
                                observe_event(&self.agent.observer, Event::CompactionDone {
                                    tokens_before: 0, tokens_after: 0,
                                }).await;
                            }
                            Op::SwitchAgent { name } => {
                                tracing::info!(target: "agent", name = %name, "op: switch_agent");
                                observe_event(&self.agent.observer, Event::AgentSwitched {
                                    from: String::new(), to: name,
                                }).await;
                            }
                            Op::Shutdown => {
                                tracing::info!(target: "agent", "op: shutdown");
                                return Err(RunError::Shutdown);
                            }
                        }
                    }

                    let steer_msgs = self.steering_queue.drain(DrainMode::All);
                    for msg in steer_msgs {
                        request.conversation.push(crate::Message::user(msg));
                    }

                    if cancellation_token.is_cancelled() {
                        return Err(RunError::Cancelled);
                    }
                    if steps >= self.agent.config.limits.max_steps {
                        observe_limit(&self.agent.observer, "max_steps").await;
                        return Ok(RunOutcome { conversation: request.conversation, stop_reason: StopReason::LimitReached });
                    }
                    steps += 1;

                    let outcome = self.agent
                        .run_turn(&mut request.conversation, &turn_id, &model_id, &cancellation_token)
                        .await?;

                    if !outcome.has_tool_calls {
                        break outcome.stop_reason;
                    }

                    if tool_calls + outcome.tool_calls_count > self.agent.config.limits.max_tool_calls {
                        observe_limit(&self.agent.observer, "max_tool_calls").await;
                        return Ok(RunOutcome { conversation: request.conversation, stop_reason: StopReason::LimitReached });
                    }
                    tool_calls += outcome.tool_calls_count;

                    let results = self.agent
                        .execute_tool_calls(outcome.pending_tool_calls, &cancellation_token)
                        .await?;
                    request.conversation.push(crate::Message::tool(results));
                };

                let follow_ups = self.follow_up_queue.drain(DrainMode::All);
                if follow_ups.is_empty() {
                    break turn_stop_reason;
                }
                for msg in follow_ups {
                    request.conversation.push(crate::Message::user(msg));
                }
            };

            Ok(RunOutcome {
                conversation: request.conversation,
                stop_reason: final_stop_reason,
            })
        }.await;

        match &result {
            Ok(outcome) => stop_reason = outcome.stop_reason,
            Err(RunError::Cancelled) => stop_reason = StopReason::Cancelled,
            Err(e) => tracing::error!(target: "agent", ?e, "run failed"),
        }
        let duration_ms = _start.elapsed().as_millis() as u64;
        tracing::info!(target: "agent", ?stop_reason, duration_ms, "run finished");
        observe_event(&self.agent.observer, Event::RunFinished { stop_reason }).await;
        result
    }
}

async fn observe_event(observer: &Arc<dyn Observer>, event: Event) {
    tracing::trace!(target: "event", ?event);
    let _ = observer.observe(event).await;
}

async fn observe_limit(observer: &Arc<dyn Observer>, reason: impl Into<String>) {
    let reason = reason.into();
    tracing::warn!(target: "agent", %reason, "limit reached");
    observe_event(observer, Event::LimitReached { reason }).await;
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
