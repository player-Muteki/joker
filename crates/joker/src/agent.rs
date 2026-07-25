// Items marked `pub` are re-exported via `lib.rs` — not truly unreachable.
#![allow(unreachable_pub)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use futures_util::{StreamExt, future::join_all};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    AllowAllPolicy, ApprovalRequest, ApprovalResponse, BuiltContext, Content, ContextBuilder,
    ContextInput, ContextLimits, Event, Model, ModelError, ModelRequest, ModelResponseEvent,
    NoopObserver, Observer, PassthroughContextBuilder, RunError, SharedApprovalChannel,
    StopReason, TextContent, ToolCall, ToolDecision, ToolInvocation, ToolName, ToolPolicy,
    ToolPolicyRequest, ToolRegistry, ToolResult,
};

pub struct Agent {
    model: Arc<dyn Model>,
    tools: Arc<ToolRegistry>,
    context_builder: Arc<dyn ContextBuilder>,
    policy: Arc<dyn ToolPolicy>,
    observer: Arc<dyn Observer>,
    config: AgentConfig,
    approval_channel: Option<SharedApprovalChannel>,
    run_state: AtomicBool,
}

/// RAII guard that resets the run state on drop.
struct RunGuard<'a>(&'a AtomicBool);

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
    pub fn with_tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub fn with_context_builder(mut self, context_builder: Arc<dyn ContextBuilder>) -> Self {
        self.context_builder = context_builder;
        self
    }

    #[must_use]
    pub fn with_policy(mut self, policy: Arc<dyn ToolPolicy>) -> Self {
        self.policy = policy;
        self
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observer = observer;
        self
    }

    #[must_use]
    pub fn with_approval_channel(mut self, channel: SharedApprovalChannel) -> Self {
        self.approval_channel = Some(channel);
        self
    }

    #[must_use]
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run(&self, mut request: RunRequest) -> Result<RunOutcome, RunError> {
        // Busy check — prevent concurrent runs
        if self.run_state.swap(true, Ordering::Acquire) {
            return Err(RunError::Busy);
        }
        let _guard = RunGuard(&self.run_state);

        let cancellation_token = request
            .cancellation_token
            .clone()
            .unwrap_or_default();
        observe(&self.observer, Event::RunStarted).await;

        // Generate a simple session/turn ID for event correlation
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
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: StopReason::LimitReached,
                    });
                }
                steps += 1;

                let outcome = self
                    .run_turn(&mut request.conversation, &turn_id, &model_id, &cancellation_token)
                    .await?;

                if !outcome.has_tool_calls {
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: outcome.stop_reason,
                    });
                }

                if tool_calls + outcome.pending_tool_calls.len() > self.config.limits.max_tool_calls
                {
                    observe_limit(&self.observer, "max_tool_calls").await;
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: StopReason::LimitReached,
                    });
                }
                tool_calls += outcome.pending_tool_calls.len();

                let results = self
                    .execute_tool_calls(outcome.pending_tool_calls, &cancellation_token)
                    .await?;
                request.conversation.push(crate::Message::tool(results));
            }
        }
        .await;

        if let Ok(outcome) = &result {
            stop_reason = outcome.stop_reason;
        } else if matches!(result, Err(RunError::Cancelled)) {
            stop_reason = StopReason::Cancelled;
        }
        observe(&self.observer, Event::RunFinished { stop_reason }).await;
        result
    }

    /// Execute a single turn: model call + tool execution.
    ///
    /// Returns a `TurnOutcome` indicating whether there were tool calls to continue.
    /// This method is the primitive that both `run()` and `AgentRuntime::run()` use.
    pub async fn run_turn(
        &self,
        conversation: &mut crate::Conversation,
        turn_id: &str,
        model_id: &str,
        cancellation_token: &CancellationToken,
    ) -> Result<TurnOutcome, RunError> {
        // Emit TurnStarted before each model call
        observe(
            &self.observer,
            Event::TurnStarted {
                session_id: String::new(),
                turn_id: turn_id.to_string(),
                agent_name: String::new(),
                model_id: model_id.to_string(),
            },
        )
        .await;

        let BuiltContext { messages } = self
            .context_builder
            .build(ContextInput {
                conversation,
                limits: self.config.context_limits,
            })
            .await?;

        observe(&self.observer, Event::ModelStarted).await;

        // Retry loop: handles model stream errors and zero-output responses
        let model_output = {
            let cfg = &self.config.retry;
            let mut stream_errors = 0usize;
            let mut zero_outputs = 0usize;

            loop {
                let stream = match self
                    .model
                    .stream(ModelRequest {
                        messages: messages.clone(),
                        tools: self.tools.definitions(),
                    })
                    .await
                {
                    Ok(stream) => stream,
                    Err(ModelError::Stream(reason)) => {
                        stream_errors += 1;
                        if stream_errors > cfg.max_stream_retries {
                            return Err(RunError::Model(ModelError::Stream(reason)));
                        }
                        observe(
                            &self.observer,
                            Event::Retrying {
                                attempt: stream_errors,
                                max_attempts: cfg.max_stream_retries,
                                reason: format!("model stream error: {reason}"),
                            },
                        )
                        .await;
                        tokio::time::sleep(Duration::from_millis(
                            cfg.base_delay_ms * (1 << (stream_errors - 1)),
                        ))
                        .await;
                        continue;
                    }
                    Err(ModelError::Cancelled) => return Err(RunError::Cancelled),
                };

                let output =
                    collect_model_output(stream, &self.observer, cancellation_token).await?;

                // Retry on zero output (empty content + no tool calls)
                if output.content.is_empty() && output.tool_calls.is_empty() {
                    zero_outputs += 1;
                    if zero_outputs <= cfg.max_zero_output_retries {
                        observe(
                            &self.observer,
                            Event::Retrying {
                                attempt: zero_outputs,
                                max_attempts: cfg.max_zero_output_retries,
                                reason: "model returned empty response".into(),
                            },
                        )
                        .await;
                        tokio::time::sleep(Duration::from_millis(
                            cfg.base_delay_ms * (1 << (zero_outputs - 1)),
                        ))
                        .await;
                        continue;
                    }
                }

                break output;
            }
        };

        // Emit ModelFinished before Usage/TurnDone (per event contract:
        // ModelStarted → … → ModelFinished → Usage → TurnDone)
        observe(
            &self.observer,
            Event::ModelFinished {
                stop_reason: model_output.stop_reason,
            },
        )
        .await;

        // Emit Usage event with token counts
        observe(
            &self.observer,
            Event::Usage {
                input_tokens: model_output.usage.input_tokens,
                output_tokens: model_output.usage.output_tokens,
                cache_hit_tokens: model_output.usage.cache_hit_tokens,
            },
        )
        .await;

        // Emit TurnDone for this model call
        observe(
            &self.observer,
            Event::TurnDone {
                turn_id: turn_id.to_string(),
                stop_reason: model_output.stop_reason,
            },
        )
        .await;

        let assistant_message = crate::Message::assistant(model_output.content.clone());
        let pending_tool_calls = model_output.tool_calls;
        conversation.push(assistant_message);

        if pending_tool_calls.is_empty() {
            return Ok(TurnOutcome {
                stop_reason: model_output.stop_reason,
                has_tool_calls: false,
                tool_calls_count: 0,
                pending_tool_calls: Vec::new(),
            });
        }

        Ok(TurnOutcome {
            stop_reason: model_output.stop_reason,
            has_tool_calls: true,
            tool_calls_count: pending_tool_calls.len(),
            pending_tool_calls,
        })
    }

    async fn execute_tool_calls(
        &self,
        calls: Vec<ToolCall>,
        cancellation_token: &CancellationToken,
    ) -> Result<Vec<ToolResult>, RunError> {
        if cancellation_token.is_cancelled() {
            return Err(RunError::Cancelled);
        }

        if self.should_run_parallel(&calls) {
            let futures = calls
                .into_iter()
                .map(|call| self.execute_tool_call(call, cancellation_token));
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
        self.config.execution_mode == ExecutionMode::ParallelWhenSafe
            && calls.iter().all(|call| {
                self.tools
                    .get(&ToolName::new(call.name.clone()))
                    .map(|tool| {
                        tool.definition().annotations.execution
                            == crate::ToolExecution::ParallelSafe
                    })
                    .unwrap_or(false)
            })
    }

    async fn execute_tool_call(
        &self,
        call: ToolCall,
        cancellation_token: &CancellationToken,
    ) -> Result<ToolResult, RunError> {
        if cancellation_token.is_cancelled() {
            return Err(RunError::Cancelled);
        }

        let invocation = ToolInvocation {
            call_id: call.id.clone(),
            name: ToolName::new(call.name.clone()),
            arguments: call.arguments,
        };
        let definition = self
            .tools
            .get(&invocation.name)
            .map(|tool| tool.definition());
        // Emit ToolDispatch before execution so the UI can show the pending call
        observe(
            &self.observer,
            Event::ToolDispatch {
                call_id: invocation.call_id.clone(),
                tool_name: invocation.name.to_string(),
                args_preview: invocation.arguments.clone(),
            },
        )
        .await;
        observe(
            &self.observer,
            Event::ToolStarted {
                call_id: invocation.call_id.clone(),
                name: invocation.name.to_string(),
            },
        )
        .await;

        let decision = self
            .policy
            .evaluate(ToolPolicyRequest {
                invocation: &invocation,
                definition: definition.as_ref(),
            })
            .await
            .expect("policy futures are infallible");

        let result = match decision {
            ToolDecision::Allow => self.tools.call(invocation).await,
            ToolDecision::Deny { reason } => ToolResult::error(
                invocation.call_id,
                invocation.name.to_string(),
                format!("tool denied by policy: {reason}"),
            ),
            ToolDecision::Ask {
                request_id,
                reason,
            } => {
                // Extract subject from invocation arguments for display
                let subject = invocation
                    .arguments
                    .get("path")
                    .or_else(|| invocation.arguments.get("command"))
                    .or_else(|| invocation.arguments.get("query"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // Emit permission requested event
                observe(
                    &self.observer,
                    Event::PermissionRequested {
                        request_id: request_id.clone(),
                        tool_name: invocation.name.to_string(),
                        subject: subject.clone(),
                        reason: reason.clone(),
                    },
                )
                .await;

                // Try to resolve via approval channel
                let approval = if let Some(channel) = &self.approval_channel {
                    channel.submit(ApprovalRequest {
                        request_id: request_id.clone(),
                        tool_name: invocation.name.to_string(),
                        subject,
                        reason,
                    });
                    // Poll for response with cancellation support
                    loop {
                        if cancellation_token.is_cancelled() {
                            break Some(ApprovalResponse::Denied {
                                reason: "cancelled".into(),
                            });
                        }
                        if let Some(response) = channel.take_response() {
                            break Some(response);
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } else {
                    None
                };

                match approval {
                    Some(ApprovalResponse::Approved {
                        remember_for_session: _,
                    }) => {
                        observe(
                            &self.observer,
                            Event::PermissionResolved {
                                request_id,
                                approved: true,
                                reason: None,
                            },
                        )
                        .await;
                        self.tools.call(invocation).await
                    }
                    Some(ApprovalResponse::Denied { reason }) => {
                        observe(
                            &self.observer,
                            Event::PermissionResolved {
                                request_id,
                                approved: false,
                                reason: Some(reason.clone()),
                            },
                        )
                        .await;
                        ToolResult::error(
                            invocation.call_id,
                            invocation.name.to_string(),
                            format!("tool denied by user: {reason}"),
                        )
                    }
                    None => {
                        observe(
                            &self.observer,
                            Event::PermissionResolved {
                                request_id,
                                approved: false,
                                reason: Some("no approval channel".into()),
                            },
                        )
                        .await;
                        ToolResult::error(
                            invocation.call_id,
                            invocation.name.to_string(),
                            "tool denied: no approval channel available",
                        )
                    }
                }
            }
        };
        observe(
            &self.observer,
            Event::ToolFinished {
                result: result.clone(),
            },
        )
        .await;
        Ok(result)
    }
}

// ── AgentBuilder ────────────────────────────────────────────────────────

/// Fluent builder for constructing an [`Agent`].
///
/// ```rust,ignore
/// use joker::AgentBuilder;
///
/// let agent = AgentBuilder::new(model)
///     .system_prompt("You are a coding agent.")
///     .tools(tool_registry)
///     .permissions(permission_policy)
///     .observer(observer)
///     .approval_channel(channel)
///     .build();
/// ```
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
        Self {
            model,
            tools: None,
            context_builder: None,
            policy: None,
            observer: None,
            config: None,
            approval_channel: None,
            _system_prompt: None,
        }
    }

    #[must_use]
    pub fn tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    #[must_use]
    pub fn context_builder(mut self, context_builder: Arc<dyn ContextBuilder>) -> Self {
        self.context_builder = Some(context_builder);
        self
    }

    #[must_use]
    pub fn permissions(mut self, policy: Arc<dyn ToolPolicy>) -> Self {
        self.policy = Some(policy);
        self
    }

    #[must_use]
    pub fn observer(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observer = Some(observer);
        self
    }

    #[must_use]
    pub fn approval_channel(mut self, channel: SharedApprovalChannel) -> Self {
        self.approval_channel = Some(channel);
        self
    }

    #[must_use]
    pub fn config(mut self, config: AgentConfig) -> Self {
        self.config = Some(config);
        self
    }

    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self._system_prompt = Some(prompt.into());
        self
    }

    #[must_use]
    pub fn build(self) -> Agent {
        let tools = self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new()));
        let context_builder: Arc<dyn ContextBuilder> = self
            .context_builder
            .unwrap_or_else(|| Arc::new(PassthroughContextBuilder));
        let policy: Arc<dyn ToolPolicy> = self
            .policy
            .unwrap_or_else(|| Arc::new(AllowAllPolicy));
        let observer: Arc<dyn Observer> = self
            .observer
            .unwrap_or_else(|| Arc::new(NoopObserver));

        Agent {
            model: self.model,
            tools,
            context_builder,
            policy,
            observer,
            config: self.config.unwrap_or_default(),
            approval_channel: self.approval_channel,
            run_state: AtomicBool::new(false),
        }
    }
}

