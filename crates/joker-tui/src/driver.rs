use std::{path::PathBuf, sync::Arc};

use joker::{
    Agent, Content, ModelResponseEvent, Observer, ObserverFuture, RunRequest, ScriptedModel,
    ScriptedStep, StopReason, ToolAnnotations, ToolDefinition, ToolFn, ToolFuture, ToolInvocation,
    ToolName, ToolOutput,
};
use joker_config::{ProviderSelection, RuntimeConfig};
use joker_provider::{anthropic, google};
use joker_provider::OpenAiCompatibleModel;
use joker_tools::readonly_tool_registry;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{TuiError, event::UiEvent};

#[derive(Clone)]
pub struct ChannelObserver {
    tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
}

impl ChannelObserver {
    #[must_use]
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) -> Self {
        Self { tx }
    }
}

impl Observer for ChannelObserver {
    fn observe(&self, event: joker::Event) -> ObserverFuture<'_> {
        let tx = self.tx.clone();
        Box::pin(async move {
            let _ = tx.send(UiEvent::Agent(event));
            Ok(())
        })
    }
}

#[derive(Clone, Debug)]
pub struct AgentDriver {
    runtime_config: RuntimeConfig,
    workspace: PathBuf,
}

impl AgentDriver {
    #[must_use]
    pub fn new(runtime_config: RuntimeConfig, workspace: impl Into<PathBuf>) -> Self {
        Self {
            runtime_config,
            workspace: workspace.into(),
        }
    }

    pub fn set_runtime_config(&mut self, runtime_config: RuntimeConfig) {
        self.runtime_config = runtime_config;
    }

    pub fn spawn_run(
        &self,
        prompt: String,
        cancellation_token: CancellationToken,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
    ) -> Result<JoinHandle<()>, TuiError> {
        let agent = self.build_agent(prompt.clone(), tx.clone())?;
        Ok(tokio::spawn(async move {
            let result = agent
                .run(RunRequest::new(prompt).with_cancellation_token(cancellation_token))
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(UiEvent::RunCompleted(result));
        }))
    }

    fn build_agent(
        &self,
        prompt: String,
        tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
    ) -> Result<Agent, TuiError> {
        let model = self.build_model(prompt)?;
        let mut agent = Agent::new(model).with_observer(Arc::new(ChannelObserver::new(tx)));

        let mut registry = readonly_tool_registry(&self.workspace)
            .map_err(|error| TuiError::Agent(error.to_string()))?;
        if self.runtime_config.demo_tool {
            registry
                .insert(make_echo_tool())
                .map_err(|error| TuiError::Agent(error.to_string()))?;
        }
        agent = agent.with_tools(Arc::new(registry));

        Ok(agent)
    }

    fn build_model(&self, prompt: String) -> Result<Arc<dyn joker::Model>, TuiError> {
        match &self.runtime_config.provider {
            ProviderSelection::Scripted { .. } => {
                Ok(Arc::new(ScriptedModel::new(self.scripted_steps(prompt)))
                    as Arc<dyn joker::Model>)
            }
            ProviderSelection::OpenAiCompatible(config) => Ok(Arc::new(
                OpenAiCompatibleModel::new(config.clone())
                    .map_err(|error| TuiError::Agent(error.to_string()))?,
            )
                as Arc<dyn joker::Model>),
            ProviderSelection::Anthropic { model, api_key } => {
                let key = api_key.clone().unwrap_or_default();
                let cfg = anthropic::AnthropicConfig {
                    base_url: anthropic::DEFAULT_BASE_URL.into(),
                    model: model.clone(),
                    api_key: key,
                };
                Ok(Arc::new(
                    anthropic::AnthropicModel::new(cfg)
                        .map_err(|e| TuiError::Agent(e.to_string()))?,
                )
                    as Arc<dyn joker::Model>)
            }
            ProviderSelection::Google { model, api_key } => {
                let key = api_key.clone().unwrap_or_default();
                let cfg = google::GoogleConfig {
                    base_url: google::DEFAULT_BASE_URL.into(),
                    model: model.clone(),
                    api_key: key,
                };
                Ok(Arc::new(
                    google::GoogleModel::new(cfg)
                        .map_err(|e| TuiError::Agent(e.to_string()))?,
                )
                    as Arc<dyn joker::Model>)
            }
        }
    }

    fn scripted_steps(&self, prompt: String) -> Vec<ScriptedStep> {
        if self.runtime_config.demo_tool {
            vec![
                ScriptedStep::message(
                    vec![
                        Content::text("Calling demo echo tool...\n"),
                        Content::ToolCall(joker::ToolCall {
                            id: "demo-echo-1".into(),
                            name: "echo".into(),
                            arguments: json!({ "text": prompt }),
                        }),
                    ],
                    StopReason::ToolUse,
                ),
                ScriptedStep::Events(streaming_text_events(
                    &self.runtime_config.scripted_response,
                )),
            ]
        } else {
            vec![ScriptedStep::Events(streaming_text_events(
                &self.runtime_config.scripted_response,
            ))]
        }
    }
}

fn streaming_text_events(text: &str) -> Vec<ModelResponseEvent> {
    let mut events = text
        .split_inclusive(char::is_whitespace)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| ModelResponseEvent::TextDelta(chunk.to_string()))
        .collect::<Vec<_>>();

    if events.is_empty() && !text.is_empty() {
        events.push(ModelResponseEvent::TextDelta(text.to_string()));
    }

    events.push(ModelResponseEvent::Finished {
        stop_reason: StopReason::Stop,
        usage: joker::Usage::default(),
    });
    events
}

fn make_echo_tool() -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    fn echo(invocation: ToolInvocation) -> ToolFuture<'static> {
        Box::pin(async move {
            Ok(ToolOutput::new(json!({
                "echo": invocation.arguments.get("text").and_then(|value| value.as_str()).unwrap_or("")
            })))
        })
    }

    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("echo"),
            description: "Returns the submitted prompt text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
            annotations: ToolAnnotations::default(),
        },
        echo as fn(ToolInvocation) -> ToolFuture<'static>,
    )
}
