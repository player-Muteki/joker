use std::sync::Arc;

use joker::{
    Agent, AgentRuntime, Event, Op, RecordingObserver, RunRequest, ScriptedModel, ScriptedStep,
    ToolAnnotations, ToolDefinition, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput,
    ToolRegistry,
};
use serde_json::json;
use tokio::sync::mpsc;

fn echo(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move { Ok(ToolOutput::new(invocation.arguments)) })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let observer = RecordingObserver::new();

    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("c1", "echo", json!({"x": 1})),
        ScriptedStep::text("result from tool"),
    ])) as Arc<dyn joker::Model>;

    let mut registry = ToolRegistry::new();
    registry
        .insert(ToolFn::new(
            ToolDefinition {
                name: ToolName::new("echo"),
                description: "echo".into(),
                input_schema: json!({"type":"object"}),
                annotations: ToolAnnotations::default(),
            },
            echo,
        ))
        .unwrap();

    let agent = Agent::new(model)
        .with_tools(Arc::new(registry))
        .with_observer(Arc::new(observer.clone()));
    let runtime = AgentRuntime::new(agent);

    let (tx, mut rx) = mpsc::unbounded_channel();

    let handle = tokio::spawn(async move { runtime.run(RunRequest::new("start"), &mut rx).await });

    tx.send(Op::Compact).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    tx.send(Op::Shutdown).unwrap();
    drop(tx);

    let result = handle.await.unwrap();
    match result {
        Err(joker::RunError::Shutdown) => println!("runtime shut down via Op"),
        other => println!("runtime finished: {other:?}"),
    }

    println!("events:");
    for event in observer.events() {
        match event {
            Event::CompactionStarted { .. } => println!("  - CompactionStarted"),
            Event::CompactionDone { .. } => println!("  - CompactionDone"),
            _ => {}
        }
    }
}