// ── ToolSet ─────────────────────────────────────────────────────────────

/// A builder for selecting which tool categories to include in an agent.
///
/// ```rust,ignore
/// let tools = ToolSet::new()
///     .read()           // list_files, read_file
///     .grep()           // grep
///     .write()          // write_file, edit_file, apply_patch
///     .shell()          // shell
///     .web_search()     // web_search, fetch_url
///     .build(workspace)?;
/// ```
///
/// Currently a stub — will be fully wired when `joker-tools` exposes categorized registries.
pub struct ToolSet {
    read: bool,
    grep: bool,
    write: bool,
    shell: bool,
    web_search: bool,
}

impl ToolSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            read: false,
            grep: false,
            write: false,
            shell: false,
            web_search: false,
        }
    }

    #[must_use]
    pub fn read(mut self) -> Self {
        self.read = true;
        self
    }

    #[must_use]
    pub fn grep(mut self) -> Self {
        self.grep = true;
        self
    }

    #[must_use]
    pub fn write(mut self) -> Self {
        self.write = true;
        self
    }

    #[must_use]
    pub fn shell(mut self) -> Self {
        self.shell = true;
        self
    }

    #[must_use]
    pub fn web_search(mut self) -> Self {
        self.web_search = true;
        self
    }

    /// Returns which categories are enabled.
    #[must_use]
    pub fn has_read(&self) -> bool { self.read }

    #[must_use]
    pub fn has_grep(&self) -> bool { self.grep }

    #[must_use]
    pub fn has_write(&self) -> bool { self.write }

    #[must_use]
    pub fn has_shell(&self) -> bool { self.shell }

    #[must_use]
    pub fn has_web_search(&self) -> bool { self.web_search }
}

