use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::{
    Agent, Event, Observer, RunError, RunOutcome, RunRequest, StopReason,
    message_queue::{DrainMode, PendingMessageQueue},
    agent::RunGuard,
};

/// Commands that can be sent to an active `AgentRuntime` run.
#[derive(Clone, Debug)]
pub enum Op {
    Cancel,
    Compact,
    SwitchAgent { name: String },
    Shutdown,
}

/// An agent execution runtime with OpLoop + steer/followUp support.
pub struct AgentRuntime {
    agent: Agent,
    steering_queue: PendingMessageQueue,
    follow_up_queue: PendingMessageQueue,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            steering_queue: PendingMessageQueue::new(),
            follow_up_queue: PendingMessageQueue::new(),
        }
    }

    pub fn steer(&self, message: impl Into<String>) {
        self.steering_queue.enqueue(message);
    }

    pub fn follow_up(&self, message: impl Into<String>) {
        self.follow_up_queue.enqueue(message);
    }

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

            loop {
                loop {
                    while let Ok(op) = rx_op.try_recv() {
                        match op {
                            Op::Cancel => cancellation_token.cancel(),
                            Op::Compact => {
                                observe_event(&self.agent.observer, Event::CompactionStarted {
                                    trigger: "manual".into(), current_tokens: 0, threshold: 0,
                                }).await;
                                observe_event(&self.agent.observer, Event::CompactionDone {
                                    tokens_before: 0, tokens_after: 0,
                                }).await;
                            }
                            Op::SwitchAgent { name } => {
                                observe_event(&self.agent.observer, Event::AgentSwitched {
                                    from: String::new(), to: name,
                                }).await;
                            }
                            Op::Shutdown => return Err(RunError::Shutdown),
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
                        break;
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
                }

                let follow_ups = self.follow_up_queue.drain(DrainMode::All);
                if follow_ups.is_empty() {
                    break;
                }
                for msg in follow_ups {
                    request.conversation.push(crate::Message::user(msg));
                }
            }

            Ok(RunOutcome { conversation: request.conversation, stop_reason: StopReason::Stop })
        }.await;

        if let Ok(outcome) = &result {
            stop_reason = outcome.stop_reason;
        } else if matches!(result, Err(RunError::Cancelled)) {
            stop_reason = StopReason::Cancelled;
        }
        observe_event(&self.agent.observer, Event::RunFinished { stop_reason }).await;
        result
    }
}

async fn observe_event(observer: &Arc<dyn Observer>, event: Event) {
    let _ = observer.observe(event).await;
}

async fn observe_limit(observer: &Arc<dyn Observer>, reason: impl Into<String>) {
    observe_event(observer, Event::LimitReached { reason: reason.into() }).await;
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
