use std::sync::Arc;

use joker::{
    Agent, Event, RecordingObserver, RunRequest, ScriptedModel, ScriptedStep, Tool,
    ToolAnnotations, ToolDefinition, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput,
    ToolRegistry,
};
use serde_json::json;

fn echo(invocation: ToolInvocation) -> ToolFuture<'static> {
    Box::pin(async move { Ok(ToolOutput::new(invocation.arguments)) })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let observer = RecordingObserver::new();

    let model = Arc::new(ScriptedModel::new([
        ScriptedStep::tool_call("c1", "echo", json!({"msg": "hi"})),
        ScriptedStep::text("done"),
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

    let _ = agent.run(RunRequest::new("echo hi")).await.unwrap();

    println!("captured {} events:", observer.events().len());
    for event in observer.events() {
        match event {
            Event::RunStarted => println!("  - RunStarted"),
            Event::TextDelta { delta } => println!("  - TextDelta: {delta:?}"),
            Event::ToolDispatch { call_id, tool_name, .. } => {
                println!("  - ToolDispatch: {tool_name} ({call_id})");
            }
            Event::ToolStarted { name, .. } => println!("  - ToolStarted: {name}"),
            Event::ToolFinished { result, .. } => {
                let ok = if result.is_error { "ERROR" } else { "OK" };
                println!("  - ToolFinished: {} = {ok}", result.name);
            }
            Event::ModelFinished { stop_reason } => {
                println!("  - ModelFinished: {stop_reason:?}");
            }
            Event::RunFinished { .. } => println!("  - RunFinished"),
            _ => {}
        }
    }
}