impl Default for ToolSet {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    pub limits: RunLimits,
    pub execution_mode: ExecutionMode,
    pub context_limits: ContextLimits,
    pub retry: RetryConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            limits: RunLimits::default(),
            execution_mode: ExecutionMode::Sequential,
            context_limits: ContextLimits::default(),
            retry: RetryConfig::default(),
        }
    }
}

/// Configuration for retry behavior when the model stream fails or returns
/// empty output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryConfig {
    /// Maximum number of retries on `ModelError::Stream` (default: 4).
    pub max_stream_retries: usize,
    /// Maximum number of retries when the model returns empty output
    /// (no content, no tool calls). Default: 3.
    pub max_zero_output_retries: usize,
    /// Base delay in milliseconds for exponential backoff.
    /// Actual delay = `base_delay_ms * 2^(attempt - 1)`. Default: 1000.
    pub base_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_stream_retries: 4,
            max_zero_output_retries: 3,
            base_delay_ms: 1000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimits {
    pub max_steps: usize,
    pub max_tool_calls: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_steps: 16,
            max_tool_calls: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Sequential,
    ParallelWhenSafe,
}

pub struct RunRequest {
    pub conversation: crate::Conversation,
    pub input: Option<String>,
    pub approval_channel: Option<SharedApprovalChannel>,
    pub cancellation_token: Option<CancellationToken>,
}

