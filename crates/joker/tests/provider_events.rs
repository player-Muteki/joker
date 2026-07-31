use std::sync::Arc;

use joker::{
    Agent, AgentConfig, Content, Event, ModelResponseEvent, RecordingObserver, RetryConfig,
    RunRequest, ScriptedModel, ScriptedStep, StopReason, ToolAnnotations, ToolDefinition,
    ToolExecution, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use serde_json::json;

#[tokio::test]
async fn text_only_turn_emits_text_delta_and_finished() {
    let observer = RecordingObserver::new();
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("hello")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));

    let outcome = agent.run(RunRequest::new("hi")).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();
    let text_deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["hello"]);

    assert!(events.iter().any(|e| matches!(
        e,
        Event::ModelFinished {
            stop_reason: StopReason::Stop
        }
    )));
}

#[tokio::test]
async fn tool_call_turn_emits_tool_events_and_finished() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "greeter", json!({"name": "world"})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;

    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("greeter"),
                description: "greets".into(),
                input_schema: json!({"type": "object"}),
                annotations: ToolAnnotations {
                    execution: ToolExecution::Sequential,
                    ..ToolAnnotations::default()
                },
            },
            |_invocation: ToolInvocation| -> ToolFuture<'static> {
                Box::pin(async move { Ok(ToolOutput::new(json!("hello"))) })
            },
        ))
        .unwrap();

    let agent = Agent::new(model)
        .with_tools(Arc::new(registry))
        .with_observer(Arc::new(observer.clone()));

    let outcome = agent.run(RunRequest::new("greet")).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();

    let tool_starts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolStarted { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_starts, vec!["greeter"]);

    let tool_finishes = events
        .iter()
        .filter(|e| matches!(e, Event::ToolFinished { .. }))
        .count();
    assert_eq!(tool_finishes, 1);

    assert!(events.iter().any(
        |e| matches!(e, Event::ModelFinished { stop_reason } if *stop_reason == StopReason::ToolUse)
    ));

    let second_finish = events
        .iter()
        .filter(|e| matches!(e, Event::ModelFinished { stop_reason } if *stop_reason == StopReason::Stop))
        .count();
    assert_eq!(second_finish, 1);
}

#[tokio::test]
async fn model_error_produces_run_error() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([ScriptedStep::Error(
        "something went wrong".into(),
    )])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model)
        .with_observer(Arc::new(observer.clone()))
        .with_config(AgentConfig {
            retry: RetryConfig {
                max_stream_retries: 0,
                ..RetryConfig::default()
            },
            ..AgentConfig::default()
        });

    let err = agent.run(RunRequest::new("hi")).await.unwrap_err();
    assert!(err.to_string().contains("something went wrong"));

    let events = observer.events();
    assert!(matches!(events.first(), Some(Event::RunStarted)));
    assert!(matches!(events.last(), Some(Event::RunFinished { .. })));
}

#[tokio::test]
async fn message_with_tool_use_triggers_tool_dispatch_events() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::message(
            vec![Content::ToolCall(joker::ToolCall {
                id: "call-1".into(),
                name: "file_reader".into(),
                arguments: json!({"path": "/tmp/test"}),
            })],
            StopReason::ToolUse,
        ),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;

    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("file_reader"),
                description: "reads a file".into(),
                input_schema: json!({"type": "object"}),
                annotations: ToolAnnotations::default(),
            },
            |_invocation: ToolInvocation| -> ToolFuture<'static> {
                Box::pin(async move { Ok(ToolOutput::new(json!("content"))) })
            },
        ))
        .unwrap();

    let agent = Agent::new(model)
        .with_tools(Arc::new(registry))
        .with_observer(Arc::new(observer.clone()));

    let outcome = agent.run(RunRequest::new("read file")).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();

    let dispatches: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolDispatch {
                call_id, tool_name, ..
            } => Some((call_id.as_str(), tool_name.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].0, "call-1");
    assert_eq!(dispatches[0].1, "file_reader");
}

#[tokio::test]
async fn usage_info_is_propagated_through_events() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([ScriptedStep::Events(vec![
        ModelResponseEvent::TextDelta("analyzed".into()),
        ModelResponseEvent::Finished {
            stop_reason: StopReason::Stop,
            usage: joker::Usage {
                input_tokens: 42,
                output_tokens: 99,
                cache_hit_tokens: 7,
            },
        },
    ])])) as Arc<dyn joker::Model>;

    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));

    let outcome = agent.run(RunRequest::new("analyze")).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();
    let usage_events: Vec<(u64, u64, u64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Usage {
                input_tokens,
                output_tokens,
                cache_hit_tokens,
            } => Some((*input_tokens, *output_tokens, *cache_hit_tokens)),
            _ => None,
        })
        .collect();
    assert_eq!(usage_events, vec![(42, 99, 7)]);
}
