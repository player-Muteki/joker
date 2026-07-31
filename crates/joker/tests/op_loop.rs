use std::sync::Arc;

use joker::{
    Agent, AgentRuntime, Op, RecordingObserver, RunError, RunRequest, ScriptedModel, ScriptedStep,
    StopReason,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn agent_runtime_basic_run() {
    let observer = RecordingObserver::new();
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("hello")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));
    let runtime = AgentRuntime::new(agent);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let _tx = tx; // keep sender alive for the duration

    let outcome = runtime.run(RunRequest::new("hi"), &mut rx).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();
    assert!(matches!(
        events.last(),
        Some(joker::Event::RunFinished { .. })
    ));
}

#[tokio::test]
async fn agent_runtime_preserves_final_model_stop_reason() {
    let model = Arc::new(ScriptedModel::new([ScriptedStep::message(
        vec![joker::Content::text("truncated")],
        StopReason::Length,
    )])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model);
    let runtime = AgentRuntime::new(agent);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let _tx = tx;

    let outcome = runtime.run(RunRequest::new("hi"), &mut rx).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Length);
}

#[tokio::test]
async fn op_cancel_stops_run() {
    let observer = RecordingObserver::new();
    // Use a tool-call step so the run needs a second turn — the Op is
    // processed between turns via try_recv.
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", serde_json::json!({"text": "hi"})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));
    let runtime = AgentRuntime::new(agent);

    let cancel_token = CancellationToken::new();
    let request = RunRequest::new("test").with_cancellation_token(cancel_token.clone());

    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = tokio::spawn(async move { runtime.run(request, &mut rx).await });

    // Send Cancel via Op. Between turns the AgentRuntime will process it.
    let _ = tx.send(Op::Cancel);
    drop(tx);

    let result = handle.await.unwrap();
    assert!(matches!(result, Err(RunError::Cancelled)));
}

#[tokio::test]
async fn op_shutdown_stops_run() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", serde_json::json!({"text": "hi"})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));
    let runtime = AgentRuntime::new(agent);

    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = tokio::spawn(async move { runtime.run(RunRequest::new("test"), &mut rx).await });

    let _ = tx.send(Op::Shutdown);
    drop(tx);

    let result = handle.await.unwrap();
    assert!(matches!(result, Err(RunError::Shutdown)));
}

#[tokio::test]
async fn op_compact_emits_compaction_events() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", serde_json::json!({"text": "hi"})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));
    let runtime = AgentRuntime::new(agent);

    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = tokio::spawn(async move { runtime.run(RunRequest::new("test"), &mut rx).await });

    let _ = tx.send(Op::Compact);
    drop(tx);

    let _ = handle.await.unwrap();

    let events = observer.events();
    let compact_started = events
        .iter()
        .any(|e| matches!(e, joker::Event::CompactionStarted { .. }));
    let compact_done = events
        .iter()
        .any(|e| matches!(e, joker::Event::CompactionDone { .. }));
    assert!(compact_started, "should emit CompactionStarted");
    assert!(compact_done, "should emit CompactionDone");
}

#[tokio::test]
async fn op_switch_agent_emits_agent_switched() {
    let observer = RecordingObserver::new();
    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("call-1", "echo", serde_json::json!({"text": "hi"})),
        ScriptedStep::text("done"),
    ])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));
    let runtime = AgentRuntime::new(agent);

    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = tokio::spawn(async move { runtime.run(RunRequest::new("test"), &mut rx).await });

    let _ = tx.send(Op::SwitchAgent {
        name: "plan".into(),
    });
    drop(tx);

    let _ = handle.await.unwrap();

    let events = observer.events();
    let switched = events
        .iter()
        .any(|e| matches!(e, joker::Event::AgentSwitched { to, .. } if to == "plan"));
    assert!(switched, "should emit AgentSwitched to 'plan'");
}

#[tokio::test]
async fn old_agent_run_api_still_works() {
    let observer = RecordingObserver::new();
    let model =
        Arc::new(ScriptedModel::new([ScriptedStep::text("hello")])) as Arc<dyn joker::Model>;
    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));

    let outcome = agent.run(RunRequest::new("test")).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Stop);

    let events = observer.events();
    assert!(matches!(events[0], joker::Event::RunStarted));
    assert!(matches!(
        events.last(),
        Some(joker::Event::RunFinished { .. })
    ));
}