impl RunRequest {
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            conversation: crate::Conversation::new(),
            input: Some(input.into()),
            approval_channel: None,
            cancellation_token: None,
        }
    }

    #[must_use]
    pub fn with_conversation(conversation: crate::Conversation) -> Self {
        Self {
            conversation,
            input: None,
            approval_channel: None,
            cancellation_token: None,
        }
    }

    #[must_use]
    pub fn with_approval_channel(mut self, channel: SharedApprovalChannel) -> Self {
        self.approval_channel = Some(channel);
        self
    }

    #[must_use]
    pub fn with_cancellation_token(mut self, cancellation_token: CancellationToken) -> Self {
        self.cancellation_token = Some(cancellation_token);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    pub conversation: crate::Conversation,
    pub stop_reason: StopReason,
}

/// Result of a single turn execution (one model call, no tool execution).
///
/// The caller is responsible for checking `max_tool_calls` and then
/// executing `pending_tool_calls` via `agent.execute_tool_calls()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    pub stop_reason: StopReason,
    pub has_tool_calls: bool,
    pub tool_calls_count: usize,
    pub pending_tool_calls: Vec<ToolCall>,
}

/// Commands that can be sent to an active `AgentRuntime` run.
///
/// Ops are processed between turns (not mid-turn). For mid-turn cancellation,
/// the existing `CancellationToken` mechanism is used.
#[derive(Clone, Debug)]
pub enum Op {
    /// Cancel the current run (triggers CancellationToken).
    Cancel,
    /// Request context compaction on the next turn.
    Compact,
    /// Switch to a different agent profile.
    SwitchAgent { name: String },
    /// Shut down the run loop.
    Shutdown,
}

