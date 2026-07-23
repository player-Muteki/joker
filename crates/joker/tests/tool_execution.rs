use std::{sync::Arc, time::Duration};

use joker::{
    Agent, AgentConfig, Content, Event, ExecutionMode, RecordingObserver, ScriptedModel,
    ScriptedStep, ToolAnnotations, ToolDefinition, ToolError, ToolExecution, ToolFn, ToolFuture,
    ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use serde_json::json;

fn slow_first(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok(ToolOutput::new(json!({ "name": invocation.name.as_str() })))
    })
}

fn fast_second(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(ToolOutput::new(json!({ "name": invocation.name.as_str() })))
    })
}

fn make_tool(
    name: &'static str,
    handler: fn(ToolInvocation) -> ToolFuture<'static>,
) -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new(name),
            description: name.into(),
            input_schema: json!({"type":"object"}),
            annotations: ToolAnnotations {
                execution: ToolExecution::ParallelSafe,
                mutating: false,
                timeout: None,
            },
        },
        handler,
    )
}

#[tokio::test]
async fn parallel_finish_events_can_differ_from_transcript_order() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::message(
            vec![
                Content::ToolCall(joker::ToolCall {
                    id: "call-1".into(),
                    name: "slow".into(),
                    arguments: json!({}),
                }),
                Content::ToolCall(joker::ToolCall {
                    id: "call-2".into(),
                    name: "fast".into(),
                    arguments: json!({}),
                }),
            ],
            joker::StopReason::ToolUse,
        ),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry.insert(make_tool("slow", slow_first)).unwrap();
    registry.insert(make_tool("fast", fast_second)).unwrap();
    let agent = Agent::new(model)
        .with_tools(Arc::new(registry))
        .with_observer(Arc::new(observer.clone()))
        .with_config(AgentConfig {
            execution_mode: ExecutionMode::ParallelWhenSafe,
            ..AgentConfig::default()
        });

    let outcome = agent.run(joker::RunRequest::new("parallel")).await.unwrap();

    assert_eq!(outcome.conversation.messages()[2].content.len(), 2);
    let Content::ToolResult(first) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected first result");
    };
    let Content::ToolResult(second) = &outcome.conversation.messages()[2].content[1] else {
        panic!("expected second result");
    };
    assert_eq!(first.name, "slow");
    assert_eq!(second.name, "fast");

    let finished_names: Vec<_> = observer
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolFinished { result } => Some(result.name),
            _ => None,
        })
        .collect();
    assert_eq!(finished_names, vec!["fast", "slow"]);
}

#[tokio::test]
async fn tool_timeout_becomes_error_result() {
    fn timeout_tool(_invocation: ToolInvocation) -> ToolFuture<'static> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(ToolOutput::new(json!("late")))
        })
    }

    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "timeout", json!({})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("timeout"),
                description: "times out".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations {
                    execution: ToolExecution::Sequential,
                    mutating: false,
                    timeout: Some(Duration::from_millis(1)),
                },
            },
            timeout_tool as fn(ToolInvocation) -> ToolFuture<'static>,
        ))
        .unwrap();
    let agent = Agent::new(model).with_tools(Arc::new(registry));

    let outcome = agent.run(joker::RunRequest::new("timeout")).await.unwrap();

    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(result.output.as_str().unwrap().contains("timed out"));
}

#[tokio::test]
async fn invalid_arguments_and_tool_failures_become_error_results() {
    fn invalid(_invocation: ToolInvocation) -> ToolFuture<'static> {
        Box::pin(async move { Err(ToolError::InvalidArguments("missing field".into())) })
    }

    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "invalid", json!({})),
        ScriptedStep::text("recovered"),
    ])) as Arc<dyn joker::Model>;
    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("invalid"),
                description: "validates args".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations::default(),
            },
            invalid as fn(ToolInvocation) -> ToolFuture<'static>,
        ))
        .unwrap();
    let agent = Agent::new(model).with_tools(Arc::new(registry));

    let outcome = agent.run(joker::RunRequest::new("invalid")).await.unwrap();

    let Content::ToolResult(result) = &outcome.conversation.messages()[2].content[0] else {
        panic!("expected tool result");
    };
    assert!(result.is_error);
    assert!(
        result
            .output
            .as_str()
            .unwrap()
            .contains("invalid arguments")
    );
}
