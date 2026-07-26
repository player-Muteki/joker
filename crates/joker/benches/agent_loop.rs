use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use joker::{
    Agent, AgentBuilder, AgentConfig, AgentRuntime, Content, ExecutionMode, Op, RecordingObserver,
    RunRequest, ScriptedModel, ScriptedStep, StopReason, ToolAnnotations, ToolDefinition,
    ToolExecution, ToolFn, ToolFuture, ToolInvocation, ToolName, ToolOutput, ToolRegistry,
};
use serde_json::json;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

fn echo_tool() -> ToolFn<fn(ToolInvocation) -> ToolFuture<'static>> {
    ToolFn::new(
        ToolDefinition {
            name: ToolName::new("echo"),
            description: "echo input".into(),
            input_schema: json!({"type":"object"}),
            annotations: ToolAnnotations {
                execution: ToolExecution::Sequential,
                mutating: false,
                timeout: None,
                ..ToolAnnotations::default()
            },
        },
        |invocation: ToolInvocation| -> ToolFuture<'static> {
            Box::pin(async move { Ok(ToolOutput::new(invocation.arguments)) })
        },
    )
}

fn bench_text_only_turn(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("agent_loop")
        .bench_function("text_only_turn", |b| {
            b.to_async(&rt).iter(|| {
                Box::pin(async {
                    let model = Arc::new(ScriptedModel::new([ScriptedStep::text("hello world")]))
                        as Arc<dyn joker::Model>;
                    let agent = Agent::new(model);
                    let _ = agent.run(RunRequest::new("hi")).await.unwrap();
                })
            })
        });
}

fn bench_tool_call_turn(c: &mut Criterion) {
    let registry = Arc::new({
        let mut r = ToolRegistry::new();
        r.insert(echo_tool()).unwrap();
        r
    });
    let rt = Runtime::new().unwrap();
    c.benchmark_group("agent_loop")
        .bench_function("single_tool_call_turn", |b| {
            b.to_async(&rt).iter(|| {
                let reg = registry.clone();
                Box::pin(async move {
                    let model = Arc::new(ScriptedModel::new([
                        ScriptedStep::tool_call("call-1", "echo", json!({"x": 1})),
                        ScriptedStep::text("done"),
                    ])) as Arc<dyn joker::Model>;
                    let agent = Agent::new(model).with_tools(reg);
                    let _ = agent.run(RunRequest::new("use echo")).await.unwrap();
                })
            })
        });
}

fn bench_multi_tool_turn(c: &mut Criterion) {
    let registry = Arc::new({
        let mut r = ToolRegistry::new();
        r.insert(echo_tool()).unwrap();
        r
    });
    let rt = Runtime::new().unwrap();
    c.benchmark_group("agent_loop")
        .bench_function("parallel_tool_calls_turn", |b| {
            b.to_async(&rt).iter(|| {
                let reg = registry.clone();
                Box::pin(async move {
                    let model = Arc::new(ScriptedModel::new([
                        ScriptedStep::message(
                            vec![
                                Content::ToolCall(joker::ToolCall {
                                    id: "c1".into(),
                                    name: "echo".into(),
                                    arguments: json!({"i": 1}),
                                }),
                                Content::ToolCall(joker::ToolCall {
                                    id: "c2".into(),
                                    name: "echo".into(),
                                    arguments: json!({"i": 2}),
                                }),
                                Content::ToolCall(joker::ToolCall {
                                    id: "c3".into(),
                                    name: "echo".into(),
                                    arguments: json!({"i": 3}),
                                }),
                            ],
                            StopReason::ToolUse,
                        ),
                        ScriptedStep::text("done"),
                    ])) as Arc<dyn joker::Model>;
                    let agent = AgentBuilder::new(model)
                        .tools(reg)
                        .config(AgentConfig {
                            execution_mode: ExecutionMode::ParallelWhenSafe,
                            ..AgentConfig::default()
                        })
                        .build();
                    let _ = agent.run(RunRequest::new("use tools")).await.unwrap();
                })
            })
        });
}

fn bench_op_processing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("agent_loop")
        .bench_function("op_processing_overhead", |b| {
            b.to_async(&rt).iter(|| {
                Box::pin(async {
                    let model = Arc::new(ScriptedModel::new([
                        ScriptedStep::tool_call("c1", "echo", json!({"x": 1})),
                        ScriptedStep::text("done"),
                    ])) as Arc<dyn joker::Model>;
                    let agent = Agent::new(model);
                    let runtime = AgentRuntime::new(agent);
                    let (tx, mut rx) = mpsc::unbounded_channel();
                    let _ = tx.send(Op::Compact);
                    let _ = runtime.run(RunRequest::new("test"), &mut rx).await;
                })
            })
        });
}

fn bench_event_recording(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.benchmark_group("agent_loop")
        .bench_function("event_recording_overhead", |b| {
            b.to_async(&rt).iter(|| {
                Box::pin(async {
                    let observer = RecordingObserver::new();
                    let model = Arc::new(ScriptedModel::new([
                        ScriptedStep::text("hello"),
                        ScriptedStep::text("world"),
                    ])) as Arc<dyn joker::Model>;
                    let agent = Agent::new(model).with_observer(Arc::new(observer.clone()));
                    let _ = agent.run(RunRequest::new("test")).await.unwrap();
                    let _ = observer.events();
                })
            })
        });
}

criterion_group!(
    name = agent_loop;
    config = Criterion::default().sample_size(100);
    targets = bench_text_only_turn, bench_tool_call_turn, bench_multi_tool_turn,
             bench_op_processing, bench_event_recording
);
criterion_main!(agent_loop);