/// An agent execution runtime with OpLoop support.
///
/// Wraps an `Agent` and processes `Op` commands between turns. The underlying
/// `Agent::run()` API is unchanged — existing code that calls `agent.run(request)`
/// continues to work without modification.
pub struct AgentRuntime {
    agent: Agent,
}

impl AgentRuntime {
    /// Create a new runtime wrapping an Agent.
    #[must_use]
    pub fn new(agent: Agent) -> Self {
        Self { agent }
    }

    /// Run with OpLoop — processes Ops between turns via the provided channel.
    ///
    /// The caller is responsible for creating the channel and keeping the sender
    /// to submit Ops during execution.
    ///
    /// # Op processing
    /// - `Cancel` triggers the CancellationToken (checked mid-turn also)
    /// - `Shutdown` returns `Err(RunError::Shutdown)` at the next turn boundary
    /// - `Compact` and `SwitchAgent` emit events for the UI to observe
    pub async fn run(
        &self,
        mut request: RunRequest,
        rx_op: &mut mpsc::UnboundedReceiver<Op>,
    ) -> Result<RunOutcome, RunError> {
        // Busy check — prevent concurrent runs
        if self.agent.run_state.swap(true, Ordering::Acquire) {
            return Err(RunError::Busy);
        }
        let _guard = RunGuard(&self.agent.run_state);

        let cancellation_token = request
            .cancellation_token
            .clone()
            .unwrap_or_default();
        observe(&self.agent.observer, Event::RunStarted).await;

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
                // Process pending Ops between turns (non-blocking)
                while let Ok(op) = rx_op.try_recv() {
                    match op {
                        Op::Cancel => {
                            cancellation_token.cancel();
                        }
                        Op::Compact => {
                            observe(
                                &self.agent.observer,
                                Event::CompactionStarted {
                                    trigger: "manual".into(),
                                    current_tokens: 0,
                                    threshold: 0,
                                },
                            )
                            .await;
                            observe(
                                &self.agent.observer,
                                Event::CompactionDone {
                                    tokens_before: 0,
                                    tokens_after: 0,
                                },
                            )
                            .await;
                        }
                        Op::SwitchAgent { name } => {
                            observe(
                                &self.agent.observer,
                                Event::AgentSwitched {
                                    from: String::new(),
                                    to: name,
                                },
                            )
                            .await;
                        }
                        Op::Shutdown => return Err(RunError::Shutdown),
                    }
                }

                if cancellation_token.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                if steps >= self.agent.config.limits.max_steps {
                    observe_limit(&self.agent.observer, "max_steps").await;
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: StopReason::LimitReached,
                    });
                }
                steps += 1;

                let outcome = self
                    .agent
                    .run_turn(&mut request.conversation, &turn_id, &model_id, &cancellation_token)
                    .await?;

                if !outcome.has_tool_calls {
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: outcome.stop_reason,
                    });
                }

                if tool_calls + outcome.tool_calls_count > self.agent.config.limits.max_tool_calls
                {
                    observe_limit(&self.agent.observer, "max_tool_calls").await;
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: StopReason::LimitReached,
                    });
                }
                tool_calls += outcome.tool_calls_count;

                let results = self
                    .agent
                    .execute_tool_calls(outcome.pending_tool_calls, &cancellation_token)
                    .await?;
                request.conversation.push(crate::Message::tool(results));
            }
        }
        .await;

        if let Ok(outcome) = &result {
            stop_reason = outcome.stop_reason;
        } else if matches!(result, Err(RunError::Cancelled)) {
            stop_reason = StopReason::Cancelled;
        }
        observe(&self.agent.observer, Event::RunFinished { stop_reason }).await;
        result
    }
}

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
        if cancellation_token.is_cancelled() {
            return Err(RunError::Cancelled);
        }
        match event? {
            ModelResponseEvent::TextDelta(delta) => {
                observe(
                    observer,
                    Event::TextDelta {
                        delta: delta.clone(),
                    },
                )
                .await;
                text.push_str(&delta);
            }
            ModelResponseEvent::ReasoningDelta(delta) => {
                observe(
                    observer,
                    Event::ReasoningDelta {
                        delta: delta.clone(),
                    },
                )
                .await;
                reasoning.push_str(&delta);
            }
            ModelResponseEvent::ToolCall(tool_call) => {
                if !text.is_empty() {
                    content.push(Content::Text(TextContent {
                        text: std::mem::take(&mut text),
                    }));
                }
                if !reasoning.is_empty() {
                    content.push(Content::Reasoning(crate::ReasoningContent {
                        text: std::mem::take(&mut reasoning),
                    }));
                }
                content.push(Content::ToolCall(tool_call.clone()));
                tool_calls.push(tool_call);
            }
            ModelResponseEvent::Finished {
                stop_reason: reason,
                usage: event_usage,
            } => {
                model_usage = event_usage;
                stop_reason = reason;
                break;
            }
        }
    }

    if !text.is_empty() {
        content.push(Content::Text(TextContent { text }));
    }
    if !reasoning.is_empty() {
        content.push(Content::Reasoning(crate::ReasoningContent {
            text: reasoning,
        }));
    }

    Ok(ModelOutput {
        content,
        tool_calls,
        stop_reason,
        usage: model_usage,
    })
}

async fn observe(observer: &Arc<dyn Observer>, event: Event) {
    let _ = observer.observe(event).await;
}

async fn observe_limit(observer: &Arc<dyn Observer>, reason: impl Into<String>) {
    observe(
        observer,
        Event::LimitReached {
            reason: reason.into(),
        },
    )
    .await;
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
