use std::sync::Arc;

use joker::{
    Agent, Content, ModelResponseEvent, Observer, ObserverFuture, RunRequest, ScriptedModel,
    ScriptedStep, StopReason, ToolAnnotations, ToolDefinition, ToolFn, ToolFuture, ToolInvocation,
    ToolName, ToolOutput, ToolRegistry,
};
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
    scripted_response: String,
    demo_tool: bool,
}

impl AgentDriver {
    #[must_use]
    pub fn new(scripted_response: impl Into<String>, demo_tool: bool) -> Self {
        Self {
            scripted_response: scripted_response.into(),
            demo_tool,
        }
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
        let model = Arc::new(ScriptedModel::new(self.steps_for(prompt))) as Arc<dyn joker::Model>;
        let mut agent = Agent::new(model).with_observer(Arc::new(ChannelObserver::new(tx)));

        if self.demo_tool {
            let mut registry = ToolRegistry::new();
            registry
                .insert(make_echo_tool())
                .map_err(|error| TuiError::Agent(error.to_string()))?;
            agent = agent.with_tools(Arc::new(registry));
        }

        Ok(agent)
    }

    fn steps_for(&self, prompt: String) -> Vec<ScriptedStep> {
        if self.demo_tool {
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
                ScriptedStep::Events(streaming_text_events(&self.scripted_response)),
            ]
        } else {
            vec![ScriptedStep::Events(streaming_text_events(
                &self.scripted_response,
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
