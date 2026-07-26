#![allow(unreachable_pub)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use futures_util::{StreamExt, future::join_all};
use tokio_util::sync::CancellationToken;

use crate::{
    AllowAllPolicy, ApprovalRequest, ApprovalResponse, BuiltContext, Content, ContextBuilder,
    ContextInput, Event, Model, ModelError, ModelRequest, ModelResponseEvent,
    NoopObserver, Observer, PassthroughContextBuilder, RunError, SharedApprovalChannel,
    StopReason, TextContent, ToolCall, ToolDecision, ToolInvocation, ToolName, ToolPolicy,
    ToolPolicyRequest, ToolRegistry, ToolResult,
    agent_config::AgentConfig,
    agent_types::TurnOutcome,
};

pub struct Agent {
    pub(crate) model: Arc<dyn Model>,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) context_builder: Arc<dyn ContextBuilder>,
    pub(crate) policy: Arc<dyn ToolPolicy>,
    pub(crate) observer: Arc<dyn Observer>,
    pub(crate) config: AgentConfig,
    pub(crate) approval_channel: Option<SharedApprovalChannel>,
    pub(crate) run_state: AtomicBool,
}

pub(crate) struct RunGuard<'a>(pub(crate) &'a AtomicBool);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Agent {
    #[must_use]
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self {
            model,
            tools: Arc::new(ToolRegistry::new()),
            context_builder: Arc::new(PassthroughContextBuilder),
            policy: Arc::new(AllowAllPolicy),
            observer: Arc::new(NoopObserver),
            config: AgentConfig::default(),
            approval_channel: None,
            run_state: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn with_tools(mut self, tools: Arc<ToolRegistry>) -> Self { self.tools = tools; self }
    #[must_use]
    pub fn with_context_builder(mut self, context_builder: Arc<dyn ContextBuilder>) -> Self { self.context_builder = context_builder; self }
    #[must_use]
    pub fn with_policy(mut self, policy: Arc<dyn ToolPolicy>) -> Self { self.policy = policy; self }
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn Observer>) -> Self { self.observer = observer; self }
    #[must_use]
    pub fn with_approval_channel(mut self, channel: SharedApprovalChannel) -> Self { self.approval_channel = Some(channel); self }
    #[must_use]
    pub fn with_config(mut self, config: AgentConfig) -> Self { self.config = config; self }

