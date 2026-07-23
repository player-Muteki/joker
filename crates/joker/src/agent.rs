use std::sync::Arc;

use futures_util::{StreamExt, future::join_all};
use tokio_util::sync::CancellationToken;

use crate::{
    AllowAllPolicy, BuiltContext, Content, ContextBuilder, ContextInput, ContextLimits, Event,
    Model, ModelRequest, ModelResponseEvent, NoopObserver, Observer, PassthroughContextBuilder,
    RunError, StopReason, TextContent, ToolCall, ToolDecision, ToolInvocation, ToolName,
    ToolPolicy, ToolPolicyRequest, ToolRegistry, ToolResult,
};

pub struct Agent {
    model: Arc<dyn Model>,
    tools: Arc<ToolRegistry>,
    context_builder: Arc<dyn ContextBuilder>,
    policy: Arc<dyn ToolPolicy>,
    observer: Arc<dyn Observer>,
    config: AgentConfig,
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
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run(&self, mut request: RunRequest) -> Result<RunOutcome, RunError> {
        let cancellation_token = request.cancellation_token.clone().unwrap_or_default();
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
    pub cancellation_token: Option<CancellationToken>,
}

impl RunRequest {
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            conversation: crate::Conversation::new(),
            input: Some(input.into()),
            cancellation_token: None,
        }
    }

    #[must_use]
    pub fn with_conversation(conversation: crate::Conversation) -> Self {
        Self {
            conversation,
            input: None,
            cancellation_token: None,
        }
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
