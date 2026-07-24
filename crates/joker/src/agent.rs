use std::sync::Arc;

use futures_util::{StreamExt, future::join_all};
use tokio_util::sync::CancellationToken;

use crate::{
    AllowAllPolicy, ApprovalRequest, ApprovalResponse, BuiltContext, Content, ContextBuilder,
    ContextInput, ContextLimits, Event, Model, ModelRequest, ModelResponseEvent, NoopObserver,
    Observer, PassthroughContextBuilder, RunError, SharedApprovalChannel, StopReason, TextContent,
    ToolCall, ToolDecision, ToolInvocation, ToolName, ToolPolicy, ToolPolicyRequest, ToolRegistry,
    ToolResult,
};

pub struct Agent {
    model: Arc<dyn Model>,
    tools: Arc<ToolRegistry>,
    context_builder: Arc<dyn ContextBuilder>,
    policy: Arc<dyn ToolPolicy>,
    observer: Arc<dyn Observer>,
    config: AgentConfig,
    approval_channel: Option<SharedApprovalChannel>,
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
        let cancellation_token = request
            .cancellation_token
            .clone()
            .unwrap_or_else(CancellationToken::new);
        observe(&self.observer, Event::RunStarted).await;

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

                let BuiltContext { messages } = self
                    .context_builder
                    .build(ContextInput {
                        conversation: &request.conversation,
                        limits: self.config.context_limits,
                    })
                    .await?;

                observe(&self.observer, Event::ModelStarted).await;
                let stream = self
                    .model
                    .stream(ModelRequest {
                        messages,
                        tools: self.tools.definitions(),
                    })
                    .await?;
                let model_output =
                    collect_model_output(stream, &self.observer, &cancellation_token).await?;
                observe(
                    &self.observer,
                    Event::ModelFinished {
                        stop_reason: model_output.stop_reason,
                    },
                )
                .await;

                let assistant_message = crate::Message::assistant(model_output.content.clone());
                let pending_tool_calls = model_output.tool_calls;
                request.conversation.push(assistant_message);

                if pending_tool_calls.is_empty() {
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: model_output.stop_reason,
                    });
                }

                if tool_calls + pending_tool_calls.len() > self.config.limits.max_tool_calls {
                    observe_limit(&self.observer, "max_tool_calls").await;
                    return Ok(RunOutcome {
                        conversation: request.conversation,
                        stop_reason: StopReason::LimitReached,
                    });
                }
                tool_calls += pending_tool_calls.len();

                let results = self
                    .execute_tool_calls(pending_tool_calls, &cancellation_token)
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
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            limits: RunLimits::default(),
            execution_mode: ExecutionMode::Sequential,
            context_limits: ContextLimits::default(),
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

struct ModelOutput {
    content: Vec<Content>,
    tool_calls: Vec<ToolCall>,
    stop_reason: StopReason,
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

    while let Some(event) = stream.next().await {
        if cancellation_token.is_cancelled() {
            return Err(RunError::Cancelled);
        }
        match event? {
            ModelResponseEvent::TextDelta(delta) => {
                observe(
                    observer,
                    Event::ModelDelta {
                        delta: delta.clone(),
                    },
                )
                .await;
                text.push_str(&delta);
            }
            ModelResponseEvent::ReasoningDelta(delta) => {
                observe(
                    observer,
                    Event::ModelDelta {
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
                ..
            } => {
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