    pub async fn run(&self, mut request: crate::RunRequest) -> Result<crate::RunOutcome, RunError> {
        if self.run_state.swap(true, Ordering::Acquire) {
            return Err(RunError::Busy);
        }
        let _guard = RunGuard(&self.run_state);

        let cancellation_token = request.cancellation_token.clone().unwrap_or_default();
        observe(&self.observer, Event::RunStarted).await;

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
                if cancellation_token.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                if steps >= self.config.limits.max_steps {
                    observe_limit(&self.observer, "max_steps").await;
                    return Ok(crate::RunOutcome { conversation: request.conversation, stop_reason: StopReason::LimitReached });
                }
                steps += 1;

                let outcome = self.run_turn(&mut request.conversation, &turn_id, &model_id, &cancellation_token).await?;

                if !outcome.has_tool_calls {
                    return Ok(crate::RunOutcome { conversation: request.conversation, stop_reason: outcome.stop_reason });
                }

                if tool_calls + outcome.pending_tool_calls.len() > self.config.limits.max_tool_calls {
                    observe_limit(&self.observer, "max_tool_calls").await;
                    return Ok(crate::RunOutcome { conversation: request.conversation, stop_reason: StopReason::LimitReached });
                }
                tool_calls += outcome.pending_tool_calls.len();

                let results = self.execute_tool_calls(outcome.pending_tool_calls, &cancellation_token).await?;
                request.conversation.push(crate::Message::tool(results));
            }
        }.await;

        if let Ok(outcome) = &result {
            stop_reason = outcome.stop_reason;
        } else if matches!(result, Err(RunError::Cancelled)) {
            stop_reason = StopReason::Cancelled;
        }
        observe(&self.observer, Event::RunFinished { stop_reason }).await;
        result
    }

    pub async fn run_turn(
        &self,
        conversation: &mut crate::Conversation,
        turn_id: &str,
        model_id: &str,
        cancellation_token: &CancellationToken,
    ) -> Result<TurnOutcome, RunError> {
        observe(&self.observer, Event::TurnStarted {
            session_id: String::new(),
            turn_id: turn_id.to_string(),
            agent_name: String::new(),
            model_id: model_id.to_string(),
        }).await;

        let BuiltContext { messages } = self.context_builder
            .build(ContextInput { conversation, limits: self.config.context_limits }).await?;

        observe(&self.observer, Event::ModelStarted).await;

        let model_output = {
            let cfg = &self.config.retry;
            let mut stream_errors = 0usize;
            let mut zero_outputs = 0usize;

            loop {
                let stream = match self.model.stream(ModelRequest {
                    messages: messages.clone(),
                    tools: self.tools.definitions(),
                }).await {
                    Ok(stream) => stream,
                    Err(ModelError::Stream(reason)) => {
                        stream_errors += 1;
                        if stream_errors > cfg.max_stream_retries {
                            return Err(RunError::Model(ModelError::Stream(reason)));
                        }
                        observe(&self.observer, Event::Retrying {
                            attempt: stream_errors,
                            max_attempts: cfg.max_stream_retries,
                            reason: format!("model stream error: {reason}"),
                        }).await;
                        tokio::time::sleep(Duration::from_millis(cfg.base_delay_ms * (1 << (stream_errors - 1)))).await;
                        continue;
                    }
                    Err(ModelError::Cancelled) => return Err(RunError::Cancelled),
                };

                let output = collect_model_output(stream, &self.observer, cancellation_token).await?;

                if output.content.is_empty() && output.tool_calls.is_empty() {
                    zero_outputs += 1;
                    if zero_outputs <= cfg.max_zero_output_retries {
                        observe(&self.observer, Event::Retrying {
                            attempt: zero_outputs,
                            max_attempts: cfg.max_zero_output_retries,
                            reason: "model returned empty response".into(),
                        }).await;
                        tokio::time::sleep(Duration::from_millis(cfg.base_delay_ms * (1 << (zero_outputs - 1)))).await;
                        continue;
                    }
                }

                break output;
            }
        };

        observe(&self.observer, Event::ModelFinished { stop_reason: model_output.stop_reason }).await;
        observe(&self.observer, Event::Usage {
            input_tokens: model_output.usage.input_tokens,
            output_tokens: model_output.usage.output_tokens,
            cache_hit_tokens: model_output.usage.cache_hit_tokens,
        }).await;
        observe(&self.observer, Event::TurnDone {
            turn_id: turn_id.to_string(),
            stop_reason: model_output.stop_reason,
        }).await;

        let assistant_message = crate::Message::assistant(model_output.content.clone());
        let pending_tool_calls = model_output.tool_calls;
        conversation.push(assistant_message);

        if pending_tool_calls.is_empty() {
            return Ok(TurnOutcome { stop_reason: model_output.stop_reason, has_tool_calls: false, tool_calls_count: 0, pending_tool_calls: Vec::new() });
        }

        Ok(TurnOutcome { stop_reason: model_output.stop_reason, has_tool_calls: true, tool_calls_count: pending_tool_calls.len(), pending_tool_calls })
    }

    pub(crate) async fn execute_tool_calls(
        &self,
        calls: Vec<ToolCall>,
        cancellation_token: &CancellationToken,
    ) -> Result<Vec<ToolResult>, RunError> {
        if cancellation_token.is_cancelled() {
            return Err(RunError::Cancelled);
        }

        if self.should_run_parallel(&calls) {
            let futures = calls.into_iter().map(|call| self.execute_tool_call(call, cancellation_token));
            let results = join_all(futures).await;
            results.into_iter().collect()
        } else {
            let mut results = Vec::new();
            for call in calls {
                results.push(self.execute_tool_call(call, cancellation_token).await?);
            }
            Ok(results)
        }
    }

    fn should_run_parallel(&self, calls: &[ToolCall]) -> bool {
        self.config.execution_mode == crate::ExecutionMode::ParallelWhenSafe
            && calls.iter().all(|call| {
                self.tools.get(&ToolName::new(call.name.clone()))
                    .map(|tool| tool.definition().annotations.execution == crate::ToolExecution::ParallelSafe)
                    .unwrap_or(false)
            })
    }

    pub(crate) async fn execute_tool_call(
        &self,
        call: ToolCall,
        cancellation_token: &CancellationToken,
    ) -> Result<ToolResult, RunError> {
        if cancellation_token.is_cancelled() { return Err(RunError::Cancelled); }

        let invocation = ToolInvocation {
            call_id: call.id.clone(),
            name: ToolName::new(call.name.clone()),
            arguments: call.arguments,
        };
        let definition = self.tools.get(&invocation.name).map(|tool| tool.definition());

        observe(&self.observer, Event::ToolDispatch {
            call_id: invocation.call_id.clone(),
            tool_name: invocation.name.to_string(),
            args_preview: invocation.arguments.clone(),
        }).await;
        observe(&self.observer, Event::ToolStarted {
            call_id: invocation.call_id.clone(),
            name: invocation.name.to_string(),
        }).await;

        let decision = self.policy
            .evaluate(ToolPolicyRequest { invocation: &invocation, definition: definition.as_ref() })
            .await
            .expect("policy futures are infallible");

        let result = match decision {
            ToolDecision::Allow => {
                observe(&self.observer, Event::ToolProgress {
                    call_id: invocation.call_id.clone(),
                    partial_output: String::new(),
                }).await;
                self.tools.call(invocation).await
            }
            ToolDecision::Deny { reason } => ToolResult::error(
                invocation.call_id, invocation.name.to_string(), format!("tool denied by policy: {reason}"),
            ),
            ToolDecision::Ask { request_id, reason } => {
                let subject = invocation.arguments.get("path")
                    .or_else(|| invocation.arguments.get("command"))
                    .or_else(|| invocation.arguments.get("query"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                observe(&self.observer, Event::ApprovalRequest {
                    request_id: request_id.clone(),
                    tool_name: invocation.name.to_string(),
                    subject: subject.clone(),
                    reason: reason.clone(),
                }).await;
                observe(&self.observer, Event::PermissionRequested {
                    request_id: request_id.clone(),
                    tool_name: invocation.name.to_string(),
                    subject: subject.clone(),
                    reason: reason.clone(),
                }).await;

                let approval = if let Some(channel) = &self.approval_channel {
                    channel.submit(ApprovalRequest {
                        request_id: request_id.clone(),
                        tool_name: invocation.name.to_string(),
                        subject, reason,
                    });
                    loop {
                        if cancellation_token.is_cancelled() { break Some(ApprovalResponse::Denied { reason: "cancelled".into() }); }
                        if let Some(response) = channel.take_response() { break Some(response); }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } else { None };

                match approval {
                    Some(ApprovalResponse::Approved { remember_for_session }) => {
                        observe(&self.observer, Event::PermissionResolved {
                            request_id, approved: true, reason: None,
                        }).await;
                        if remember_for_session && let Some(channel) = &self.approval_channel {
                            channel.grant_for_session(invocation.name.to_string());
                        }
                        self.tools.call(invocation).await
                    }
                    Some(ApprovalResponse::Denied { reason }) => {
                        observe(&self.observer, Event::PermissionResolved {
                            request_id, approved: false, reason: Some(reason.clone()),
                        }).await;
                        ToolResult::error(invocation.call_id, invocation.name.to_string(), format!("tool denied by user: {reason}"))
                    }
                    None => {
                        observe(&self.observer, Event::PermissionResolved {
                            request_id, approved: false, reason: Some("no approval channel".into()),
                        }).await;
                        ToolResult::error(invocation.call_id, invocation.name.to_string(), "tool denied: no approval channel available")
                    }
                }
            }
        };
        observe(&self.observer, Event::ToolFinished { result: result.clone() }).await;
        Ok(result)
    }
}

// ── AgentBuilder ─────────────────────────────────────────────────────────

pub struct AgentBuilder {
    model: Arc<dyn Model>,
    tools: Option<Arc<ToolRegistry>>,
    context_builder: Option<Arc<dyn ContextBuilder>>,
    policy: Option<Arc<dyn ToolPolicy>>,
    observer: Option<Arc<dyn Observer>>,
    config: Option<AgentConfig>,
    approval_channel: Option<SharedApprovalChannel>,
    _system_prompt: Option<String>,
}

impl AgentBuilder {
    #[must_use]
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self { model, tools: None, context_builder: None, policy: None, observer: None, config: None, approval_channel: None, _system_prompt: None }
    }

    #[must_use] pub fn tools(mut self, tools: Arc<ToolRegistry>) -> Self { self.tools = Some(tools); self }
    #[must_use] pub fn context_builder(mut self, context_builder: Arc<dyn ContextBuilder>) -> Self { self.context_builder = Some(context_builder); self }
    #[must_use] pub fn permissions(mut self, policy: Arc<dyn ToolPolicy>) -> Self { self.policy = Some(policy); self }
    #[must_use] pub fn observer(mut self, observer: Arc<dyn Observer>) -> Self { self.observer = Some(observer); self }
    #[must_use] pub fn approval_channel(mut self, channel: SharedApprovalChannel) -> Self { self.approval_channel = Some(channel); self }
    #[must_use] pub fn config(mut self, config: AgentConfig) -> Self { self.config = Some(config); self }
    #[must_use] pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self { self._system_prompt = Some(prompt.into()); self }

    #[must_use]
    pub fn build(self) -> Agent {
        Agent {
            model: self.model,
            tools: self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
            context_builder: self.context_builder.unwrap_or_else(|| Arc::new(PassthroughContextBuilder)),
            policy: self.policy.unwrap_or_else(|| Arc::new(AllowAllPolicy)),
            observer: self.observer.unwrap_or_else(|| Arc::new(NoopObserver)),
            config: self.config.unwrap_or_default(),
            approval_channel: self.approval_channel,
            run_state: AtomicBool::new(false),
        }
    }
}

// ── Model stream collection ──────────────────────────────────────────────

struct ModelOutput {
    content: Vec<Content>,
    tool_calls: Vec<ToolCall>,
    stop_reason: StopReason,
    usage: crate::Usage,
}

async fn collect_model_output(
    mut stream: crate::ModelStream,
    observer: &Arc<dyn Observer>,
    cancellation_token: &CancellationToken,
) -> Result<ModelOutput, RunError> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut content = Vec::new();
    let mut tool_calls = Vec::new();
    let mut stop_reason = StopReason::Stop;
    let mut model_usage = crate::Usage::default();

    while let Some(event) = stream.next().await {
        if cancellation_token.is_cancelled() { return Err(RunError::Cancelled); }
        match event? {
            ModelResponseEvent::TextDelta(delta) => {
                observe(observer, Event::TextDelta { delta: delta.clone() }).await;
                text.push_str(&delta);
            }
            ModelResponseEvent::ReasoningDelta(delta) => {
                observe(observer, Event::ReasoningDelta { delta: delta.clone() }).await;
                reasoning.push_str(&delta);
            }
            ModelResponseEvent::ToolCall(tool_call) => {
                if !text.is_empty() { content.push(Content::Text(TextContent { text: std::mem::take(&mut text) })); }
                if !reasoning.is_empty() { content.push(Content::Reasoning(crate::ReasoningContent { text: std::mem::take(&mut reasoning) })); }
                content.push(Content::ToolCall(tool_call.clone()));
                tool_calls.push(tool_call);
            }
            ModelResponseEvent::Finished { stop_reason: reason, usage: event_usage } => {
                model_usage = event_usage;
                stop_reason = reason;
                break;
            }
            ModelResponseEvent::Retrying { attempt, max_retries, reason } => {
                observe(observer, Event::Retrying {
                    attempt: attempt as usize,
                    max_attempts: max_retries as usize,
                    reason: reason.clone(),
                }).await;
            }
        }
    }

    if !text.is_empty() { content.push(Content::Text(TextContent { text })); }
    if !reasoning.is_empty() { content.push(Content::Reasoning(crate::ReasoningContent { text: reasoning })); }

    Ok(ModelOutput { content, tool_calls, stop_reason, usage: model_usage })
}

async fn observe(observer: &Arc<dyn Observer>, event: Event) {
    let _ = observer.observe(event).await;
}

async fn observe_limit(observer: &Arc<dyn Observer>, reason: impl Into<String>) {
    observe(observer, Event::LimitReached { reason: reason.into() }).await;
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
